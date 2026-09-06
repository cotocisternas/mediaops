use std::path::{Path, PathBuf};

use mediaops_core::{
    ControlPort, Envelope, InstalledFile, ReclaimCandidate, reclaim_preview, reclaim_proved,
};
use mediaops_proto::ControlPortClient;
use mediaops_proto::control_service_client::ControlServiceClient;
use mediaops_sync::{apply_reclaim, scan_schema_files};
use mediaops_transfer::{HomeChannel, connect_home, list_entries};
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;
use crate::out::{Style, Tone, finish, fmt_bytes, human_from_path, human_title_id, indent, row};

#[derive(Debug, Serialize)]
struct PreviewData {
    candidates: Vec<ReclaimCandidate>,
}

#[derive(Debug, Serialize)]
struct ApplyData {
    deleted: usize,
    skipped_seeding: usize,
    /// qBittorrent is configured but did not answer; nothing was unlinked.
    qbit_unavailable: usize,
    failed: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub async fn preview(
    json: bool,
    state_db: Option<PathBuf>,
    library_root: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    let snap = snapshot(
        state_db,
        library_root,
        socket,
        tls_dir,
        config_dir,
        false,
        None,
        None,
    )
    .await?;
    let data = PreviewData {
        candidates: snap.candidates,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format_preview(&data.candidates))
    }
}

fn format_preview(candidates: &[ReclaimCandidate]) -> String {
    if candidates.is_empty() {
        return "nothing to reclaim".into();
    }
    let style = Style::stdout();
    let mut lines = Vec::new();
    for candidate in candidates {
        let path = candidate.remote.rel_path().display().to_string();
        let title = human_from_path(&path).unwrap_or_else(|| human_title_id(&candidate.title_id));
        lines.push(row(
            style,
            "reclaim",
            Tone::Wait,
            &title,
            &fmt_bytes(candidate.len),
        ));
        lines.push(indent(
            style,
            &format!("{} / {path}", candidate.remote.root_id()),
        ));
        let mut bits = Vec::new();
        if let Some(ratio) = candidate.ratio {
            bits.push(format!("ratio {ratio:.1}"));
        }
        if let Some(private) = candidate.is_private {
            bits.push(if private { "private" } else { "public" }.into());
        }
        if !bits.is_empty() {
            lines.push(indent(style, &bits.join("  ")));
        }
    }
    finish(lines)
}

#[allow(clippy::too_many_arguments)]
pub async fn apply(
    json: bool,
    state_db: Option<PathBuf>,
    library_root: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    max: Option<usize>,
) -> Result<String, AppError> {
    let snap = snapshot(
        state_db,
        library_root,
        socket,
        tls_dir,
        config_dir,
        true,
        max,
        None,
    )
    .await?;
    apply_snapshot(json, snap).await
}

async fn apply_snapshot(json: bool, snap: Snapshot) -> Result<String, AppError> {
    let remotes: Vec<_> = snap.candidates.iter().map(|c| c.remote.clone()).collect();
    let control = ControlPortClient::new(ControlServiceClient::new(snap.channel));
    let report = apply_reclaim(&control, &remotes)
        .await
        .map_err(map_control)?;
    let data = ApplyData {
        deleted: report.deleted,
        skipped_seeding: report.skipped_seeding,
        qbit_unavailable: report.qbit_unavailable,
        failed: report.failed,
        errors: report.errors,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format_apply(&data))
    }
}

fn format_apply(data: &ApplyData) -> String {
    let style = Style::stdout();
    let mut lines = Vec::new();
    if data.deleted > 0 {
        lines.push(row(
            style,
            "deleted",
            Tone::Go,
            "",
            &data.deleted.to_string(),
        ));
    }
    if data.skipped_seeding > 0 {
        lines.push(row(
            style,
            "kept",
            Tone::Quiet,
            "still seeding",
            &data.skipped_seeding.to_string(),
        ));
    }
    if data.qbit_unavailable > 0 {
        lines.push(row(style, "kept", Tone::Wait, "qbit did not answer", ""));
    }
    if data.failed > 0 {
        lines.push(row(
            style,
            "failed",
            Tone::Bad,
            "",
            &data.failed.to_string(),
        ));
    }
    for err in &data.errors {
        lines.push(indent(style, err));
    }
    if lines.is_empty() {
        return "nothing to reclaim".into();
    }
    finish(lines)
}

struct Snapshot {
    candidates: Vec<ReclaimCandidate>,
    channel: HomeChannel,
    _lock: Option<std::fs::File>,
}

