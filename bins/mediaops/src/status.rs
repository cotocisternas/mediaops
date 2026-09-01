use std::path::{Path, PathBuf};

use mediaops_core::{DesiredState, Envelope, JobKind, JobState, TitleId, WantState, free_bytes};
use mediaops_store::Store;
use mediaops_sync::scan_schema_files;
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;

#[derive(Debug, Serialize)]
struct WhyData {
    title_id: String,
    want: Option<JobView>,
    library: Option<LibraryView>,
    pull: Option<JobView>,
    encode: Option<JobView>,
    watermark: WatermarkView,
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
}

pub async fn why(
    json: bool,
    title: String,
    state_db: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    library_root: Option<PathBuf>,
    config_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    let title_id = TitleId::parse(&title).map_err(|err| AppError::Usage(err.to_string()))?;
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
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

    let data = WhyData {
        title_id: title_id.render(),
        want,
        library,
        pull,
        encode,
        watermark,
        lock,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!(
            "why {} want {:?} pull {:?} encode {:?} free {:?} min_free {:?}",
            data.title_id,
            data.want.as_ref().map(|j| j.state.as_str()),
            data.pull.as_ref().map(|j| j.state.as_str()),
            data.encode.as_ref().map(|j| j.state.as_str()),
            data.watermark.free,
            data.watermark.min_free
        ))
    }
}

pub async fn status(
    json: bool,
    state_db: Option<PathBuf>,
    plans_dir: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    library_root: Option<PathBuf>,
    config_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
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
    let data = StatusData {
        lock,
        open_wants,
        in_flight,
        last_plan,
        watermark,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!(
            "status wants {} in_flight {} plan {:?}",
            data.open_wants.len(),
            data.in_flight.len(),
            data.last_plan
        ))
    }
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
        let err = why(true, "not-a-title".into(), None, None, None, None)
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
        )
        .await
        .expect("why");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["data"]["want"]["job_id"], open.id().get());
        assert_eq!(value["data"]["want"]["state"], "open");
        assert_eq!(value["data"]["library"]["present"], true);

        std::fs::remove_file(&movie).expect("unlink");
        let json = why(
            true,
            "movie:tmdb:603".into(),
            Some(db),
            Some(ds),
            Some(library),
            Some(dir.clone()),
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
        let _ = std::fs::remove_dir_all(dir);
    }
}
