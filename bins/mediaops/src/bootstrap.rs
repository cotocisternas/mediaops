use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

use mediaops_core::{
    ControlPort, DesiredState, Envelope, EnvelopeError, ExecPort, ExitCode, Probe, ProviderKind,
    TlsIdentity, endpoint_fingerprint, pin_matrix_refuse, upsert_tls_table,
};
use mediaops_proto::ControlPortClient;
use mediaops_proto::control_client::ControlClient;
use mediaops_ssh::{
    SshHost, copy_binary_and_restart_unit, install_provider, musl_binary_path, parse_ssh_config,
    refuse_git_work_tree, systemd_user_unit,
};
use mediaops_store::Store;
use mediaops_transfer::{IdentityBundle, connect_home, mint, probe_range, probe_range_n};
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct BootstrapArgs {
    pub provider: ProviderKind,
    pub yes: bool,
    pub config_dir: PathBuf,
    pub desired_state: PathBuf,
    pub ssh_config: PathBuf,
    pub state_db: PathBuf,
    pub address: Option<String>,
    pub skip_probe: bool,
    pub roots: Vec<(String, PathBuf)>,
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

#[derive(Debug, Clone)]
pub struct UpgradeArgs {
    pub yes: bool,
    pub config_dir: PathBuf,
    pub desired_state: PathBuf,
    pub ssh_config: PathBuf,
    pub state_db: PathBuf,
    pub socket: PathBuf,
    pub tls_dir: PathBuf,
    pub skip_edge: bool,
}

#[derive(Debug, Serialize)]
pub struct UpgradeReport {
    pub applied: bool,
    pub steps: Vec<String>,
    pub skew: Option<String>,
    pub fingerprint: Option<String>,
}

pub fn parse_root(raw: &str) -> Result<(String, PathBuf), String> {
    let (id, path) = raw
        .split_once('=')
        .ok_or_else(|| "expected id=path".to_string())?;
    if id.is_empty() {
        return Err("empty root id".into());
    }
    Ok((id.to_string(), PathBuf::from(path)))
}

pub fn plan_steps(args: &BootstrapArgs, address: &str) -> Vec<String> {
    let mut steps = vec![
        format!("import ssh Host seedbox from {}", args.ssh_config.display()),
        format!(
            "mint ECDSA P-256 certs into {}",
            args.config_dir.join("tls").display()
        ),
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
        steps.push(format!("probe Range concurrency at {address}"));
    }
    steps
}

pub async fn bootstrap(
    args: BootstrapArgs,
    exec: &impl ExecPort,
) -> Result<BootstrapReport, BootstrapError> {
    args.provider
        .ensure_installable()
        .map_err(BootstrapError::Provider)?;
    if args.provider == ProviderKind::SwizzinBox && args.roots.is_empty() {
        return Err(BootstrapError::Usage(
            "swizzin-box bootstrap requires at least one --root id=path".into(),
        ));
    }

    std::fs::create_dir_all(&args.config_dir).map_err(|err| BootstrapError::Io(err.to_string()))?;
    let lock_file = File::create(args.config_dir.join("bootstrap.lock"))
        .map_err(|err| BootstrapError::Io(err.to_string()))?;
    match fs4::FileExt::try_lock(&lock_file) {
        Ok(()) => {}
        Err(fs4::TryLockError::WouldBlock) => return Err(BootstrapError::LockConflict),
        Err(err) => return Err(BootstrapError::Io(err.to_string())),
    }

    let ssh_text = std::fs::read_to_string(&args.ssh_config)
        .map_err(|err| BootstrapError::Io(err.to_string()))?;
    let host = parse_ssh_config(&ssh_text, "seedbox").map_err(BootstrapError::from_ssh)?;
    let tls_dir = args.config_dir.join("tls");
    refuse_git_work_tree(&args.config_dir)
        .and_then(|_| refuse_git_work_tree(&tls_dir))
        .map_err(BootstrapError::from_ssh)?;

    let address = resolve_grpc_address(args.address.as_deref(), &host)?;
    let steps = plan_steps(&args, &address);
    let underlay_for_plan = std::fs::read_to_string(&args.desired_state)
        .ok()
        .and_then(|text| DesiredState::from_toml(&text).ok())
        .map(|ds| ds.underlay())
        .unwrap_or_default();
    if !args.yes {
        return Err(BootstrapError::NeedsConfirm(BootstrapReport {
            provider: args.provider.as_str().to_string(),
            host_alias: host.alias,
            endpoint_fingerprint: endpoint_fingerprint(&address, underlay_for_plan),
            range_concurrency: 0,
            steps,
            applied: false,
        }));
    }

    let toml_text = std::fs::read_to_string(&args.desired_state)
        .map_err(|err| BootstrapError::Io(err.to_string()))?;
    let ds =
        DesiredState::from_toml(&toml_text).map_err(|err| BootstrapError::Io(err.to_string()))?;
    let underlay = ds.underlay();
    let fingerprint = endpoint_fingerprint(&address, underlay);

    let store = Store::open(&args.state_db)
        .await
        .map_err(|err| BootstrapError::Io(err.to_string()))?;
    let existing = store
        .get_probe(&fingerprint)
        .await
        .map_err(|err| BootstrapError::Io(err.to_string()))?;
    let probe_plan = if args.skip_probe {
        match existing {
            Some(probe) => ProbePlan::Reuse(probe.range_concurrency),
            None => {
                return Err(BootstrapError::Io(
                    "--skip-probe requires an existing probes row for this endpoint".into(),
                ));
            }
        }
    } else {
        match existing {
            Some(probe) => ProbePlan::Reuse(probe.range_concurrency),
            None => ProbePlan::Sweep,
        }
    };

    let reuse_tls = tls_bundle_on_disk(&tls_dir);
    let bundle = if reuse_tls {
        IdentityBundle::from_dir(&tls_dir).map_err(|err| BootstrapError::Io(err.to_string()))?
    } else {
        let bundle = mint().map_err(|err| BootstrapError::Io(err.to_string()))?;
        bundle
            .write_to_dir(&tls_dir)
            .map_err(|err| BootstrapError::Io(err.to_string()))?;
        bundle
    };
    if !reuse_tls || ds.tls().is_none() {
        let tls = tls_identity_from_bundle(&tls_dir, &bundle);
        let updated = upsert_tls_table(&toml_text, &tls)
            .map_err(|err| BootstrapError::Io(err.to_string()))?;
        if updated.contains("BEGIN ") {
            return Err(BootstrapError::Io(
                "desired-state grew a PEM body; refusing to write".into(),
            ));
        }
        write_atomic(&args.desired_state, &updated)?;
    }

    let unit = systemd_user_unit(&seedbox_exec_start(&args.roots));
    let unit_path = args.config_dir.join("mediaopsd.service");
    install_provider(
        exec,
        args.provider,
        &musl_binary_path(),
        &unit,
        &unit_path,
        &tls_dir,
        &args.ssh_config,
    )
    .await
    .map_err(BootstrapError::from_ssh)?;

    let n = match probe_plan {
        ProbePlan::Reuse(n) => n,
        ProbePlan::Sweep => {
            let n = match connect_home(&default_socket(), &tls_dir).await {
                Ok(channel) => match probe_range(channel, 32).await {
                    Ok(n) => n,
                    Err(_) => {
                        let sock = resolve_socket_addr(&address)?;
                        let client = bundle
                            .client_config()
                            .map_err(|err| BootstrapError::Io(err.to_string()))?;
                        probe_range_n(sock, client, 32)
                            .await
                            .map_err(|err| BootstrapError::Io(err.to_string()))?
                    }
                },
                Err(_) => {
                    let sock = resolve_socket_addr(&address)?;
                    let client = bundle
                        .client_config()
                        .map_err(|err| BootstrapError::Io(err.to_string()))?;
                    probe_range_n(sock, client, 32)
                        .await
                        .map_err(|err| BootstrapError::Io(err.to_string()))?
                }
            };
            store
                .put_probe(&Probe {
                    endpoint_fingerprint: fingerprint.clone(),
                    range_concurrency: n,
                })
                .await
                .map_err(|err| BootstrapError::Io(err.to_string()))?;
            n
        }
    };

    drop(lock_file);
    Ok(BootstrapReport {
        provider: args.provider.as_str().to_string(),
        host_alias: host.alias,
        endpoint_fingerprint: fingerprint,
        range_concurrency: n,
        steps,
        applied: true,
    })
}

pub async fn upgrade(
    args: UpgradeArgs,
    exec: &impl ExecPort,
) -> Result<UpgradeReport, BootstrapError> {
    std::fs::create_dir_all(&args.config_dir).map_err(|err| BootstrapError::Io(err.to_string()))?;
    let _lock = exclusive_lock(&lock_path(&args.state_db))?;
    let toml_text = std::fs::read_to_string(&args.desired_state)
        .map_err(|err| BootstrapError::Io(err.to_string()))?;
    let ds =
        DesiredState::from_toml(&toml_text).map_err(|err| BootstrapError::Io(err.to_string()))?;
    if let Some(msg) = pin_matrix_refuse(ds.pins()) {
        return Err(BootstrapError::Policy(msg));
    }
    let steps = vec![
        "copy musl-static mediaopsd over ssh".into(),
        "restart mediaopsd.service".into(),
        "edge check".into(),
    ];
    if !args.yes {
        return Err(BootstrapError::Policy(format!(
            "refusing seedbox upgrade without --yes: {}",
            steps.join("; ")
        )));
    }
    copy_binary_and_restart_unit(exec, &musl_binary_path(), &args.ssh_config)
        .await
        .map_err(BootstrapError::from_ssh)?;
    let mut skew = None;
    let mut fingerprint = None;
    if !args.skip_edge {
        let channel = connect_home(&args.socket, &args.tls_dir)
            .await
            .map_err(|err| BootstrapError::Io(err.to_string()))?;
        let control = ControlPortClient::new(ControlClient::new(channel));
        let df = control
            .df()
            .await
            .map_err(|err| BootstrapError::Io(err.message))?;
        skew = mediaops_proto::minor_skew_warning(&df.semver, env!("CARGO_PKG_VERSION"));
        if let Some(msg) = &skew {
            tracing::warn!("{msg}");
        }
        let edge = control
            .edge_check()
            .await
            .map_err(|err| BootstrapError::Io(err.message))?;
        if !edge.invariant_ok {
            return Err(BootstrapError::Policy(format!(
                "edge check after upgrade drifted: {}",
                edge.drift
            )));
        }
        fingerprint = Some(edge.fingerprint);
    }
    Ok(UpgradeReport {
        applied: true,
        steps,
        skew,
        fingerprint,
    })
}

enum ProbePlan {
    Reuse(u32),
    Sweep,
}

fn tls_bundle_on_disk(dir: &Path) -> bool {
    [
        "ca.pem",
        "server.pem",
        "server.key",
        "client.pem",
        "client.key",
    ]
    .into_iter()
    .all(|name| dir.join(name).is_file())
}

fn tls_identity_from_bundle(tls_dir: &Path, bundle: &IdentityBundle) -> TlsIdentity {
    TlsIdentity {
        ca_path: path_string(&tls_dir.join("ca.pem")),
        server_cert_path: path_string(&tls_dir.join("server.pem")),
        server_key_path: path_string(&tls_dir.join("server.key")),
        client_cert_path: path_string(&tls_dir.join("client.pem")),
        client_key_path: path_string(&tls_dir.join("client.key")),
        ca_sha256: bundle.ca_sha256.clone(),
        server_sha256: bundle.server_sha256.clone(),
        client_sha256: bundle.client_sha256.clone(),
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn seedbox_exec_start(roots: &[(String, PathBuf)]) -> String {
    let mut cmd = String::from(
        "%h/.local/bin/mediaopsd serve --role seedbox --bind 0.0.0.0:50051 --tls-dir %h/.config/mediaops/tls",
    );
    for (id, path) in roots {
        cmd.push_str(" --root ");
        cmd.push_str(id);
        cmd.push('=');
        cmd.push_str(&path.display().to_string());
    }
    cmd
}

fn write_atomic(path: &Path, contents: &str) -> Result<(), BootstrapError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|err| BootstrapError::Io(err.to_string()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("desired-state.toml");
    let tmp = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, contents).map_err(|err| BootstrapError::Io(err.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|err| BootstrapError::Io(err.to_string()))?;
    Ok(())
}

/// gRPC listen port is 50051; ssh `Port` (often 2097) is not the probe address.
fn resolve_grpc_address(explicit: Option<&str>, host: &SshHost) -> Result<String, BootstrapError> {
    if let Some(address) = explicit.filter(|a| !a.is_empty()) {
        return Ok(address.to_string());
    }
    let hostname = host.hostname.as_deref().ok_or_else(|| {
        BootstrapError::Usage("Host seedbox has no HostName; pass --address HOST:50051".into())
    })?;
    Ok(format!("{hostname}:50051"))
}

fn resolve_socket_addr(address: &str) -> Result<SocketAddr, BootstrapError> {
    if let Ok(sock) = address.parse::<SocketAddr>() {
        return Ok(sock);
    }
    address
        .to_socket_addrs()
        .map_err(|err| BootstrapError::Io(err.to_string()))?
        .next()
        .ok_or_else(|| BootstrapError::Io(format!("could not resolve {address}")))
}

#[derive(Debug)]
pub enum BootstrapError {
    Provider(mediaops_core::ProviderError),
    Ssh(String),
    Io(String),
    Usage(String),
    Policy(String),
    LockConflict,
    NeedsConfirm(BootstrapReport),
}

impl BootstrapError {
    fn from_ssh(err: mediaops_ssh::SshError) -> Self {
        match err {
            mediaops_ssh::SshError::GitWorkTree(path) => {
                Self::Policy(format!("refusing to mint TLS into a git work tree: {path}"))
            }
            other => Self::Ssh(other.to_string()),
        }
    }
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider(err) => write!(f, "{err}"),
            Self::Ssh(err) | Self::Io(err) | Self::Usage(err) | Self::Policy(err) => {
                write!(f, "{err}")
            }
            Self::LockConflict => write!(f, "bootstrap lock is held by another process"),
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
            Self::Provider(_) | Self::Usage(_) => ExitCode::Usage,
            Self::NeedsConfirm(_) | Self::Policy(_) => ExitCode::PolicyRefusal,
            Self::LockConflict => ExitCode::LockConflict,
            Self::Ssh(_) | Self::Io(_) => ExitCode::Runtime,
        }
    }
}

pub fn render_upgrade(json: bool, report: &UpgradeReport) -> Result<String, serde_json::Error> {
    if json {
        serde_json::to_string(&Envelope::ok(report))
    } else {
        Ok(format!(
            "seedbox upgrade {} steps={}",
            if report.applied { "applied" } else { "planned" },
            report.steps.len()
        ))
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

pub fn render_needs_confirm(
    json: bool,
    report: &BootstrapReport,
) -> Result<String, serde_json::Error> {
    if json {
        let envelope = Envelope {
            ok: false,
            data: Some(report),
            error: Some(EnvelopeError {
                code: ExitCode::PolicyRefusal.error_code().to_string(),
                message: format!(
                    "refusing to run destructive bootstrap steps without --yes: {}",
                    report.steps.join("; ")
                ),
            }),
        };
        serde_json::to_string(&envelope)
    } else {
        let mut out = format!(
            "seedbox bootstrap planned host={} fingerprint={} N={}\n",
            report.host_alias, report.endpoint_fingerprint, report.range_concurrency
        );
        for step in &report.steps {
            out.push_str(step);
            out.push('\n');
        }
        out.push_str("refusing to run destructive bootstrap steps without --yes");
        Ok(out)
    }
}

pub fn default_config_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.config_dir().join("mediaops"))
        .unwrap_or_else(|| PathBuf::from(".mediaops-config"))
}

pub fn default_state_db() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| {
            b.state_dir()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| b.home_dir().join(".local").join("state"))
                .join("mediaops")
                .join("state.db")
        })
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

