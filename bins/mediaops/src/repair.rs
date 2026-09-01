use std::path::PathBuf;

use mediaops_core::{ControlPort, Envelope};
use mediaops_proto::ControlPortClient;
use mediaops_proto::control_client::ControlClient;
use mediaops_ssh::{SystemExec, desired_nginx_app, write_remote_file};
use mediaops_store::Store;
use mediaops_transfer::connect_home;
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;
use crate::doctor::{EDGE_FINGERPRINT_KEY, confirm_or_pin};

#[derive(Debug, Serialize)]
struct RepairData {
    pub noop: bool,
    pub diff: String,
    pub fingerprint: String,
}

pub async fn repair_edge(
    json: bool,
    repair: bool,
    confirm: bool,
    pin: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    state_db: Option<PathBuf>,
    ssh_config: Option<PathBuf>,
) -> Result<String, AppError> {
    if !repair {
        return Err(AppError::Usage(
            "repair edge requires --repair plus --confirm or --pin".into(),
        ));
    }
    confirm_or_pin(confirm, pin.as_deref())?;
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let desired_state =
        desired_state.unwrap_or_else(|| bootstrap::default_desired_state(&config_dir));
    let lock_path = bootstrap::lock_path(&state_db);
    let _lock = bootstrap::exclusive_lock(&lock_path).map_err(|err| {
        if err.exit_code() == mediaops_core::ExitCode::LockConflict {
            AppError::LockConflict(err.to_string())
        } else {
            AppError::Runtime(anyhow::anyhow!("{err}"))
        }
    })?;
    let toml = std::fs::read(&desired_state).map_err(|err| AppError::Runtime(err.into()))?;
    let ds = mediaops_core::DesiredState::from_toml_bytes(&toml)
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let channel = connect_home(&socket, &tls_dir)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let control = ControlPortClient::new(ControlClient::new(channel));
    let api = control
        .edge_apply(&toml)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{}", err.message)))?;
    let mut diffs = Vec::new();
    if !api.diff.is_empty() {
        diffs.push(api.diff);
    }
    let ssh_config = ssh_config.unwrap_or_else(bootstrap::default_ssh_config);
    let bases = ds.edge().map(|e| e.url_bases.clone()).unwrap_or_default();
    for (app, port) in [
        ("sonarr", 8989_u16),
        ("radarr", 7878),
        ("lidarr", 8686),
        ("prowlarr", 9696),
    ] {
        let url_base = bases
            .get(app)
            .map(String::as_str)
            .unwrap_or_else(|| match app {
                "sonarr" => "/sonarr",
                "radarr" => "/radarr",
                "lidarr" => "/lidarr",
                _ => "/prowlarr",
            });
        let desired = desired_nginx_app(url_base, port);
        let remote = format!("/etc/nginx/apps/{app}.conf");
        let diff = write_remote_file(&SystemExec, &ssh_config, &remote, &desired)
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        if !diff.is_empty() {
            diffs.push(diff);
        }
    }
    let check = control
        .edge_check()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{}", err.message)))?;
    if !check.invariant_ok {
        return Err(AppError::DriftVerify(format!(
            "edge check after repair still drifted: {}",
            check.drift
        )));
    }
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    store
        .put_machine(EDGE_FINGERPRINT_KEY, &check.fingerprint)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let data = RepairData {
        noop: diffs.is_empty(),
        diff: diffs.join("\n"),
        fingerprint: check.fingerprint,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!(
            "repair edge fingerprint={} noop={}",
            data.fingerprint, data.noop
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn repair_without_confirm_or_pin_is_refused() {
        let err = repair_edge(true, true, false, None, None, None, None, None, None, None)
            .await
            .expect_err("confirm");
        assert!(
            matches!(err, AppError::Policy(ref m) if m.contains("confirm") || m.contains("pin")),
            "{err}"
        );
    }
}
