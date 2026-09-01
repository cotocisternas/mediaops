use std::path::PathBuf;

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
    kind: String,
    state: String,
}

#[derive(Debug, Serialize)]
struct LibraryView {
    path: String,
    install_b3: String,
    current_b3: String,
}

#[derive(Debug, Serialize)]
struct WatermarkView {
    free: u64,
    min_free: u64,
}

#[derive(Debug, Serialize)]
struct StatusData {
    lock: Option<serde_json::Value>,
    open_wants: Vec<JobView>,
    in_flight: Vec<JobView>,
    last_plan: Option<String>,
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
        .find(|j| j.kind() == JobKind::Want)
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
    let mut library = None;
    if let Some(entry) = store
        .get_title(&title_id)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
    {
        let path = if entry.path_missing() {
            if let Some(root) = library_root.as_ref() {
                scan_schema_files(root)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|(id, _, _)| id == &title_id)
                    .map(|(_, p, _)| p.display().to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            }
        } else {
            entry.path().to_string()
        };
        library = Some(LibraryView {
            path,
            install_b3: entry.install_b3().to_string(),
            current_b3: entry.current_b3().to_string(),
        });
    }

    let min_free = std::fs::read_to_string(&desired_state)
        .ok()
        .and_then(|t| DesiredState::from_toml(&t).ok())
        .map(|ds| ds.min_free().get())
        .unwrap_or(0);
    let free = library_root
        .as_ref()
        .and_then(|root| free_bytes(root).ok())
        .unwrap_or(0);
    let lock = bootstrap::lock_holder_if_contended(&bootstrap::lock_path(&state_db))
        .map_err(map_bootstrap)?;

    let data = WhyData {
        title_id: title_id.render(),
        want,
        library,
        pull,
        encode,
        watermark: WatermarkView { free, min_free },
        lock,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!(
            "why {} want {:?} pull {:?} encode {:?} free {} min_free {}",
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
) -> Result<String, AppError> {
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let plans_dir = plans_dir.unwrap_or_else(bootstrap::default_plans_dir);
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
    let data = StatusData {
        lock,
        open_wants,
        in_flight,
        last_plan,
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
        kind: job.kind().as_str().to_string(),
        state: job.state().as_str().to_string(),
    }
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
