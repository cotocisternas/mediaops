//! One-shot Home API sync. No persistent Wants.

use std::path::PathBuf;

use anyhow::anyhow;
use mediaops_core::{HomeObject, StatusBody};
use mediaops_home_client::new_sync_request_id;

use crate::AppError;
use crate::api_cmd::{self, Output};
use crate::sync_format::format_human;

pub async fn run(
    dry_run: bool,
    request_id: Option<String>,
    output: Output,
    socket: Option<PathBuf>,
) -> Result<String, AppError> {
    let request_id = match request_id {
        Some(id) if !id.is_empty() => id,
        _ => new_sync_request_id(),
    };
    let api = match api_cmd::connect(socket, mediaops_core::Actor::Cli).await {
        Ok(api) => api,
        Err(err) => return Err(with_request_id(err, &request_id)),
    };
    match api.sync(&request_id, dry_run).await {
        Ok(obj) => render_sync(&request_id, dry_run, &obj, output),
        Err(err) => Err(with_request_id(api_cmd::map_client(err), &request_id)),
    }
}

pub(crate) fn render_sync(
    request_id: &str,
    dry_run: bool,
    obj: &HomeObject,
    output: Output,
) -> Result<String, AppError> {
    if !matches!(obj.status, StatusBody::Sync(_)) {
        return Err(AppError::Runtime(anyhow!(
            "sync `{request_id}`: unexpected response"
        )));
    }
    match output {
        Output::Json => serde_json::to_string(obj).map_err(|err| AppError::Runtime(err.into())),
        Output::LegacyJson => serde_json::to_string(&mediaops_core::Envelope::ok(obj))
            .map_err(|err| AppError::Runtime(err.into())),
        Output::Table | Output::Wide => format_human(request_id, dry_run, obj)
            .ok_or_else(|| AppError::Runtime(anyhow!("sync `{request_id}`: unexpected response"))),
    }
}

pub(crate) fn with_request_id(err: AppError, request_id: &str) -> AppError {
    match err {
        AppError::Runtime(err) => AppError::Runtime(anyhow!("sync `{request_id}`: {err}")),
        AppError::Usage(message) => AppError::Usage(format!("sync `{request_id}`: {message}")),
        AppError::Policy(message) => AppError::Policy(format!("sync `{request_id}`: {message}")),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_cmd::Output;

    #[test]
    fn transport_wrapper_names_request_id() {
        let err = with_request_id(AppError::Runtime(anyhow!("connect refused")), "sync-9");
        assert!(err.to_string().contains("sync `sync-9`"), "{err}");
        assert!(err.to_string().contains("connect refused"), "{err}");
    }

    #[test]
    fn human_output_makes_controls_inert_json_keeps_them() {
        use crate::sync_format::format_human;
        use mediaops_core::{
            JobSpec, Kind, Spec, StatusBody, SyncDisposition, SyncEntry, SyncPhase, SyncSpec,
            SyncStatus,
        };
        let dirty = "seed\u{1b}[31mbox";
        let obj = HomeObject::new(
            Kind::Sync,
            "id",
            Spec::Sync(SyncSpec::default()),
            StatusBody::Sync(SyncStatus {
                phase: SyncPhase::Captured,
                list_generation: 1,
                entries: vec![SyncEntry {
                    remote_root: dirty.into(),
                    remote_path: "a\nb".into(),
                    job: Some(JobSpec {
                        dest_rel: "d\u{07}est".into(),
                        ..JobSpec::default()
                    }),
                    disposition: SyncDisposition::WouldQueue,
                    reason: "no\u{1b}pe".into(),
                    ..SyncEntry::default()
                }],
                ..SyncStatus::default()
            }),
        );
        let human = format_human("req\u{1b}id", true, &obj).expect("human");
        assert!(!human.contains('\u{1b}'), "{human}");
        assert!(!human.contains('\u{07}'), "{human}");
        let json = render_sync("req\u{1b}id", true, &obj, Output::Json).expect("json");
        assert!(json.contains("[31m"), "{json}");
        assert!(
            json.contains("no\\u001bpe") || json.contains('\u{1b}'),
            "{json}"
        );
    }
}