pub fn default_tls_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("tls")
}

pub fn default_state_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| {
            b.state_dir()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| b.home_dir().join(".local").join("state"))
                .join("mediaops")
        })
        .unwrap_or_else(|| PathBuf::from(".mediaops-state"))
}

pub fn default_socket() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|dir| PathBuf::from(dir).join("mediaopsd.sock"))
        .unwrap_or_else(|| default_state_dir().join("mediaopsd.sock"))
}

pub fn default_unit_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.config_dir().join("systemd").join("user"))
        .unwrap_or_else(|| PathBuf::from(".config/systemd/user"))
}

pub fn lock_path(state_db: &Path) -> PathBuf {
    state_db
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("mediaops.lock")
}

pub fn default_plans_dir() -> PathBuf {
    default_state_dir().join("plans")
}

/// Compact UTC stamp `YYYYMMDDTHHMMSSZ` without a datetime crate.
pub fn utc_compact() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    utc_compact_secs(secs)
}

pub(crate) fn utc_compact_secs(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}{m:02}{d:02}T{hour:02}{min:02}{sec:02}Z")
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

/// If another process holds the exclusive flock, return the lockfile JSON.
pub fn lock_holder_if_contended(path: &Path) -> Result<Option<serde_json::Value>, BootstrapError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(BootstrapError::Io(err.to_string())),
    };
    match fs4::FileExt::try_lock_shared(&file) {
        Ok(()) => {
            let _ = fs4::FileExt::unlock(&file);
            Ok(None)
        }
        Err(fs4::TryLockError::WouldBlock) => {
            let text =
                std::fs::read_to_string(path).map_err(|err| BootstrapError::Io(err.to_string()))?;
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(Some(serde_json::json!({"unparsed": ""})));
            }
            match serde_json::from_str(trimmed) {
                Ok(value) => Ok(Some(value)),
                Err(_) => Ok(Some(serde_json::json!({ "unparsed": trimmed }))),
            }
        }
        Err(err) => Err(BootstrapError::Io(err.to_string())),
    }
}

