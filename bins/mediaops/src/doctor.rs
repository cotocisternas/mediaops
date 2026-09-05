use std::path::{Path, PathBuf};

use mediaops_core::{ControlPort, EdgeApiReport, Envelope, KeyPresence};
use mediaops_proto::ControlPortClient;
use mediaops_proto::control_client::ControlClient;
use mediaops_ssh::is_git_work_tree;
use mediaops_store::Store;
use mediaops_transfer::connect_home;
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;

pub const EDGE_FINGERPRINT_KEY: &str = "edge_fingerprint";

#[derive(Debug, Serialize)]
pub struct DoctorData {
    pub read_only: bool,
    pub invariant_ok: bool,
    pub frozen: bool,
    pub drift: String,
    pub fingerprint: String,
    pub last_repaired: Option<String>,
    pub keys: KeyPresence,
}

pub fn is_frozen(live: &EdgeApiReport, last_repaired: Option<&str>) -> bool {
    if !live.invariant_ok {
        return true;
    }
    match last_repaired {
        Some(last) if !live.fingerprint.is_empty() && last != live.fingerprint => true,
        _ => false,
    }
}

pub fn refuse_pems_in_git_work_tree(dir: &Path) -> Result<(), AppError> {
    if !is_git_work_tree(dir) {
        return Ok(());
    }
    let mut hits = Vec::new();
    walk_pems(dir, 0, &mut hits)?;
    if hits.is_empty() {
        return Ok(());
    }
    Err(AppError::Policy(format!(
        "cert PEMs inside a git work tree: {}",
        hits.iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

fn walk_pems(dir: &Path, depth: u8, hits: &mut Vec<PathBuf>) -> Result<(), AppError> {
    if depth > 4 {
        return Err(AppError::Policy("pem scan truncated".into()));
    }
    let reader = std::fs::read_dir(dir).map_err(|e| AppError::Policy(e.to_string()))?;
    for entry in reader {
        let entry = entry.map_err(|e| AppError::Policy(e.to_string()))?;
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
                continue;
            }
            walk_pems(&path, depth + 1, hits)?;
            continue;
        }
        if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("pem" | "crt" | "key")
        ) {
            hits.push(path);
        }
    }
    Ok(())
}

pub async fn doctor(
    json: bool,
    repair: bool,
    confirm: bool,
    pin: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    state_db: Option<PathBuf>,
) -> Result<String, AppError> {
    if repair && !confirm && pin.as_ref().is_none_or(|p| !pin_ok(p)) {
        return Err(AppError::Policy(
            "doctor --repair unattended from a public laptop is refused; pass --confirm or --pin"
                .into(),
        ));
    }
    if repair {
        return Err(AppError::Policy(
            "doctor is read-only; use `mediaops repair edge --repair --confirm`".into(),
        ));
    }
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    refuse_pems_in_git_work_tree(&config_dir)?;
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    refuse_pems_in_git_work_tree(&tls_dir)?;
    if let Ok(cwd) = std::env::current_dir() {
        refuse_pems_in_git_work_tree(&cwd)?;
    }
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let _ = desired_state;
    let channel = connect_home(&socket, &tls_dir)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let control = ControlPortClient::new(ControlClient::new(channel));
    let edge = control
        .edge_check()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{}", err.message)))?;
    let keys = control
        .key_discovery()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{}", err.message)))?;
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let last = store
        .get_machine(EDGE_FINGERPRINT_KEY)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let frozen = is_frozen(&edge, last.as_deref());
    let data = DoctorData {
        read_only: true,
        invariant_ok: edge.invariant_ok && !frozen,
        frozen,
        drift: edge.drift.clone(),
        fingerprint: edge.fingerprint.clone(),
        last_repaired: last,
        keys,
    };
    if frozen || !edge.invariant_ok {
        return Err(AppError::DriftVerify(format!(
            "EdgeInvariant drift frozen={} {}",
            data.frozen, data.drift
        )));
    }
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok("ok".into())
    }
}

fn pin_ok(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

pub fn confirm_or_pin(confirm: bool, pin: Option<&Path>) -> Result<(), AppError> {
    if confirm {
        return Ok(());
    }
    if pin.is_some_and(pin_ok) {
        return Ok(());
    }
    Err(AppError::Policy(
        "repair requires --repair plus --confirm or --pin".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn doctor_repair_unattended_from_a_public_laptop() {
        let err = doctor(true, true, false, None, None, None, None, None, None)
            .await
            .expect_err("unattended");
        assert!(
            matches!(err, AppError::Policy(ref m) if m.contains("public laptop")),
            "{err}"
        );
    }

    #[test]
    fn cert_pems_inside_a_git_work_tree_are_refused() {
        let dir = crate::test_support::scratch("pem-git");
        std::fs::create_dir_all(dir.join(".git")).expect("git");
        std::fs::create_dir_all(dir.join("tls")).expect("tls");
        std::fs::write(dir.join("tls/ca.pem"), "-----BEGIN CERTIFICATE-----\n").expect("pem");
        let err = refuse_pems_in_git_work_tree(&dir).expect_err("pem");
        assert!(err.to_string().contains("PEM") || err.to_string().contains("pem"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn freeze_when_fingerprint_drifts() {
        let live = EdgeApiReport {
            fingerprint: "aaa".into(),
            invariant_ok: true,
            drift: String::new(),
        };
        assert!(is_frozen(&live, Some("bbb")));
        assert!(!is_frozen(&live, Some("aaa")));
        let bad = EdgeApiReport {
            fingerprint: "aaa".into(),
            invariant_ok: false,
            drift: "bind-to-star".into(),
        };
        assert!(is_frozen(&bad, Some("aaa")));
    }

    #[tokio::test]
    async fn doctor_read_only_ok_through_home_socket() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("doctor-ok");
        let lb = crate::test_support::start_pair(None, b"").await;
        let json = doctor(
            true,
            false,
            false,
            None,
            None,
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect("doctor");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["read_only"], true);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn doctor_reports_drift_when_fingerprint_pinned_stale() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("doctor-drift");
        let lb = crate::test_support::start_pair(None, b"").await;
        let store = crate::test_support::open_store(&dir).await;
        store
            .put_machine(EDGE_FINGERPRINT_KEY, "stale")
            .await
            .expect("pin");
        let err = doctor(
            true,
            false,
            false,
            None,
            None,
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect_err("drift");
        assert!(matches!(err, AppError::DriftVerify(_)), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn doctor_refuses_pem_in_config_git_tree() {
        let dir = crate::test_support::scratch("doctor-pem");
        std::fs::create_dir_all(dir.join(".git")).expect("git");
        std::fs::write(dir.join("ca.pem"), "-----BEGIN CERTIFICATE-----\n").expect("pem");
        let err = doctor(
            true,
            false,
            false,
            None,
            None,
            Some(dir.join("missing.sock")),
            Some(dir.join("tls")),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect_err("pem");
        assert!(matches!(err, AppError::Policy(_)), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
