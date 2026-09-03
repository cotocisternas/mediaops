use std::path::PathBuf;

use mediaops_core::{ControlPort, Envelope, GrabApplyReport};
use mediaops_proto::ControlPortClient;
use mediaops_proto::control_client::ControlClient;
use mediaops_store::Store;
use mediaops_transfer::connect_home;
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;

#[derive(Debug, Serialize)]
struct ApplyData {
    noop: bool,
    diff: String,
}

pub async fn seedbox_apply(
    json: bool,
    desired_state: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    state_db: Option<PathBuf>,
) -> Result<String, AppError> {
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let lock_path = bootstrap::lock_path(&state_db);
    let _lock = bootstrap::exclusive_lock(&lock_path).map_err(map_bootstrap)?;
    let desired_state =
        desired_state.unwrap_or_else(|| bootstrap::default_desired_state(&config_dir));
    let toml = std::fs::read(&desired_state).map_err(|err| AppError::Runtime(err.into()))?;
    let _ds = mediaops_core::DesiredState::from_toml_bytes(&toml)
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let channel = connect_home(&socket, &tls_dir)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let control = ControlPortClient::new(ControlClient::new(channel));
    let edge = control.edge_check().await.map_err(map_control)?;
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let last = store
        .get_machine(crate::doctor::EDGE_FINGERPRINT_KEY)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    if crate::doctor::is_frozen(&edge, last.as_deref()) {
        return Err(AppError::Policy(
            "panel fingerprint freeze; run mediaops repair edge --repair --confirm".into(),
        ));
    }
    let report = control.grab_apply(&toml).await.map_err(map_control)?;
    render_apply(json, &report)
}

fn render_apply(json: bool, report: &GrabApplyReport) -> Result<String, AppError> {
    let data = ApplyData {
        noop: report.noop,
        diff: report.diff.clone(),
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else if report.noop {
        Ok("seedbox apply: no-op".into())
    } else {
        Ok(format!("seedbox apply\n{}", report.diff))
    }
}

fn map_bootstrap(err: bootstrap::BootstrapError) -> AppError {
    match err.exit_code() {
        mediaops_core::ExitCode::LockConflict => AppError::LockConflict(err.to_string()),
        mediaops_core::ExitCode::Usage => AppError::Usage(err.to_string()),
        mediaops_core::ExitCode::PolicyRefusal => AppError::Policy(err.to_string()),
        _ => AppError::Runtime(anyhow::anyhow!("{err}")),
    }
}

fn map_control(err: mediaops_core::ControlError) -> AppError {
    match err.exit_code {
        mediaops_core::ExitCode::PolicyRefusal => AppError::Policy(err.message),
        mediaops_core::ExitCode::DriftVerify => AppError::DriftVerify(err.message),
        mediaops_core::ExitCode::LockConflict => AppError::LockConflict(err.message),
        mediaops_core::ExitCode::Usage => AppError::Usage(err.message),
        _ => AppError::Runtime(anyhow::anyhow!("{}", err.message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DiffThenNoop {
        calls: std::sync::atomic::AtomicU32,
    }

    impl mediaops_core::GrabOps for DiffThenNoop {
        fn grab_apply<'a>(
            &'a self,
            _: &'a mediaops_core::DesiredState,
        ) -> mediaops_core::BoxFuture<
            'a,
            Result<mediaops_core::GrabApplyReport, mediaops_core::ControlError>,
        > {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move {
                if n == 0 {
                    Ok(mediaops_core::GrabApplyReport {
                        noop: false,
                        diff: "indexers: +nzbgeek\n".into(),
                    })
                } else {
                    Ok(mediaops_core::GrabApplyReport {
                        noop: true,
                        diff: String::new(),
                    })
                }
            })
        }
        fn key_discovery(
            &self,
        ) -> mediaops_core::BoxFuture<
            '_,
            Result<mediaops_core::KeyPresence, mediaops_core::ControlError>,
        > {
            Box::pin(async { Ok(mediaops_core::KeyPresence::default()) })
        }
        fn edge_api_check(
            &self,
        ) -> mediaops_core::BoxFuture<
            '_,
            Result<mediaops_core::EdgeApiReport, mediaops_core::ControlError>,
        > {
            Box::pin(async {
                Ok(mediaops_core::EdgeApiReport {
                    fingerprint: String::new(),
                    invariant_ok: true,
                    drift: String::new(),
                })
            })
        }
        fn edge_apply<'a>(
            &'a self,
            _: &'a mediaops_core::DesiredState,
        ) -> mediaops_core::BoxFuture<
            'a,
            Result<mediaops_core::GrabApplyReport, mediaops_core::ControlError>,
        > {
            Box::pin(async {
                Ok(mediaops_core::GrabApplyReport {
                    noop: true,
                    diff: String::new(),
                })
            })
        }
        fn hold_list(
            &self,
        ) -> mediaops_core::BoxFuture<
            '_,
            Result<Vec<mediaops_core::HoldLiveItem>, mediaops_core::ControlError>,
        > {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn hold_reject<'a>(
            &'a self,
            _: &'a mediaops_core::HoldKey,
        ) -> mediaops_core::BoxFuture<'a, Result<(), mediaops_core::ControlError>> {
            Box::pin(async { Ok(()) })
        }
        fn wanted_missing(
            &self,
        ) -> mediaops_core::BoxFuture<
            '_,
            Result<Vec<mediaops_core::TitleId>, mediaops_core::ControlError>,
        > {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn unmonitor<'a>(
            &'a self,
            _: &'a mediaops_core::TitleId,
        ) -> mediaops_core::BoxFuture<'a, Result<(), mediaops_core::ControlError>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn seedbox_apply_second_call_is_noop() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("seedbox-apply");
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let lb = crate::test_support::start_pair_with_grab_ops(
            mediaops_core::Grabber::Servarr,
            Some(std::sync::Arc::new(DiffThenNoop {
                calls: std::sync::atomic::AtomicU32::new(0),
            })),
        )
        .await;
        let json = seedbox_apply(
            true,
            Some(ds.clone()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect("first");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["noop"], false, "{json}");
        assert!(
            value["data"]["diff"]
                .as_str()
                .expect("diff")
                .contains("nzbgeek"),
            "{json}"
        );
        let json = seedbox_apply(
            true,
            Some(ds),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect("second");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["data"]["noop"], true, "{json}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn seedbox_apply_freezes_when_fingerprint_drifted() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("apply-freeze");
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let lb = crate::test_support::start_pair(None, b"").await;
        let store = crate::test_support::open_store(&dir).await;
        store
            .put_machine(crate::doctor::EDGE_FINGERPRINT_KEY, "old-fingerprint")
            .await
            .expect("pin");
        let err = seedbox_apply(
            true,
            Some(ds),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect_err("freeze");
        assert!(
            matches!(err, AppError::Policy(ref m) if m.contains("fingerprint freeze")),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