pub fn exclusive_lock(path: &Path) -> Result<File, BootstrapError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| BootstrapError::Io(err.to_string()))?;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|err| BootstrapError::Io(err.to_string()))?;
    match fs4::FileExt::try_lock(&file) {
        Ok(()) => {
            write_lock_holder(&mut file)?;
            Ok(file)
        }
        Err(fs4::TryLockError::WouldBlock) => Err(BootstrapError::LockConflict),
        Err(err) => Err(BootstrapError::Io(err.to_string())),
    }
}

fn write_lock_holder(file: &mut File) -> Result<(), BootstrapError> {
    let started_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let command = std::env::args().collect::<Vec<_>>().join(" ");
    let record = serde_json::json!({
        "pid": std::process::id(),
        "started_at": started_at,
        "command": command,
    });
    file.set_len(0)
        .map_err(|err| BootstrapError::Io(err.to_string()))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|err| BootstrapError::Io(err.to_string()))?;
    file.write_all(record.to_string().as_bytes())
        .map_err(|err| BootstrapError::Io(err.to_string()))?;
    file.write_all(b"\n")
        .map_err(|err| BootstrapError::Io(err.to_string()))?;
    file.sync_all()
        .map_err(|err| BootstrapError::Io(err.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{Probe, UnderlayMode};
    use mediaops_ssh::{TranscriptExec, musl_binary_path};

    const DS: &str = "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 1\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n";

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-boot-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn write_ssh(dir: &Path) -> PathBuf {
        let ssh = dir.join("ssh_config");
        std::fs::write(
            &ssh,
            "Host seedbox\n  HostName 127.0.0.1\n  User x\n  Port 2097\n",
        )
        .expect("ssh");
        ssh
    }

    fn write_ds(dir: &Path) -> PathBuf {
        let ds = dir.join("desired-state.toml");
        if !ds.exists() {
            std::fs::write(&ds, DS).expect("ds");
        }
        ds
    }

    fn args(dir: &Path, yes: bool, skip_probe: bool, provider: ProviderKind) -> BootstrapArgs {
        BootstrapArgs {
            provider,
            yes,
            config_dir: dir.to_path_buf(),
            desired_state: write_ds(dir),
            ssh_config: write_ssh(dir),
            state_db: dir.join("state.db"),
            address: None,
            skip_probe,
            roots: Vec::new(),
        }
    }

    fn address_fp() -> String {
        endpoint_fingerprint("127.0.0.1:50051", UnderlayMode::Direct)
    }

    async fn seed_probe(path: &Path, n: u32) {
        let store = Store::open(path).await.expect("store");
        store
            .put_probe(&Probe {
                endpoint_fingerprint: address_fp(),
                range_concurrency: n,
            })
            .await
            .expect("put");
    }

    #[tokio::test]
    async fn yes_already_there_skip_probe_mints_once() {
        let dir = scratch("yes-already");
        seed_probe(&dir.join("state.db"), 8).await;
        let exec = TranscriptExec::new();
        let report = bootstrap(args(&dir, true, true, ProviderKind::AlreadyThere), &exec)
            .await
            .expect("apply");
        assert!(report.applied);
        assert_eq!(report.range_concurrency, 8);
        let tls = dir.join("tls");
        for name in [
            "ca.pem",
            "server.pem",
            "server.key",
            "client.pem",
            "client.key",
        ] {
            assert!(tls.join(name).is_file(), "missing {name}");
        }
        let toml_text = std::fs::read_to_string(dir.join("desired-state.toml")).expect("ds");
        assert!(!toml_text.contains("BEGIN "));
        assert!(!toml_text.contains("-----BEGIN"));
        let ds = DesiredState::from_toml(&toml_text).expect("parse");
        let tls_id = ds.tls().expect("tls table");
        for fp in [
            &tls_id.ca_sha256,
            &tls_id.server_sha256,
            &tls_id.client_sha256,
        ] {
            assert_eq!(fp.len(), 64);
            assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
        }
        assert!(exec.recorded().is_empty());
        let ca = std::fs::read(tls.join("ca.pem")).expect("ca");
        let exec2 = TranscriptExec::new();
        let again = bootstrap(args(&dir, true, true, ProviderKind::AlreadyThere), &exec2)
            .await
            .expect("second");
        assert!(again.applied);
        assert_eq!(ca, std::fs::read(tls.join("ca.pem")).expect("ca2"));
        let toml2 = std::fs::read_to_string(dir.join("desired-state.toml")).expect("ds2");
        assert_eq!(toml_text, toml2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn skip_probe_without_row_errors() {
        let dir = scratch("skip-missing");
        let exec = TranscriptExec::new();
        let err = bootstrap(args(&dir, true, true, ProviderKind::AlreadyThere), &exec)
            .await
            .expect_err("missing row");
        assert!(err.to_string().contains("skip-probe"));
        assert!(!dir.join("tls").join("ca.pem").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn git_work_tree_refuses_before_mint() {
        let dir = scratch("git");
        std::fs::create_dir_all(dir.join(".git")).expect("git");
        let exec = TranscriptExec::new();
        let err = bootstrap(args(&dir, true, true, ProviderKind::AlreadyThere), &exec)
            .await
            .expect_err("git");
        assert_eq!(err.exit_code(), ExitCode::PolicyRefusal);
        assert!(err.to_string().contains("git"));
        assert!(!dir.join("tls").join("ca.pem").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn swizzin_scp_uses_musl_artifact_path() {
        let dir = scratch("swizzin");
        seed_probe(&dir.join("state.db"), 4).await;
        let exec = TranscriptExec::new();
        let mut boot = args(&dir, true, true, ProviderKind::SwizzinBox);
        boot.roots = vec![("media".into(), PathBuf::from("/data/media"))];
        let report = bootstrap(boot, &exec).await.expect("swizzin");
        assert!(report.applied);
        let calls = exec.recorded();
        let musl = musl_binary_path();
        let musl_s = musl.display().to_string();
        assert!(
            calls
                .iter()
                .any(|c| { c.program_name() == "scp" && c.args.iter().any(|a| a == &musl_s) }),
            "expected musl path {musl_s} in scp, got {calls:?}"
        );
        assert!(
            !calls
                .iter()
                .any(|c| c.args.iter().any(|a| a.contains("client.key")))
        );
        let unit = std::fs::read_to_string(dir.join("mediaopsd.service")).expect("unit");
        assert!(unit.contains("WantedBy=default.target"));
        assert!(unit.contains("--tls-dir"));
        assert!(unit.contains("--bind 0.0.0.0:50051"));
        assert!(unit.contains("--root media=/data/media"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn lock_conflict_is_exit_3() {
        let dir = scratch("lock");
        seed_probe(&dir.join("state.db"), 2).await;
        let held = File::create(dir.join("bootstrap.lock")).expect("lock file");
        fs4::FileExt::try_lock(&held).expect("hold");
        let exec = TranscriptExec::new();
        let err = bootstrap(args(&dir, true, true, ProviderKind::AlreadyThere), &exec)
            .await
            .expect_err("conflict");
        assert!(matches!(err, BootstrapError::LockConflict));
        assert_eq!(err.exit_code(), ExitCode::LockConflict);
        assert!(!dir.join("tls").join("ca.pem").exists());
        drop(held);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn default_state_db_is_xdg_state_not_share() {
        let path = default_state_db();
        let rendered = path.to_string_lossy();
        assert!(
            rendered.contains("/.local/state/") || rendered.ends_with("state.db"),
            "{rendered}"
        );
        assert!(
            !rendered.contains("/.local/share/"),
            "AD-7 is ~/.local/state not share: {rendered}"
        );
    }

    #[test]
    fn utc_compact_epoch_is_1970() {
        assert_eq!(utc_compact_secs(0), "19700101T000000Z");
        assert_eq!(utc_compact_secs(86_400), "19700102T000000Z");
    }

    #[test]
    fn exclusive_lock_records_pid_and_command() {
        let dir = scratch("lock");
        let path = dir.join("mediaops.lock");
        let _file = exclusive_lock(&path).expect("lock");
        let text = std::fs::read_to_string(&path).expect("read");
        let value: serde_json::Value = serde_json::from_str(text.trim()).expect("json");
        assert_eq!(value["pid"], std::process::id());
        assert!(value["started_at"].as_u64().is_some());
        assert!(
            value["command"].as_str().is_some_and(|c| !c.is_empty()),
            "command: {value}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn exclusive_lock_conflict_does_not_truncate_holder() {
        let dir = scratch("lock-trunc");
        let path = dir.join("mediaops.lock");
        let _held = exclusive_lock(&path).expect("first");
        let before = std::fs::read_to_string(&path).expect("read");
        let err = exclusive_lock(&path).expect_err("conflict");
        assert!(matches!(err, BootstrapError::LockConflict));
        let after = std::fs::read_to_string(&path).expect("read");
        assert_eq!(
            before, after,
            "conflicting locker must not wipe holder JSON"
        );
        let parsed: serde_json::Value = serde_json::from_str(after.trim()).expect("json");
        assert_eq!(parsed["pid"], std::process::id());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn lidarr_glibc_trap_refuses_upgrade() {
        let dir = scratch("upgrade-pin");
        let ds = dir.join("desired-state.toml");
        std::fs::write(
            &ds,
            r#"
schema_version = 1
max_copy_gib = 1
min_free_gib = 0
range_len_mib = 1
max_nvenc = 1
lock = false
[pins]
lidarr = "2.15.0"
[[pins.matrix]]
package = "lidarr"
os = "ubuntu-20.04"
glibc_min = "2.31"
refuse_above = "2.14.5"
"#,
        )
        .expect("ds");
        let err = upgrade(
            UpgradeArgs {
                yes: true,
                config_dir: dir.clone(),
                desired_state: ds,
                ssh_config: dir.join("ssh"),
                state_db: dir.join("state.db"),
                socket: dir.join("sock"),
                tls_dir: dir.join("tls"),
                skip_edge: true,
            },
            &mediaops_ssh::TranscriptExec::new(),
        )
        .await
        .expect_err("pin");
        assert!(matches!(err, BootstrapError::Policy(ref m) if m.contains("Lidarr glibc trap")));
        assert_eq!(err.exit_code(), ExitCode::PolicyRefusal);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn upgrade_without_yes_is_policy_refusal() {
        let dir = scratch("upgrade-yes");
        let ds = dir.join("desired-state.toml");
        std::fs::write(
            &ds,
            "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 1\nmax_nvenc = 1\nlock = false\n",
        )
        .expect("ds");
        let err = upgrade(
            UpgradeArgs {
                yes: false,
                config_dir: dir.clone(),
                desired_state: ds,
                ssh_config: dir.join("ssh"),
                state_db: dir.join("state.db"),
                socket: dir.join("sock"),
                tls_dir: dir.join("tls"),
                skip_edge: true,
            },
            &mediaops_ssh::TranscriptExec::new(),
        )
        .await
        .expect_err("yes");
        assert!(matches!(err, BootstrapError::Policy(_)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn upgrade_with_yes_copies_binary_skipping_edge() {
        let dir = scratch("upgrade-yes-run");
        let ds = dir.join("desired-state.toml");
        std::fs::write(
            &ds,
            "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 1\nmax_nvenc = 1\nlock = false\n",
        )
        .expect("ds");
        let exec = mediaops_ssh::TranscriptExec::new().reply(
            "cargo",
            mediaops_core::ExecOutput {
                status: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
        );
        let report = upgrade(
            UpgradeArgs {
                yes: true,
                config_dir: dir.clone(),
                desired_state: ds,
                ssh_config: dir.join("ssh"),
                state_db: dir.join("state.db"),
                socket: dir.join("sock"),
                tls_dir: dir.join("tls"),
                skip_edge: true,
            },
            &exec,
        )
        .await
        .expect("upgrade");
        assert!(report.applied);
        assert!(exec.recorded().iter().any(|c| c.program_name() == "scp"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
