use std::path::{Path, PathBuf};

use mediaops_core::{
    ControlPort, Envelope, ReclaimCandidate, TitleId, reclaim_preview, reclaim_proved,
};
use mediaops_proto::ControlPortClient;
use mediaops_proto::control_client::ControlClient;
use mediaops_store::Store;
use mediaops_sync::{apply_reclaim, scan_schema_files};
use mediaops_transfer::{HomeChannel, connect_home, list_entries};
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;

#[derive(Debug, Serialize)]
struct PreviewData {
    candidates: Vec<ReclaimCandidate>,
}

#[derive(Debug, Serialize)]
struct ApplyData {
    deleted: usize,
    skipped_seeding: usize,
    failed: usize,
}

#[allow(clippy::too_many_arguments)]
pub async fn preview(
    json: bool,
    state_db: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    library_root: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    let snap = snapshot(
        state_db,
        desired_state,
        library_root,
        socket,
        tls_dir,
        config_dir,
        false,
    )
    .await?;
    let data = PreviewData {
        candidates: snap.candidates,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else if data.candidates.is_empty() {
        Ok("reclaim preview: 0".into())
    } else {
        let mut out = format!("reclaim preview: {}\n", data.candidates.len());
        for c in &data.candidates {
            out.push_str(&format!(
                "{} {} ratio={:?} private={:?}\n",
                c.title_id.render(),
                c.remote.rel_path().display(),
                c.ratio,
                c.is_private
            ));
        }
        Ok(out)
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn apply(
    json: bool,
    state_db: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    library_root: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    let snap = snapshot(
        state_db,
        desired_state,
        library_root,
        socket,
        tls_dir,
        config_dir,
        true,
    )
    .await?;
    let remotes: Vec<_> = snap.candidates.iter().map(|c| c.remote.clone()).collect();
    let control = ControlPortClient::new(ControlClient::new(snap.channel));
    let report = apply_reclaim(&control, &remotes)
        .await
        .map_err(map_control)?;
    let data = ApplyData {
        deleted: report.deleted,
        skipped_seeding: report.skipped_seeding,
        failed: report.failed,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!(
            "reclaim apply deleted {} skipped_seeding {} failed {}",
            data.deleted, data.skipped_seeding, data.failed
        ))
    }
}

struct Snapshot {
    candidates: Vec<ReclaimCandidate>,
    channel: HomeChannel,
    _lock: Option<std::fs::File>,
}

#[allow(clippy::too_many_arguments)]
async fn snapshot(
    state_db: Option<PathBuf>,
    _desired_state: Option<PathBuf>,
    library_root: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    exclusive: bool,
) -> Result<Snapshot, AppError> {
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let lock_path = bootstrap::lock_path(&state_db);
    let lock = if exclusive {
        Some(bootstrap::exclusive_lock(&lock_path).map_err(map_bootstrap)?)
    } else {
        None
    };
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let library_root = match library_root {
        Some(p) => p,
        None => store
            .get_machine("library_root")
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
            .map(PathBuf::from)
            .unwrap_or_default(),
    };
    let title_index = store
        .list_titles()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let on_disk = on_disk_titles(&library_root)?;
    let channel = connect_home(&socket, &tls_dir)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let listings = list_entries(channel.clone())
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let control = ControlPortClient::new(ControlClient::new(channel.clone()));
    let torrents = control.guard_preview().await;
    let candidates = match (exclusive, torrents) {
        (_, Ok(items)) => reclaim_preview(&listings, &title_index, &on_disk, &items),
        (false, Err(_)) => Vec::new(),
        (true, Err(_)) => reclaim_proved(&listings, &title_index, &on_disk),
    };
    Ok(Snapshot {
        candidates,
        channel,
        _lock: lock,
    })
}

pub(crate) fn on_disk_titles(library_root: &Path) -> Result<Vec<TitleId>, AppError> {
    if library_root.as_os_str().is_empty() || !library_root.exists() {
        return Ok(Vec::new());
    }
    Ok(scan_schema_files(library_root)
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
        .into_iter()
        .map(|(id, _, _)| id)
        .collect())
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
    use mediaops_core::{Blake3Hex, TitleId};
    use mediaops_store::Store;

    #[tokio::test]
    async fn preview_is_ranked_and_does_not_unlink() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("reclaim-preview");
        let library = crate::test_support::library_root(&dir);
        let db = dir.join("state.db");
        let store = Store::open(&db).await.expect("store");
        let title = TitleId::movie("603").expect("id");
        store
            .record_install(
                &title,
                &Blake3Hex::of_bytes(b"orig"),
                crate::test_support::MOVIE_REL,
            )
            .await
            .expect("index");
        let movie = library.join(crate::test_support::MOVIE_REL);
        std::fs::create_dir_all(movie.parent().expect("parent")).expect("mkdir");
        std::fs::write(&movie, b"orig").expect("file");
        let lb =
            crate::test_support::start_pair(Some(crate::test_support::MOVIE_REL), b"remote").await;
        let json = preview(
            true,
            Some(db.clone()),
            None,
            Some(library.clone()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
        )
        .await
        .expect("preview");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true, "{json}");
        let cands = value["data"]["candidates"].as_array().expect("cands");
        assert_eq!(cands.len(), 1, "{json}");
        assert_eq!(cands[0]["title_id"], "movie:tmdb:603");
        assert!(
            lb.remote_root
                .join(crate::test_support::MOVIE_REL)
                .is_file(),
            "preview must not unlink"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn apply_unlinks_usenet_after_proof_and_skips_without_digest() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("reclaim-apply");
        let library = crate::test_support::library_root(&dir);
        let db = dir.join("state.db");
        let store = Store::open(&db).await.expect("store");
        let title = TitleId::movie("603").expect("id");
        store
            .record_install(
                &title,
                &Blake3Hex::of_bytes(b"orig"),
                crate::test_support::MOVIE_REL,
            )
            .await
            .expect("index");
        let movie = library.join(crate::test_support::MOVIE_REL);
        std::fs::create_dir_all(movie.parent().expect("parent")).expect("mkdir");
        std::fs::write(&movie, b"orig").expect("file");
        let lb =
            crate::test_support::start_pair(Some(crate::test_support::MOVIE_REL), b"remote").await;
        let json = apply(
            true,
            Some(db.clone()),
            None,
            Some(library.clone()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
        )
        .await
        .expect("apply");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["data"]["deleted"], 1, "{json}");
        assert!(
            !lb.remote_root.join(crate::test_support::MOVIE_REL).exists(),
            "usenet with proof must unlink"
        );

        let lb2 =
            crate::test_support::start_pair(Some(crate::test_support::MOVIE_REL), b"remote").await;
        let db2 = dir.join("empty.db");
        let json = apply(
            true,
            Some(db2),
            None,
            Some(library),
            Some(lb2.sock.clone()),
            Some(lb2.tls_dir.clone()),
            Some(dir.clone()),
        )
        .await
        .expect("no digest");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["data"]["deleted"], 0, "{json}");
        assert!(
            lb2.remote_root
                .join(crate::test_support::MOVIE_REL)
                .is_file(),
            "no install_b3 means no delete"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    struct QbitDownOps;

    impl mediaops_core::GrabOps for QbitDownOps {
        fn grab_apply<'a>(
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
        ) -> mediaops_core::BoxFuture<'_, Result<Vec<TitleId>, mediaops_core::ControlError>>
        {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn unmonitor<'a>(
            &'a self,
            _: &'a TitleId,
        ) -> mediaops_core::BoxFuture<'a, Result<(), mediaops_core::ControlError>> {
            Box::pin(async { Ok(()) })
        }
        fn qbit_snapshot(
            &self,
        ) -> mediaops_core::BoxFuture<
            '_,
            Result<Vec<mediaops_core::GuardPreviewItem>, mediaops_core::ControlError>,
        > {
            Box::pin(async { Err(mediaops_core::ControlError::runtime("qbit down")) })
        }
    }

    #[tokio::test]
    async fn apply_with_qbit_down_still_hits_delete_remote_as_skipped_seeding() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("reclaim-qbit-down");
        let library = crate::test_support::library_root(&dir);
        let db = dir.join("state.db");
        let store = Store::open(&db).await.expect("store");
        let title = TitleId::movie("603").expect("id");
        store
            .record_install(
                &title,
                &Blake3Hex::of_bytes(b"orig"),
                crate::test_support::MOVIE_REL,
            )
            .await
            .expect("index");
        let movie = library.join(crate::test_support::MOVIE_REL);
        std::fs::create_dir_all(movie.parent().expect("parent")).expect("mkdir");
        std::fs::write(&movie, b"orig").expect("file");
        let lb = crate::test_support::start_pair_with(
            Some(crate::test_support::MOVIE_REL),
            b"remote",
            mediaops_core::Grabber::None,
            Some(std::sync::Arc::new(QbitDownOps)),
        )
        .await;
        let json = apply(
            true,
            Some(db),
            None,
            Some(library),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            Some(dir.clone()),
        )
        .await
        .expect("apply");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["data"]["deleted"], 0, "{json}");
        assert_eq!(value["data"]["skipped_seeding"], 1, "{json}");
        assert!(
            lb.remote_root
                .join(crate::test_support::MOVIE_REL)
                .is_file(),
            "qBit down must not unlink"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
