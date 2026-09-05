use std::path::{Path, PathBuf};

use mediaops_core::{
    ControlPort, DesiredState, Envelope, Job, JobKind, JobState, Placement, ReclaimCandidate,
    TitleId, TitleIndexEntry, WantState, classify_remote, free_bytes, reclaim_preview,
};
use mediaops_proto::ControlPortClient;
use mediaops_proto::control_service_client::ControlServiceClient;
use mediaops_store::Store;
use mediaops_sync::scan_schema_files;
use mediaops_transfer::{connect_home, list_entries};
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;
use crate::out::{
    Style, TitleHint, Tone, finish, fmt_bytes, hint_from_placement, hints_from_holds,
    hints_from_index, hints_from_jobs, human_from_path, human_title_from_placement, human_title_id,
    lock_command, merge_hints, names_for, placement_from_path, resolve_title, row,
};

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
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let desired_state =
        desired_state.unwrap_or_else(|| bootstrap::default_desired_state(&config_dir));
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let index = store
        .list_titles()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let all_jobs = store
        .list_jobs()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let root_kinds = crate::reclaim::root_kinds_from(&desired_state);
    let title_id =
        resolve_why_title(&title, &index, &all_jobs, &socket, &tls_dir, &root_kinds).await?;
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
    let titles = store
        .get_title(&title_id)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let on_disk = match library_root.as_deref() {
        Some(root) if root.exists() => scan_schema_files(root)
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
            .into_iter()
            .filter(|file| file.title_id == title_id)
            .collect(),
        _ => Vec::new(),
    };
    let remote =
        load_remote_why(&socket, &tls_dir, &title_id, &root_kinds, &titles, &on_disk).await;

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
        let label = why_label(
            &title_id,
            data.library.as_ref(),
            data.import.as_ref(),
            remote.hold_placement.as_ref(),
        );
        Ok(format_why(&data, &label))
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
    let title_index = store
        .list_titles()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let hints = hints_from_index(&title_index);
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
        Ok(format_status(&data, &hints))
    }
}

struct RemoteWhy {
    grab: Option<GrabView>,
    import: Option<ImportView>,
    hold: Option<HoldView>,
    hold_placement: Option<Placement>,
    df: Option<DfView>,
    reclaim: Option<ReclaimView>,
}

