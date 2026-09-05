use std::path::PathBuf;

use mediaops_core::{ControlPort, Envelope, JobKind, JobState, TitleId, WantState};
use mediaops_proto::ControlPortClient;
use mediaops_proto::control_service_client::ControlServiceClient;
use mediaops_store::Store;
use mediaops_transfer::connect_home;
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;
use crate::out::{
    Style, Tone, finish, hints_from_holds, hints_from_index, hints_from_jobs, human_title_id,
    indent, merge_hints, resolve_title, row,
};

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
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let title_id = resolve_watch_title(&store, &title).await?;
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
        Ok(format_watch(&human_title_id(&title_id), &data))
    }
}

fn format_watch(label: &str, data: &WatchData) -> String {
    let style = Style::stdout();
    let meta = if data.created { "" } else { "already" };
    finish(vec![
        row(style, "watching", Tone::Go, label, meta),
        indent(style, &data.title_id),
    ])
}

async fn resolve_watch_title(store: &Store, query: &str) -> Result<TitleId, AppError> {
    if let Ok(id) = TitleId::parse(query) {
        return Ok(id);
    }
    let titles = store
        .list_titles()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let jobs = store
        .list_jobs()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let mut hints = hints_from_index(&titles);
    hints.extend(hints_from_jobs(&jobs));
    let config_dir = bootstrap::default_config_dir();
    let tls_dir = bootstrap::default_tls_dir(&config_dir);
    let socket = bootstrap::default_socket();
    if let Ok(channel) = connect_home(&socket, &tls_dir).await {
        let control = ControlPortClient::new(ControlServiceClient::new(channel));
        if let Ok(holds) = control.hold_list().await {
            hints.extend(hints_from_holds(&holds));
        }
    }
    resolve_title(query, &merge_hints(hints)).map_err(AppError::Usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_human_is_a_title_and_an_id() {
        assert_eq!(
            format_watch(
                "Foundation (2021)",
                &WatchData {
                    title_id: "series:key:foundation.2021".into(),
                    job_id: 3,
                    created: true,
                }
            ),
            "\
watching  Foundation (2021)
          series:key:foundation.2021"
        );
        assert_eq!(
            format_watch(
                "Foundation (2021)",
                &WatchData {
                    title_id: "series:key:foundation.2021".into(),
                    job_id: 3,
                    created: false,
                }
            ),
            "\
watching  Foundation (2021)  already
          series:key:foundation.2021"
        );
    }
}
