use std::path::PathBuf;

use mediaops_core::{Envelope, JobKind, JobState, TitleId, WantState};
use mediaops_store::Store;
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;

#[derive(Debug, Serialize)]
struct WatchData {
    title_id: String,
    job_id: i64,
    created: bool,
}

pub async fn watch(
    json: bool,
    title: String,
    state_db: Option<PathBuf>,
) -> Result<String, AppError> {
    let title_id = TitleId::parse(&title).map_err(|err| AppError::Usage(err.to_string()))?;
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let existing = store
        .list_jobs_by_title(&title_id)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let open = existing
        .iter()
        .find(|j| matches!(j.state(), JobState::Want(WantState::Open)));
    let (job, created) = match open {
        Some(job) => (job.clone(), false),
        None => {
            let job = store
                .create_job(JobKind::Want, &title_id, None)
                .await
                .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
            (job, true)
        }
    };
    let data = WatchData {
        title_id: title_id.render(),
        job_id: job.id().get(),
        created,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!(
            "watch {} job {} {}",
            data.title_id,
            data.job_id,
            if data.created { "created" } else { "open" }
        ))
    }
}