async fn load_remote_why(
    socket: &Path,
    tls_dir: &Path,
    title: &TitleId,
    root_kinds: &mediaops_core::RootKinds,
    titles: &[TitleIndexEntry],
    on_disk: &[mediaops_core::InstalledFile],
) -> RemoteWhy {
    let Ok(channel) = connect_home(socket, tls_dir).await else {
        return RemoteWhy {
            grab: None,
            import: None,
            hold: None,
            hold_placement: None,
            df: None,
            reclaim: None,
        };
    };
    let control = ControlPortClient::new(ControlServiceClient::new(channel.clone()));
    let df = control.df().await.ok().map(|snap| DfView {
        free: snap.free.get(),
    });
    let wanted = control.wanted_missing().await;
    let holds = control.hold_list().await.ok();
    let listings = list_entries(channel).await.ok();
    let torrents = control.guard_preview().await.ok();
    let reclaim = match (listings.as_ref(), torrents.as_ref()) {
        (Some(entries), Some(items)) => {
            let mut candidates = reclaim_preview(entries, root_kinds, titles, on_disk, items);
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
            let Ok((id, _)) = mediaops_core::classify_remote(root_kinds, entry) else {
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
        hold_placement: hold_item.and_then(|item| item.placement.clone()),
        df,
        reclaim,
    }
}

async fn load_remote_df(socket: &Path, tls_dir: &Path) -> Option<DfView> {
    let Ok(channel) = connect_home(socket, tls_dir).await else {
        return None;
    };
    let control = ControlPortClient::new(ControlServiceClient::new(channel));
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
    // A show or album is many rows; the first with a path stands for the title.
    let rows = store
        .get_title(title_id)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let Some(entry) = rows
        .iter()
        .find(|row| !row.path_missing())
        .or_else(|| rows.first())
        .cloned()
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
        match files.into_iter().find(|file| &file.title_id == title_id) {
            Some(file) => {
                let present = root.join(&file.path).is_file();
                (file.path, present)
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

async fn resolve_why_title(
    query: &str,
    titles: &[TitleIndexEntry],
    jobs: &[Job],
    socket: &Path,
    tls_dir: &Path,
    root_kinds: &mediaops_core::RootKinds,
) -> Result<TitleId, AppError> {
    if let Ok(id) = TitleId::parse(query) {
        return Ok(id);
    }
    let mut hints = hints_from_index(titles);
    hints.extend(hints_from_jobs(jobs));
    if let Ok(channel) = connect_home(socket, tls_dir).await {
        let control = ControlPortClient::new(ControlServiceClient::new(channel.clone()));
        if let Ok(holds) = control.hold_list().await {
            hints.extend(hints_from_holds(&holds));
        }
        if let Ok(entries) = list_entries(channel).await {
            for entry in &entries {
                if let Ok((id, placement)) = classify_remote(root_kinds, entry) {
                    hints.push(hint_from_placement(id, &placement));
                }
            }
        }
    }
    resolve_title(query, &merge_hints(hints)).map_err(AppError::Usage)
}

fn why_label(
    title_id: &TitleId,
    library: Option<&LibraryView>,
    import: Option<&ImportView>,
    hold_placement: Option<&Placement>,
) -> String {
    if let Some(placement) = hold_placement {
        return human_title_from_placement(placement);
    }
    if let Some(lib) = library
        && let Some(placement) = placement_from_path(&lib.path)
    {
        return human_title_from_placement(&placement);
    }
    if let Some(imp) = import
        && let Some(placement) = placement_from_path(&imp.path)
    {
        return human_title_from_placement(&placement);
    }
    human_title_id(title_id)
}

fn format_why(data: &WhyData, label: &str) -> String {
    let style = Style::stdout();
    let mut lines = vec![style.bold(label), style.dim(&data.title_id), String::new()];
    let mut said = false;
    if let Some(lock) = &data.lock {
        lines.push(row(style, "busy", Tone::Wait, &lock_command(lock), ""));
        said = true;
    }
    if let Some(hold) = &data.hold {
        let meta = if hold.size > 0 {
            fmt_bytes(hold.size)
        } else {
            String::new()
        };
        let reason = if hold.reason.is_empty() {
            "on hold"
        } else {
            hold.reason.as_str()
        };
        lines.push(row(style, "hold", Tone::Wait, reason, &meta));
        said = true;
    }
    if let Some(grab) = &data.grab {
        if let Some(err) = &grab.error {
            lines.push(row(style, "grab", Tone::Bad, err, ""));
            said = true;
        } else if grab.wanted_missing == Some(true) {
            lines.push(row(style, "grab", Tone::Wait, "wanted, not on the box", ""));
            said = true;
        }
    }
    if let Some(import) = &data.import {
        let title = human_from_path(&import.path).unwrap_or_else(|| import.path.clone());
        lines.push(row(style, "import", Tone::Go, &title, ""));
        said = true;
    }
    if let Some(want) = &data.want
        && want.state == "open"
    {
        lines.push(row(style, "want", Tone::Quiet, "open", ""));
        said = true;
    }
    if let Some(pull) = &data.pull {
        lines.push(row(style, "pull", Tone::Go, &pull.state, ""));
        said = true;
    }
    if let Some(encode) = &data.encode {
        lines.push(row(style, "encode", Tone::Go, &encode.state, ""));
        said = true;
    }
    match &data.library {
        Some(lib) if lib.present => {
            let path = if lib.path.is_empty() {
                "here"
            } else {
                lib.path.as_str()
            };
            lines.push(row(style, "library", Tone::Go, path, ""));
            said = true;
        }
        Some(_) => {
            lines.push(row(style, "library", Tone::Quiet, "not here", ""));
        }
        None => {}
    }
    if let Some(n) = data
        .reclaim
        .as_ref()
        .map(|r| r.candidates.len())
        .filter(|n| *n > 0)
    {
        lines.push(row(
            style,
            "reclaim",
            Tone::Wait,
            &format!("{n} leftover on the box"),
            "",
        ));
        said = true;
    }
    if !said {
        lines.push("quiet".into());
    }
    if let Some(line) = watermark_tight(&data.watermark) {
        lines.push(String::new());
        lines.push(row(style, "disk", Tone::Wait, "", &line));
    }
    finish(lines)
}

fn format_status(data: &StatusData, hints: &[TitleHint]) -> String {
    let style = Style::stdout();
    let mut lines = Vec::new();
    if let Some(lock) = &data.lock {
        lines.push(row(style, "busy", Tone::Wait, &lock_command(lock), ""));
    }
    for job in &data.in_flight {
        let title = TitleId::parse(&job.title_id)
            .map(|id| names_for(&id, hints))
            .unwrap_or_else(|_| job.title_id.clone());
        let (verb, tone) = match job.kind.as_str() {
            "pull" => ("pull", Tone::Go),
            "encode" => ("encode", Tone::Go),
            "hold" => ("hold", Tone::Wait),
            "want" => ("want", Tone::Quiet),
            other => (other, Tone::Quiet),
        };
        let meta = if job.kind == "want" || job.state == "open" {
            String::new()
        } else {
            job.state.clone()
        };
        lines.push(row(style, verb, tone, &title, &meta));
    }
    if lines.is_empty() {
        lines.push("nothing happening".into());
    }
    if let Some(disk) = status_disk(&data.watermark) {
        lines.push(String::new());
        lines.push(row(style, "disk", Tone::Quiet, "", &disk));
        if let Some(home) = data.df.as_ref() {
            lines.push(row(
                style,
                "home",
                Tone::Quiet,
                "",
                &format!("{} free", fmt_bytes(home.free)),
            ));
        }
    } else if let Some(home) = data.df.as_ref() {
        lines.push(String::new());
        lines.push(row(
            style,
            "home",
            Tone::Quiet,
            "",
            &format!("{} free", fmt_bytes(home.free)),
        ));
    }
    finish(lines)
}

fn status_disk(w: &WatermarkView) -> Option<String> {
    let free = w.free?;
    let mut s = format!("{} free", fmt_bytes(free));
    if let Some(min) = w.min_free
        && min > 0
        && free < min
    {
        s.push_str(&format!("    need {}", fmt_bytes(min)));
    }
    Some(s)
}

fn watermark_tight(w: &WatermarkView) -> Option<String> {
    let free = w.free?;
    let min = w.min_free.filter(|m| *m > 0)?;
    (free < min).then(|| format!("{} free    need {}", fmt_bytes(free), fmt_bytes(min)))
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
        let dir = crate::test_support::scratch("why-bad-name");
        let db = dir.join("state.db");
        let _store = Store::open(&db).await.expect("store");
        let err = why(
            true,
            "not-a-title".into(),
            Some(db),
            None,
            None,
            Some(dir.clone()),
            None,
            None,
        )
        .await
        .expect_err("usage");
        assert!(matches!(err, AppError::Usage(_)), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn why_accepts_a_human_name() {
        let dir = crate::test_support::scratch("why-name");
        let library = crate::test_support::library_root(&dir);
        let db = dir.join("state.db");
        let store = Store::open(&db).await.expect("store");
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
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
            "The Matrix".into(),
            Some(db),
            Some(ds),
            Some(library),
            Some(dir.clone()),
            None,
            None,
        )
        .await
        .expect("why");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["data"]["title_id"], "movie:key:thematrix.1999");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn why_human_quiet_and_hold_are_exact_screens() {
        let quiet = format_why(
            &WhyData {
                title_id: "series:key:foundation.2021".into(),
                grab: None,
                import: None,
                hold: None,
                want: None,
                library: None,
                pull: None,
                encode: None,
                watermark: WatermarkView {
                    free: Some(744_189_419_520),
                    min_free: Some(274_877_906_944),
                },
                df: None,
                reclaim: None,
                lock: None,
            },
            "Foundation (2021)",
        );
        assert_eq!(
            quiet,
            "\
Foundation (2021)
series:key:foundation.2021

quiet"
        );
        let stuck = format_why(
            &WhyData {
                title_id: "movie:tmdb:4539".into(),
                grab: None,
                import: None,
                hold: Some(HoldView {
                    release_id: "deadbeef".into(),
                    reason: "Manual Import required.".into(),
                    size: 7_588_856_506,
                }),
                want: None,
                library: Some(LibraryView {
                    path: String::new(),
                    install_b3: String::new(),
                    current_b3: String::new(),
                    present: false,
                }),
                pull: None,
                encode: None,
                watermark: WatermarkView {
                    free: Some(744_189_419_520),
                    min_free: Some(274_877_906_944),
                },
                df: None,
                reclaim: None,
                lock: None,
            },
            "Hearts of Darkness (1991)",
        );
        assert_eq!(
            stuck,
            "\
Hearts of Darkness (1991)
movie:tmdb:4539

hold      Manual Import required.  7.1 GiB
library   not here"
        );
        let waiting = format_why(
            &WhyData {
                title_id: "series:key:mrrobot.2015".into(),
                grab: Some(GrabView {
                    wanted_missing: Some(true),
                    queue: None,
                    error: None,
                }),
                import: Some(ImportView {
                    path: "Mr.Robot.(2015)/Season.01/Mr.Robot.(2015).S01E02.eps1.1.mkv".into(),
                }),
                hold: None,
                want: None,
                library: None,
                pull: None,
                encode: None,
                watermark: WatermarkView {
                    free: Some(744_189_419_520),
                    min_free: Some(274_877_906_944),
                },
                df: None,
                reclaim: None,
                lock: None,
            },
            "Mr Robot (2015)",
        );
        assert_eq!(
            waiting,
            "\
Mr Robot (2015)
series:key:mrrobot.2015

grab      wanted, not on the box
import    Mr Robot (2015) S01E02"
        );
    }

    #[test]
    fn status_human_quiet_and_work_are_exact_screens() {
        let quiet = format_status(
            &StatusData {
                lock: None,
                open_wants: Vec::new(),
                in_flight: Vec::new(),
                last_plan: Some("zzz.json".into()),
                watermark: WatermarkView {
                    free: Some(744_189_419_520),
                    min_free: Some(274_877_906_944),
                },
                df: Some(DfView {
                    free: 4_182_917_251_072,
                }),
            },
            &[],
        );
        assert_eq!(
            quiet,
            "\
nothing happening

disk      693.1 GiB free
home      3.8 TiB free"
        );
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let hints = hints_from_index(&[TitleIndexEntry::new(
            title.clone(),
            crate::test_support::MOVIE_REL,
            Blake3Hex::of_bytes(b"a"),
            Blake3Hex::of_bytes(b"a"),
        )]);
        let work = format_status(
            &StatusData {
                lock: None,
                open_wants: vec![JobView {
                    job_id: 1,
                    title_id: title.render(),
                    kind: "want".into(),
                    state: "open".into(),
                }],
                in_flight: vec![
                    JobView {
                        job_id: 1,
                        title_id: title.render(),
                        kind: "want".into(),
                        state: "open".into(),
                    },
                    JobView {
                        job_id: 2,
                        title_id: title.render(),
                        kind: "pull".into(),
                        state: "pulling".into(),
                    },
                ],
                last_plan: None,
                watermark: WatermarkView {
                    free: Some(744_189_419_520),
                    min_free: Some(0),
                },
                df: None,
            },
            &hints,
        );
        assert_eq!(
            work,
            "\
want      The Matrix (1999)
pull      The Matrix (1999)  pulling

disk      693.1 GiB free"
        );
    }

    #[tokio::test]
    async fn why_prefers_open_want_and_stats_library_file() {
        let dir = crate::test_support::scratch("why");
        let library = crate::test_support::library_root(&dir);
        let db = dir.join("state.db");
        let store = Store::open(&db).await.expect("store");
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
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
            "movie:key:thematrix.1999".into(),
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
            "movie:key:thematrix.1999".into(),
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
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
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
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
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
            "movie:key:thematrix.1999".into(),
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
        assert_eq!(why_cands[0]["title_id"], "movie:key:thematrix.1999");
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
            "movie:key:thematrix.1999".into(),
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
