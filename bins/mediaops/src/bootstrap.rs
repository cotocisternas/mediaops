use std::path::{Path, PathBuf};

use mediaops_core::{
    DesiredState, Envelope, ExitCode, Probe, ProviderKind, endpoint_fingerprint, upsert_tls_table,
};
use mediaops_ssh::{
    SystemExec, install_provider, parse_ssh_config, refuse_git_work_tree, systemd_user_unit,
};
use mediaops_store::Store;
use mediaops_transfer::mint;
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct BootstrapArgs {
    pub provider: ProviderKind,
    pub yes: bool,
    pub config_dir: PathBuf,
    pub desired_state: PathBuf,
    pub ssh_config: PathBuf,
    pub state_db: PathBuf,
    pub address: String,
    pub skip_probe: bool,
}

#[derive(Debug, Serialize)]
pub struct BootstrapReport {
    pub provider: String,
    pub host_alias: String,
    pub endpoint_fingerprint: String,
    pub range_concurrency: u32,
    pub steps: Vec<String>,
    pub applied: bool,
}

pub fn plan_steps(args: &BootstrapArgs) -> Vec<String> {
    let mut steps = vec![
        format!("import ssh Host seedbox from {}", args.ssh_config.display()),
        format!("mint ECDSA P-256 certs into {}", args.config_dir.join("tls").display()),
        format!("write fingerprints into {}", args.desired_state.display()),
    ];
    match args.provider {
        ProviderKind::AlreadyThere => steps.push("AlreadyThere: no-op install".into()),
        ProviderKind::SwizzinBox => {
            steps.push("build x86_64-unknown-linux-musl mediaopsd".into());
            steps.push("scp binary + systemd user unit to Host seedbox".into());
        }
        other => steps.push(format!("provider {other} is unimplemented")),
    }
    if args.skip_probe {
        steps.push("skip range probe".into());
    } else {
        steps.push(format!("probe Range concurrency at {}", args.address));
    }
    steps
}

