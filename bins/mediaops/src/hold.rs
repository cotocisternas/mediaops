use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use mediaops_core::{ControlPort, Envelope};
use mediaops_proto::ControlPortClient;
use mediaops_proto::control_client::ControlClient;
use mediaops_store::Store;
use mediaops_sync::inbox;
use mediaops_transfer::connect_home;
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;

#[derive(Debug, Serialize)]
struct HoldJson {
    title_id: String,
    release_id: String,
    age_secs: u64,
    size: u64,
    reason: String,
}

#[derive(Debug, Serialize)]
struct HoldListData {
    holds: Vec<HoldJson>,
}

pub async fn list(
    json: bool,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    state_db: Option<PathBuf>,
) -> Result<String, AppError> {
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let channel = connect_home(&socket, &tls_dir)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let control = ControlPortClient::new(ControlClient::new(channel));
    let live = control.hold_list().await.map_err(map_control)?;
    let decided = store
        .list_decided()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let holds: Vec<HoldJson> = inbox(&live, &decided)
        .into_iter()
        .map(|item| HoldJson {
            title_id: item.key.title_id.render(),
            release_id: item.key.release_id.as_str().to_string(),
            age_secs: item.age_secs(now),
            size: item.size,
            reason: item.reason,
        })
        .collect();
    let data = HoldListData { holds };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else if data.holds.is_empty() {
        Ok("hold list: 0".into())
    } else {
        let mut out = format!("hold list: {}\n", data.holds.len());
        for h in &data.holds {
            out.push_str(&format!(
                "{} {} age={}s size={} {}\n",
                h.title_id, h.release_id, h.age_secs, h.size, h.reason
            ));
        }
        Ok(out)
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
    use mediaops_core::{
        BoxFuture, ControlError, DesiredState, EdgeApiReport, GrabApplyReport, GrabOps, Grabber,
        HoldDecision, HoldKey, HoldLiveItem, HoldsRepo, KeyPresence, ReleaseId, TitleId,
    };
    use mediaops_sync::{SCHEMA_DIRS, ensure_layout};
    use std::sync::Arc;

    struct FakeGrabOps {
        items: Vec<HoldLiveItem>,
    }

    impl GrabOps for FakeGrabOps {
        fn grab_apply<'a>(
            &'a self,
            _: &'a DesiredState,
        ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>> {
            Box::pin(async {
                Ok(GrabApplyReport {
                    noop: true,
                    diff: String::new(),
                })
            })
        }
        fn key_discovery(&self) -> BoxFuture<'_, Result<KeyPresence, ControlError>> {
            Box::pin(async { Ok(KeyPresence::default()) })
        }
        fn edge_api_check(&self) -> BoxFuture<'_, Result<EdgeApiReport, ControlError>> {
            Box::pin(async {
                Ok(EdgeApiReport {
                    fingerprint: String::new(),
                    invariant_ok: true,
                    drift: String::new(),
                })
            })
        }
        fn edge_apply<'a>(
            &'a self,
            _: &'a DesiredState,
        ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>> {
            Box::pin(async {
                Ok(GrabApplyReport {
                    noop: true,
                    diff: String::new(),
                })
            })
        }
        fn hold_list(&self) -> BoxFuture<'_, Result<Vec<HoldLiveItem>, ControlError>> {
            let items = self.items.clone();
            Box::pin(async move { Ok(items) })
        }
    }

    fn live_item(
        title: &str,
        release: &str,
        added_unix: i64,
        size: u64,
        reason: &str,
    ) -> HoldLiveItem {
        HoldLiveItem {
            key: HoldKey::new(
                TitleId::parse(title).expect("title"),
                ReleaseId::parse(release).expect("release"),
            ),
            added_unix,
            size,
            reason: reason.into(),
        }
    }

    #[tokio::test]
    async fn loopback_grabber_none_lists_empty_inbox() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("hold-list-empty");
        let lb = crate::test_support::start_pair(None, b"").await;
        let json = list(
            true,
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect("list");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["holds"], serde_json::json!([]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn decided_key_drops_out_of_inbox() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("hold-list-join");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let decided = live_item(
            "movie:tmdb:603",
            "deadbeef",
            now - 180,
            111,
            "No files found are eligible for import",
        );
        let keep = live_item(
            "series:tvdb:79126",
            "cafebabe",
            now - 90,
            999,
            "Sample file only",
        );
        let lb = crate::test_support::start_pair_with_grab_ops(
            Grabber::Servarr,
            Some(Arc::new(FakeGrabOps {
                items: vec![decided.clone(), keep.clone()],
            })),
        )
        .await;
        let store = crate::test_support::open_store(&dir).await;
        store
            .put(&decided.key, HoldDecision::Rejected)
            .await
            .expect("put");
        let json = list(
            true,
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect("list");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        let holds = value["data"]["holds"].as_array().expect("holds");
        assert_eq!(holds.len(), 1, "{holds:?}");
        assert_eq!(holds[0]["title_id"], "series:tvdb:79126");
        assert_eq!(holds[0]["release_id"], "cafebabe");
        assert_eq!(holds[0]["size"], 999);
        assert_eq!(holds[0]["reason"], "Sample file only");
        let age = holds[0]["age_secs"].as_u64().expect("age");
        assert!((80..=110).contains(&age), "age_secs={age} should be ~90s");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn exclusive_flock_does_not_block_list_or_write_schema() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("hold-list-lockfree");
        let library = dir.join("library");
        ensure_layout(&library).expect("layout");
        let lb = crate::test_support::start_pair(None, b"").await;
        let lock_path = dir.join("mediaops.lock");
        let file = std::fs::File::create(&lock_path).expect("lock file");
        fs4::FileExt::try_lock(&file).expect("hold lock");
        let json = list(
            true,
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
            Some(dir.join("state.db")),
        )
        .await
        .expect("lock-free list");
        drop(file);
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["holds"], serde_json::json!([]));
        for name in SCHEMA_DIRS {
            let dir = library.join(name);
            let empty = std::fs::read_dir(&dir).expect("read").next().is_none();
            assert!(empty, "{name} must stay empty (not a hold folder)");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