#[allow(clippy::too_many_arguments)]
async fn snapshot(
    state_db: Option<PathBuf>,
    library_root: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    exclusive: bool,
    max: Option<usize>,
    home: Option<&crate::home_library::HomeLibrary>,
) -> Result<Snapshot, AppError> {
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let state_db = crate::home_library::state_db_path(state_db);
    let lock_path = bootstrap::lock_path(&state_db);
    let lock = if exclusive {
        Some(bootstrap::exclusive_lock(&lock_path).map_err(map_bootstrap)?)
    } else {
        None
    };
    let (library_root, title_index, root_kinds) = {
        let loaded;
        let home = match home {
            Some(home) => home,
            None => {
                loaded = crate::home_library::HomeLibrary::load().await?;
                &loaded
            }
        };
        let root = home.root(library_root)?;
        let rows = home.rows(false).await?;
        // Reclaim removes the remote copy. Check the current bytes immediately
        // before planning deletion, even if the last drift observation is old.
        let mut verified = Vec::new();
        for row in rows {
            let digest = std::fs::File::open(root.join(row.path()))
                .and_then(mediaops_core::Blake3Hex::of_reader);
            if digest
                .as_ref()
                .is_ok_and(|digest| digest == row.current_b3())
            {
                verified.push(row);
            }
        }
        (root, verified, home.root_kinds()?)
    };
    let on_disk = on_disk_files(&library_root)?;
    let channel = connect_home(&socket, &tls_dir)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let listings = list_entries(channel.clone())
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let control = ControlPortClient::new(ControlServiceClient::new(channel.clone()));
    let torrents = control.guard_preview().await;
    let mut candidates = match (exclusive, torrents) {
        (_, Ok(items)) => reclaim_preview(&listings, &root_kinds, &title_index, &on_disk, &items),
        (false, Err(err)) => return Err(map_control(err)),
        // Seedbox DeleteRemote re-queries qBit and fail-closes; do not
        // pretend we ranked an empty torrent list.
        (true, Err(_)) => reclaim_proved(&listings, &root_kinds, &title_index, &on_disk),
    };
    if let Some(max) = max {
        candidates.truncate(max);
    }
    Ok(Snapshot {
        candidates,
        channel,
        _lock: lock,
    })
}