pub async fn bootstrap(args: BootstrapArgs) -> Result<BootstrapReport, BootstrapError> {
    args.provider
        .ensure_installable()
        .map_err(BootstrapError::Provider)?;
    let ssh_text =
        std::fs::read_to_string(&args.ssh_config).map_err(|err| BootstrapError::Io(err.to_string()))?;
    let host = parse_ssh_config(&ssh_text, "seedbox").map_err(|err| BootstrapError::Ssh(err.to_string()))?;
    let tls_dir = args.config_dir.join("tls");
    refuse_git_work_tree(&args.config_dir)
        .and_then(|_| refuse_git_work_tree(&tls_dir))
        .map_err(|err| BootstrapError::Ssh(err.to_string()))?;

    let steps = plan_steps(&args);
    if !args.yes {
        return Err(BootstrapError::NeedsConfirm(BootstrapReport {
            provider: args.provider.as_str().to_string(),
            host_alias: host.alias,
            endpoint_fingerprint: endpoint_fingerprint(&args.address, DesiredState::from_toml(
                &std::fs::read_to_string(&args.desired_state).unwrap_or_default(),
            )
            .map(|ds| ds.underlay())
            .unwrap_or_default()),
            range_concurrency: 0,
            steps,
            applied: false,
        }));
    }

    let toml_text =
        std::fs::read_to_string(&args.desired_state).map_err(|err| BootstrapError::Io(err.to_string()))?;
    let ds = DesiredState::from_toml(&toml_text).map_err(|err| BootstrapError::Io(err.to_string()))?;
    let underlay = ds.underlay();
    let fingerprint = endpoint_fingerprint(&args.address, underlay);

    let bundle = mint().map_err(|err| BootstrapError::Io(err.to_string()))?;
    let tls = bundle
        .write_to_dir(&tls_dir)
        .map_err(|err| BootstrapError::Io(err.to_string()))?;
    let updated = upsert_tls_table(&toml_text, &tls).map_err(|err| BootstrapError::Io(err.to_string()))?;
    if updated.contains("BEGIN ") {
        return Err(BootstrapError::Io(
            "desired-state grew a PEM body; refusing to write".into(),
        ));
    }
    std::fs::write(&args.desired_state, updated).map_err(|err| BootstrapError::Io(err.to_string()))?;

    let unit = systemd_user_unit("mediaopsd serve --role seedbox");
    let unit_path = args.config_dir.join("mediaopsd.service");
    let fake_bin = args.config_dir.join("mediaopsd");
    install_provider(&SystemExec, args.provider, &fake_bin, &unit, &unit_path)
        .await
        .map_err(|err| BootstrapError::Ssh(err.to_string()))?;

    let store = Store::open(&args.state_db).map_err(|err| BootstrapError::Io(err.to_string()))?;
    let n = if args.skip_probe {
        store
            .get_probe(&fingerprint)
            .map_err(|err| BootstrapError::Io(err.to_string()))?
            .map(|p| p.range_concurrency)
            .unwrap_or(1)
    } else if let Some(existing) = store
        .get_probe(&fingerprint)
        .map_err(|err| BootstrapError::Io(err.to_string()))?
    {
        existing.range_concurrency
    } else {
        let n = mediaops_transfer::probe_range_n(
            args.address
                .parse()
                .map_err(|err| BootstrapError::Io(format!("{err}")))?,
            bundle
                .client_config()
                .map_err(|err| BootstrapError::Io(err.to_string()))?,
            4,
        )
        .await
        .map_err(|err| BootstrapError::Io(err.to_string()))?;
        store
            .put_probe(&Probe {
                endpoint_fingerprint: fingerprint.clone(),
                range_concurrency: n,
            })
            .map_err(|err| BootstrapError::Io(err.to_string()))?;
        n
    };

    Ok(BootstrapReport {
        provider: args.provider.as_str().to_string(),
        host_alias: host.alias,
        endpoint_fingerprint: fingerprint,
        range_concurrency: n,
        steps,
        applied: true,
    })
}

#[derive(Debug)]
pub enum BootstrapError {
    Provider(mediaops_core::ProviderError),
    Ssh(String),
    Io(String),
    NeedsConfirm(BootstrapReport),
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider(err) => write!(f, "{err}"),
            Self::Ssh(err) | Self::Io(err) => write!(f, "{err}"),
            Self::NeedsConfirm(report) => write!(
                f,
                "refusing to run destructive bootstrap steps without --yes: {}",
                report.steps.join("; ")
            ),
        }
    }
}

impl BootstrapError {
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Provider(_) => ExitCode::Usage,
            Self::NeedsConfirm(_) => ExitCode::PolicyRefusal,
            Self::Ssh(_) | Self::Io(_) => ExitCode::Runtime,
        }
    }
}

pub fn render_report(json: bool, report: &BootstrapReport) -> Result<String, serde_json::Error> {
    if json {
        serde_json::to_string(&Envelope::ok(report))
    } else {
        Ok(format!(
            "seedbox bootstrap {} host={} fingerprint={} N={}",
            if report.applied { "applied" } else { "planned" },
            report.host_alias,
            report.endpoint_fingerprint,
            report.range_concurrency
        ))
    }
}

pub fn default_config_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.config_dir().join("mediaops"))
        .unwrap_or_else(|| PathBuf::from(".mediaops-config"))
}

pub fn default_state_db() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.data_local_dir().join("mediaops").join("state.db"))
        .unwrap_or_else(|| PathBuf::from(".mediaops-state").join("state.db"))
}

pub fn default_ssh_config() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().join(".ssh").join("config"))
        .unwrap_or_else(|| PathBuf::from(".ssh/config"))
}

pub fn default_desired_state(config_dir: &Path) -> PathBuf {
    config_dir.join("desired-state.toml")
}
