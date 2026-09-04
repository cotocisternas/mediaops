use std::path::{Path, PathBuf};

use mediaops_core::{
    ControlPort, DesiredState, Envelope, JobKind, JobState, ReclaimCandidate, TitleId,
    TitleIndexEntry, WantState, free_bytes, parse_placement, reclaim_preview,
};
use mediaops_proto::ControlPortClient;
use mediaops_proto::control_client::ControlClient;
use mediaops_store::Store;
use mediaops_sync::scan_schema_files;
use mediaops_transfer::{connect_home, list_entries};
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;

#[derive(Debug, Serialize)]
struct WhyData {
    title_id: String,
    grab: Option<GrabView>,
    import: Option<ImportView>,
    hold: Option<HoldView>,
    want: Option<JobView>,
    library: Option<LibraryView>,
    pull: Option<JobView>,
    encode: Option<JobView>,
    watermark: WatermarkView,
    df: Option<DfView>,
    reclaim: Option<ReclaimView>,
    lock: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct JobView {
    job_id: i64,
    title_id: String,
    kind: String,
    state: String,
}

#[derive(Debug, Serialize)]
struct LibraryView {
    path: String,
    install_b3: String,
    current_b3: String,
    present: bool,
}

#[derive(Debug, Serialize)]
struct WatermarkView {
    free: Option<u64>,
    min_free: Option<u64>,
}

#[derive(Debug, Serialize)]
struct StatusData {
    lock: Option<serde_json::Value>,
    open_wants: Vec<JobView>,
    in_flight: Vec<JobView>,
    last_plan: Option<String>,
    watermark: WatermarkView,
    df: Option<DfView>,
}

/// Present when there is something to say about the grabber. `grab: null` means
/// the grabber answered and is not looking for this title.
#[derive(Debug, Serialize)]
struct GrabView {
    /// `null` when the wanted/missing snapshot could not be read -- see `error`.
    wanted_missing: Option<bool>,
    /// Always `null` for now: the *arr download queue has no snapshot RPC, so
    /// this is "unknown", never a claim that the title is not queued.
    queue: Option<bool>,
    /// Why the grabber could not answer. Distinguishes a broken grabber from a
    /// healthy one that simply does not want this title.
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct ImportView {
    path: String,
}

#[derive(Debug, Serialize)]
struct HoldView {
    release_id: String,
    reason: String,
    size: u64,
}

#[derive(Debug, Serialize)]
struct DfView {
    free: u64,
}

#[derive(Debug, Serialize)]
struct ReclaimView {
    candidates: Vec<ReclaimCandidate>,
}

#[allow(clippy::too_many_arguments)]
pub async fn why(
    json: bool,
    title: String,
    state_db: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    library_root: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    let title_id = TitleId::parse(&title).map_err(|err| AppError::Usage(err.to_string()))?;
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let desired_state =
        desired_state.unwrap_or_else(|| bootstrap::default_desired_state(&config_dir));
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let jobs = store
        .list_jobs_by_title(&title_id)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let want = jobs
        .iter()
        .filter(|j| j.kind() == JobKind::Want)
        .filter(|j| matches!(j.state(), JobState::Want(WantState::Open)))
        .max_by_key(|j| j.id().get())
        .or_else(|| {
            jobs.iter()
                .filter(|j| j.kind() == JobKind::Want)
                .max_by_key(|j| j.id().get())
        })
        .map(job_view);
    let pull = jobs
        .iter()
        .filter(|j| j.kind() == JobKind::Pull)
        .max_by_key(|j| j.id().get())
        .map(job_view);
    let encode = jobs
        .iter()
        .filter(|j| j.kind() == JobKind::Encode)
        .max_by_key(|j| j.id().get())
        .map(job_view);

    let library_root = match library_root {
        Some(p) => Some(p),
        None => store
            .get_machine("library_root")
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
            .map(PathBuf::from),
    };
    let library = library_view(&store, &title_id, library_root.as_deref()).await?;
    let watermark = watermark_view(library_root.as_deref(), &desired_state);
    let lock = bootstrap::lock_holder_if_contended(&bootstrap::lock_path(&state_db))
        .map_err(map_bootstrap)?;
    let titles = match store
        .get_title(&title_id)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
    {
        Some(row) => vec![row],
        None => Vec::new(),
    };
    let on_disk = if library.as_ref().is_some_and(|view| view.present) {
        vec![title_id.clone()]
    } else {
        Vec::new()
    };
    let remote = load_remote_why(&socket, &tls_dir, &title_id, &titles, &on_disk).await;

    let data = WhyData {
        title_id: title_id.render(),
        grab: remote.grab,
        import: remote.import,
        hold: remote.hold,
        want,
        library,
        pull,
        encode,
        watermark,
        df: remote.df,
        reclaim: remote.reclaim,
        lock,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!(
            "why {} grab {:?} import {:?} hold {:?} want {:?} pull {:?} encode {:?} free {:?} min_free {:?} df {:?} reclaim {}",
            data.title_id,
            data.grab
                .as_ref()
                .map(|g| match (g.wanted_missing, g.error.as_deref()) {
                    (_, Some(err)) => format!("error {err}"),
                    (Some(true), None) => "wanted_missing".into(),
                    (_, None) => "unknown".to_string(),
                }),
            data.import.as_ref().map(|i| i.path.as_str()),
            data.hold.as_ref().map(|h| h.release_id.as_str()),
            data.want.as_ref().map(|j| j.state.as_str()),
            data.pull.as_ref().map(|j| j.state.as_str()),
            data.encode.as_ref().map(|j| j.state.as_str()),
            data.watermark.free,
            data.watermark.min_free,
            data.df.as_ref().map(|d| d.free),
            data.reclaim
                .as_ref()
                .map(|r| r.candidates.len())
                .unwrap_or(0)
        ))
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn status(
    json: bool,
    state_db: Option<PathBuf>,
    plans_dir: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    library_root: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let plans_dir = plans_dir.unwrap_or_else(bootstrap::default_plans_dir);
    let desired_state =
        desired_state.unwrap_or_else(|| bootstrap::default_desired_state(&config_dir));
    let lock = bootstrap::lock_holder_if_contended(&bootstrap::lock_path(&state_db))
        .map_err(map_bootstrap)?;
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let jobs = store
        .list_jobs()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let open_wants = jobs
        .iter()
        .filter(|j| matches!(j.state(), JobState::Want(WantState::Open)))
        .map(job_view)
        .collect();
    let in_flight = jobs
        .iter()
        .filter(|j| is_in_flight(j.state()))
        .map(job_view)
        .collect();
    let last_plan = latest_plan_name(&plans_dir);
    let library_root = match library_root {
        Some(p) => Some(p),
        None => store
            .get_machine("library_root")
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
            .map(PathBuf::from),
    };
    let watermark = watermark_view(library_root.as_deref(), &desired_state);
    let df = load_remote_df(&socket, &tls_dir).await;
    let data = StatusData {
        lock,
        open_wants,
        in_flight,
        last_plan,
        watermark,
        df,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!(
            "status wants {} in_flight {} plan {:?} df {:?}",
            data.open_wants.len(),
            data.in_flight.len(),
            data.last_plan,
            data.df.as_ref().map(|d| d.free),
        ))
    }
}

struct RemoteWhy {
    grab: Option<GrabView>,
    import: Option<ImportView>,
    hold: Option<HoldView>,
    df: Option<DfView>,
    reclaim: Option<ReclaimView>,
}

async fn load_remote_why(
    socket: &Path,
    tls_dir: &Path,
    title: &TitleId,
    titles: &[TitleIndexEntry],
    on_disk: &[TitleId],
) -> RemoteWhy {
    let Ok(channel) = connect_home(socket, tls_dir).await else {
        return RemoteWhy {
            grab: None,
            import: None,
            hold: None,
            df: None,
            reclaim: None,
        };
    };
    let control = ControlPortClient::new(ControlClient::new(channel.clone()));
    let df = control.df().await.ok().map(|snap| DfView {
        free: snap.free.get(),
    });
    let wanted = control.wanted_missing().await;
    let holds = control.hold_list().await.ok();
    let listings = list_entries(channel).await.ok();
    let torrents = control.guard_preview().await.ok();
    let reclaim = match (listings.as_ref(), torrents.as_ref()) {
        (Some(entries), Some(items)) => {
            let mut candidates = reclaim_preview(entries, titles, on_disk, items);
            candidates.retain(|c| c.title_id == *title);
            Some(ReclaimView { candidates })
        }
        _ => None,
    };
    let grab = match wanted {
        Ok(ids) if ids.iter().any(|id| id == title) => Some(GrabView {
            wanted_missing: Some(true),
            queue: None,
            error: None,
        }),
        Ok(_) => None,
        Err(err) => Some(GrabView {
            wanted_missing: None,
            queue: None,
            error: Some(err.message),
        }),
    };
    let hold_item = holds
        .as_ref()
        .and_then(|items| items.iter().find(|item| item.key.title_id == *title));
    let import = listings.as_ref().and_then(|entries| {
        entries.iter().find_map(|entry| {
            let Ok((id, _)) = parse_placement(entry.r#ref().rel_path()) else {
                return None;
            };
            (id == *title).then(|| ImportView {
                path: entry.r#ref().rel_path().display().to_string(),
            })
        })
    });
    let hold = hold_item.map(|item| HoldView {
        release_id: item.key.release_id.as_str().to_string(),
        reason: item.reason.clone(),
        size: item.size,
    });
    RemoteWhy {
        grab,
        import,
        hold,
        df,
        reclaim,
    }
}

async fn load_remote_df(socket: &Path, tls_dir: &Path) -> Option<DfView> {
    let Ok(channel) = connect_home(socket, tls_dir).await else {
        return None;
    };
    let control = ControlPortClient::new(ControlClient::new(channel));
    control.df().await.ok().map(|snap| DfView {
        free: snap.free.get(),
    })
}

fn job_view(job: &mediaops_core::Job) -> JobView {
    JobView {
        job_id: job.id().get(),
        title_id: job.title_id().render(),
        kind: job.kind().as_str().to_string(),
        state: job.state().as_str().to_string(),
    }
}

fn watermark_view(library_root: Option<&Path>, desired_state: &Path) -> WatermarkView {
    let min_free = std::fs::read_to_string(desired_state)
        .ok()
        .and_then(|t| DesiredState::from_toml(&t).ok())
        .map(|ds| ds.min_free().get());
    let free = library_root.and_then(|root| {
        if !root.exists() {
            return None;
        }
        free_bytes(root).ok()
    });
    WatermarkView { free, min_free }
}

async fn library_view(
    store: &Store,
    title_id: &TitleId,
    library_root: Option<&Path>,
) -> Result<Option<LibraryView>, AppError> {
    let Some(entry) = store
        .get_title(title_id)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
    else {
        return Ok(None);
    };
    let (path, present) = if entry.path_missing() {
        let Some(root) = library_root else {
            return Ok(Some(LibraryView {
                path: String::new(),
                install_b3: entry.install_b3().to_string(),
                current_b3: entry.current_b3().to_string(),
                present: false,
            }));
        };
        let files =
            scan_schema_files(root).map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        match files.into_iter().find(|(id, _, _)| id == title_id) {
            Some((_, rel, _)) => {
                let present = root.join(&rel).is_file();
                (rel.display().to_string(), present)
            }
            None => (String::new(), false),
        }
    } else {
        let path = entry.path().to_string();
        let present = library_root
            .map(|root| root.join(&path).is_file())
            .unwrap_or(false);
        (path, present)
    };
    Ok(Some(LibraryView {
        path,
        install_b3: entry.install_b3().to_string(),
        current_b3: entry.current_b3().to_string(),
        present,
    }))
}

fn is_in_flight(state: JobState) -> bool {
    match state {
        JobState::Want(WantState::Open)
        | JobState::Pull(mediaops_core::PullState::Queued)
        | JobState::Pull(mediaops_core::PullState::Pulling)
        | JobState::Pull(mediaops_core::PullState::Verifying)
        | JobState::Encode(mediaops_core::EncodeState::Queued)
        | JobState::Encode(mediaops_core::EncodeState::Encoding)
        | JobState::Encode(mediaops_core::EncodeState::Replacing)
        | JobState::Hold(mediaops_core::HoldState::Open) => true,
        JobState::Want(_) | JobState::Pull(_) | JobState::Encode(_) | JobState::Hold(_) => false,
    }
}

fn latest_plan_name(plans_dir: &std::path::Path) -> Option<String> {
    let mut names = Vec::new();
    let reader = std::fs::read_dir(plans_dir).ok()?;
    for entry in reader.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".json") {
            names.push(name.into_owned());
        }
    }
    names.sort();
    names.pop()
}