pub(crate) fn on_disk_files(library_root: &Path) -> Result<Vec<InstalledFile>, AppError> {
    if library_root.as_os_str().is_empty() || !library_root.exists() {
        return Ok(Vec::new());
    }
    scan_schema_files(library_root).map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))
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
    use mediaops_core::TitleId;
    use std::sync::Arc;

    struct TestGrabOps {
        qbit: Result<Vec<mediaops_core::GuardPreviewItem>, mediaops_core::ControlError>,
    }

    impl Default for TestGrabOps {
        fn default() -> Self {
            Self {
                qbit: Ok(Vec::new()),
            }
        }
    }

    impl mediaops_core::GrabOps for TestGrabOps {
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
            let qbit = self.qbit.clone();
            Box::pin(async move { qbit })
        }
    }

    struct Fixture {
        home: crate::home_library::HomeLibrary,
        dir: PathBuf,
        server: tokio::task::JoinHandle<()>,
    }

    impl Fixture {
        async fn new(tag: &str, proved: bool) -> Self {
            let (home, dir, server) = crate::home_library::test_home(tag).await;
            let path = home
                .root(None)
                .unwrap()
                .join(crate::test_support::MOVIE_REL);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"orig").unwrap();
            if proved {
                home.reindex(&crate::progress::OperationProgress::new(false, "reindex"))
                    .await
                    .expect("Home proof");
            }
            Self { home, dir, server }
        }

        async fn snapshot(
            &self,
            lb: &crate::test_support::Loopback,
            exclusive: bool,
            max: Option<usize>,
        ) -> Result<Snapshot, AppError> {
            snapshot(
                Some(self.dir.join("capabilities.db")),
                None,
                Some(lb.sock.clone()),
                Some(lb.tls_dir.clone()),
                None,
                exclusive,
                max,
                Some(&self.home),
            )
            .await
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.abort();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    async fn remote(qbit_down: bool) -> crate::test_support::Loopback {
        let ops = if qbit_down {
            TestGrabOps {
                qbit: Err(mediaops_core::ControlError::runtime("qbit down")),
            }
        } else {
            TestGrabOps::default()
        };
        crate::test_support::start_pair_with(
            Some(crate::test_support::MOVIE_REL),
            b"remote",
            if qbit_down {
                mediaops_core::Grabber::Servarr
            } else {
                mediaops_core::Grabber::None
            },
            Some(Arc::new(ops)),
        )
        .await
    }

    #[tokio::test]
    async fn preview_is_ranked_and_does_not_unlink() {
        let _g = crate::test_support::serial_net();
        let fixture = Fixture::new("reclaim-preview", true).await;
        let lb = remote(false).await;
        let snap = fixture.snapshot(&lb, false, None).await.expect("preview");
        assert_eq!(snap.candidates.len(), 1);
        assert_eq!(
            snap.candidates[0].title_id.render(),
            "movie:key:thematrix.1999"
        );
        assert!(
            lb.remote_root
                .join(crate::test_support::MOVIE_REL)
                .is_file()
        );
    }

    #[tokio::test]
    async fn apply_unlinks_usenet_after_proof_and_skips_without_digest() {
        let _g = crate::test_support::serial_net();
        for proved in [true, false] {
            let fixture = Fixture::new("reclaim-proof", proved).await;
            let lb = remote(false).await;
            let snap = fixture.snapshot(&lb, true, None).await.expect("snapshot");
            let json = apply_snapshot(true, snap).await.expect("apply");
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(value["data"]["deleted"], usize::from(proved), "{json}");
            assert_eq!(
                lb.remote_root
                    .join(crate::test_support::MOVIE_REL)
                    .is_file(),
                !proved
            );
        }
    }

    #[tokio::test]
    async fn changed_home_file_is_not_reclaim_proof() {
        let _g = crate::test_support::serial_net();
        let fixture = Fixture::new("reclaim-changed", true).await;
        std::fs::write(
            fixture
                .home
                .root(None)
                .unwrap()
                .join(crate::test_support::MOVIE_REL),
            b"changed",
        )
        .unwrap();
        let lb = remote(false).await;
        let snap = fixture.snapshot(&lb, true, None).await.expect("snapshot");
        assert!(snap.candidates.is_empty());
        let _ = apply_snapshot(true, snap).await.expect("apply");
        assert!(
            lb.remote_root
                .join(crate::test_support::MOVIE_REL)
                .is_file()
        );
    }

    #[tokio::test]
    async fn apply_with_qbit_down_still_hits_delete_remote_as_skipped_seeding() {
        let _g = crate::test_support::serial_net();
        let fixture = Fixture::new("reclaim-qbit-down", true).await;
        let lb = remote(true).await;
        let snap = fixture.snapshot(&lb, true, None).await.expect("snapshot");
        let json = apply_snapshot(true, snap).await.expect("apply");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["data"]["deleted"], 0, "{json}");
        assert_eq!(value["data"]["qbit_unavailable"], 1, "{json}");
        assert!(
            lb.remote_root
                .join(crate::test_support::MOVIE_REL)
                .is_file()
        );
    }

    #[tokio::test]
    async fn preview_errors_when_qbit_cannot_answer() {
        let _g = crate::test_support::serial_net();
        let fixture = Fixture::new("reclaim-preview-down", true).await;
        let lb = remote(true).await;
        let err = fixture
            .snapshot(&lb, false, None)
            .await
            .err()
            .expect("qBit unavailable");
        assert!(matches!(err, AppError::Runtime(_)), "{err}");
    }

    #[tokio::test]
    async fn apply_max_truncates_ranked_candidates() {
        let _g = crate::test_support::serial_net();
        let fixture = Fixture::new("reclaim-max", true).await;
        let lb = remote(false).await;
        let snap = fixture
            .snapshot(&lb, true, Some(0))
            .await
            .expect("snapshot");
        let json = apply_snapshot(true, snap).await.expect("apply");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["data"]["deleted"], 0, "{json}");
        assert!(
            lb.remote_root
                .join(crate::test_support::MOVIE_REL)
                .is_file()
        );
    }

    #[test]
    fn reclaim_human_screens() {
        assert_eq!(format_preview(&[]), "nothing to reclaim");
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let candidate = ReclaimCandidate {
            title_id: title,
            remote: mediaops_core::RemoteRef::from_wire_parts(
                "seedbox".into(),
                std::path::PathBuf::from(crate::test_support::MOVIE_REL),
            )
            .expect("ref"),
            len: 7_250_189_951,
            mtime: 0,
            ratio: Some(2.1),
            is_private: Some(false),
        };
        assert_eq!(
            format_preview(&[candidate]),
            "\
reclaim   The Matrix (1999)  6.8 GiB
          seedbox / movies/The.Matrix.(1999)/The.Matrix.(1999).mkv
          ratio 2.1  public"
        );
        assert_eq!(
            format_apply(&ApplyData {
                deleted: 2,
                skipped_seeding: 1,
                qbit_unavailable: 0,
                failed: 0,
                errors: Vec::new(),
            }),
            "\
deleted   2
kept      still seeding  1"
        );
    }
}