fn map_bootstrap(err: bootstrap::BootstrapError) -> AppError {
    match err.exit_code() {
        mediaops_core::ExitCode::Usage => AppError::Usage(err.to_string()),
        mediaops_core::ExitCode::PolicyRefusal => AppError::Policy(err.to_string()),
        mediaops_core::ExitCode::LockConflict => AppError::LockConflict(err.to_string()),
        _ => AppError::Runtime(anyhow::anyhow!("{err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{Blake3Hex, JobEvent, JobKind, PullEvent, WantEvent};

    #[tokio::test]
    async fn why_invalid_title_is_usage() {
        let err = why(
            true,
            "not-a-title".into(),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect_err("usage");
        assert!(matches!(err, AppError::Usage(_)), "{err}");
    }

    #[tokio::test]
    async fn why_prefers_open_want_and_stats_library_file() {
        let dir = crate::test_support::scratch("why");
        let library = crate::test_support::library_root(&dir);
        let db = dir.join("state.db");
        let store = Store::open(&db).await.expect("store");
        let title = TitleId::movie("603").expect("id");
        let old = store
            .create_job(JobKind::Want, &title, None)
            .await
            .expect("old");
        store
            .advance(old.id(), JobEvent::Want(WantEvent::Satisfy))
            .await
            .expect("satisfy");
        let open = store
            .create_job(JobKind::Want, &title, None)
            .await
            .expect("open");
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
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let json = why(
            true,
            "movie:tmdb:603".into(),
            Some(db.clone()),
            Some(ds.clone()),
            Some(library.clone()),
            Some(dir.clone()),
            None,
            None,
        )
        .await
        .expect("why");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["data"]["want"]["job_id"], open.id().get());
        assert_eq!(value["data"]["want"]["state"], "open");
        assert_eq!(value["data"]["library"]["present"], true);
        assert_eq!(value["data"]["grab"], serde_json::Value::Null);
        assert_eq!(value["data"]["import"], serde_json::Value::Null);
        assert_eq!(value["data"]["hold"], serde_json::Value::Null);
        assert_eq!(value["data"]["df"], serde_json::Value::Null);

        std::fs::remove_file(&movie).expect("unlink");
        let json = why(
            true,
            "movie:tmdb:603".into(),
            Some(db),
            Some(ds),
            Some(library),
            Some(dir.clone()),
            None,
            None,
        )
        .await
        .expect("missing file");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["data"]["library"]["present"], false);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn status_watermark_last_plan_and_in_flight() {
        let dir = crate::test_support::scratch("status");
        let library = crate::test_support::library_root(&dir);
        let db = dir.join("state.db");
        let store = Store::open(&db).await.expect("store");
        store
            .put_machine("library_root", &library.display().to_string())
            .await
            .expect("root");
        let title = TitleId::movie("603").expect("id");
        store
            .create_job(JobKind::Want, &title, None)
            .await
            .expect("want");
        let pull = store
            .create_job(JobKind::Pull, &title, None)
            .await
            .expect("pull");
        store
            .advance(pull.id(), JobEvent::Pull(PullEvent::Start))
            .await
            .expect("start");
        let plans = dir.join("plans");
        std::fs::create_dir_all(&plans).expect("plans");
        std::fs::write(plans.join("aaa.json"), "{}").expect("a");
        std::fs::write(plans.join("zzz.json"), "{}").expect("z");
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let json = status(
            true,
            Some(db),
            Some(plans),
            Some(ds),
            Some(library),
            Some(dir.clone()),
            None,
            None,
        )
        .await
        .expect("status");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(
            value["data"]["open_wants"].as_array().expect("wants").len(),
            1
        );
        assert!(
            value["data"]["in_flight"]
                .as_array()
                .expect("flight")
                .iter()
                .any(|j| j["kind"] == "pull"),
            "{json}"
        );
        assert_eq!(value["data"]["last_plan"], "zzz.json");
        assert_eq!(value["data"]["watermark"]["min_free"], 0);
        assert!(value["data"]["watermark"]["free"].as_u64().is_some());
        assert_eq!(value["data"]["df"], serde_json::Value::Null);
        let _ = std::fs::remove_dir_all(dir);
    }

    struct FakeGrabOps {
        wanted: Vec<TitleId>,
        items: Vec<mediaops_core::HoldLiveItem>,
    }

    impl mediaops_core::GrabOps for FakeGrabOps {
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
            let items = self.items.clone();
            Box::pin(async move { Ok(items) })
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
            let wanted = self.wanted.clone();
            Box::pin(async move { Ok(wanted) })
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
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    #[tokio::test]
    async fn why_chain_sets_grab_import_hold_df_without_replacing_home_watermark() {
        let _g = crate::test_support::serial_net();
        let dir = crate::test_support::scratch("why-chain");
        let library = crate::test_support::library_root(&dir);
        let db = dir.join("state.db");
        let store = Store::open(&db).await.expect("store");
        let title = TitleId::movie("603").expect("id");
        store
            .create_job(JobKind::Want, &title, None)
            .await
            .expect("want");
        let pull = store
            .create_job(JobKind::Pull, &title, None)
            .await
            .expect("pull");
        store
            .advance(pull.id(), JobEvent::Pull(PullEvent::Start))
            .await
            .expect("start");
        store
            .create_job(JobKind::Encode, &title, None)
            .await
            .expect("encode");
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
        let hold = mediaops_core::HoldLiveItem::new(
            mediaops_core::HoldKey::new(
                title.clone(),
                mediaops_core::ReleaseId::parse("deadbeef").expect("id"),
            ),
            1,
            99,
            "blocked",
        );
        let lb = crate::test_support::start_pair_with(
            Some(crate::test_support::MOVIE_REL),
            b"remote",
            mediaops_core::Grabber::Servarr,
            Some(std::sync::Arc::new(FakeGrabOps {
                wanted: vec![title.clone()],
                items: vec![hold],
            })),
        )
        .await;
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let json = why(
            true,
            "movie:tmdb:603".into(),
            Some(db.clone()),
            Some(ds.clone()),
            Some(library.clone()),
            Some(dir.clone()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
        )
        .await
        .expect("why");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["data"]["grab"]["wanted_missing"], true);
        // No queue snapshot RPC exists yet, so the field is "unknown", never a
        // claim that the title is not downloading.
        assert_eq!(value["data"]["grab"]["queue"], serde_json::Value::Null);
        assert_eq!(value["data"]["grab"]["error"], serde_json::Value::Null);
        assert_eq!(
            value["data"]["import"]["path"],
            crate::test_support::MOVIE_REL
        );
        assert_eq!(value["data"]["hold"]["release_id"], "deadbeef");
        assert_eq!(value["data"]["pull"]["state"], "pulling");
        assert_eq!(value["data"]["encode"]["state"], "queued");
        assert_eq!(value["data"]["library"]["present"], true);
        assert!(value["data"]["df"]["free"].as_u64().is_some());
        assert!(value["data"]["watermark"]["free"].as_u64().is_some());
        assert!(
            value["data"]["reclaim"].is_object(),
            "reclaim sibling must be present when UDS is up: {json}"
        );
        let why_cands = value["data"]["reclaim"]["candidates"]
            .as_array()
            .expect("why reclaim candidates");
        assert_eq!(why_cands.len(), 1, "{json}");
        assert_eq!(why_cands[0]["title_id"], "movie:tmdb:603");
        let status_json = status(
            true,
            Some(db),
            Some(dir.join("plans")),
            Some(ds),
            Some(library),
            Some(dir.clone()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
        )
        .await
        .expect("status");
        let status_v: serde_json::Value = serde_json::from_str(&status_json).expect("json");
        assert!(status_v["data"]["df"]["free"].as_u64().is_some());
        assert!(status_v["data"]["watermark"]["free"].as_u64().is_some());
        assert_eq!(
            status_v["data"]["reclaim"],
            serde_json::Value::Null,
            "status keeps df; reclaim ranking is why + reclaim preview: {status_json}"
        );
        drop(lb);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn why_and_status_stay_lock_free_when_flock_is_held() {
        let dir = crate::test_support::scratch("why-lock");
        let db = dir.join("state.db");
        let lock_path = dir.join("mediaops.lock");
        std::fs::write(
            &lock_path,
            r#"{"pid":7,"started_at":1,"command":"mediaops run"}
"#,
        )
        .expect("lock json");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .expect("open");
        fs4::FileExt::try_lock(&file).expect("flock");
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let json = why(
            true,
            "movie:tmdb:603".into(),
            Some(db.clone()),
            Some(ds.clone()),
            None,
            Some(dir.clone()),
            None,
            None,
        )
        .await
        .expect("why");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["lock"]["pid"], 7);
        assert_eq!(value["data"]["df"], serde_json::Value::Null);
        let status_json = status(
            true,
            Some(db),
            Some(dir.join("plans")),
            Some(ds),
            None,
            Some(dir.clone()),
            None,
            None,
        )
        .await
        .expect("status");
        let status_v: serde_json::Value = serde_json::from_str(&status_json).expect("json");
        assert_eq!(status_v["data"]["lock"]["pid"], 7);
        drop(file);
        let _ = std::fs::remove_dir_all(dir);
    }
}
