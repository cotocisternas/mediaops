use super::*;
use crate::api_cmd::Output;
use crate::sync_cmd::render_sync;
use mediaops_core::{JobSpec, Kind, Placement, Spec, SyncSpec, WantSpec};

fn entry(disposition: SyncDisposition, reason: &str) -> SyncEntry {
    SyncEntry {
        remote_root: "seedbox".into(),
        remote_path: "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
        file_len: 100,
        title_id: "movie:key:the.matrix.1999".into(),
        placement: Some(Placement::movie("Other.Title", 2001, "mkv")),
        job: Some(JobSpec {
            dest_rel: "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
            ..Default::default()
        }),
        disposition,
        reason: reason.into(),
        job_name: String::new(),
        job_uid: String::new(),
    }
}

fn object(phase: SyncPhase, dry_entries: Vec<SyncEntry>, message: &str) -> HomeObject {
    HomeObject::new(
        Kind::Sync,
        "sync-1",
        Spec::Sync(SyncSpec::default()),
        StatusBody::Sync(SyncStatus {
            phase,
            list_generation: 4,
            entries: dry_entries,
            message: message.into(),
            ..Default::default()
        }),
    )
}

#[test]
fn human_summary_counts_each_source_and_destination() {
    let obj = object(
        SyncPhase::Scheduled,
        vec![
            entry(SyncDisposition::Queued, ""),
            entry(SyncDisposition::AlreadyQueued, "job pull-1"),
            entry(SyncDisposition::Present, ""),
            entry(SyncDisposition::Blocked, "unproved destination"),
            entry(SyncDisposition::Ineligible, "not completed"),
        ],
        "",
    );
    let text = format_human("sync-1", false, &obj).expect("sync status");
    assert!(text.contains("sync"), "{text}");
    assert!(text.contains("sync-1"), "{text}");
    assert!(text.contains("generation 4"), "{text}");
    assert!(
        text.contains("copy 1  reuse 1  present 1  blocked 1  ineligible 1"),
        "{text}"
    );
    assert!(
        text.contains("seedbox / movies/The.Matrix.(1999)/The.Matrix.(1999).mkv"),
        "{text}"
    );
    assert!(text.contains("unproved destination"), "{text}");
    assert!(!text.to_ascii_lowercase().contains("scheduled"), "{text}");
}

#[test]
fn dry_run_never_prints_scheduled_success() {
    let obj = object(
        SyncPhase::Scheduled,
        vec![entry(SyncDisposition::WouldQueue, "")],
        "",
    );
    let text = format_human("sync-1", true, &obj).expect("sync status");
    assert!(text.contains("preview"), "{text}");
    assert!(text.contains("sync-1"), "{text}");
    assert!(text.contains("copy 1"), "{text}");
    assert!(!text.to_ascii_lowercase().contains("scheduled"), "{text}");
    assert!(!text.contains("queued success"), "{text}");
}

#[test]
fn captured_dry_run_preview_is_exact_screen_with_job_dest() {
    let obj = object(
        SyncPhase::Captured,
        vec![entry(SyncDisposition::WouldQueue, "")],
        "",
    );
    let text = format_human("sync-1", true, &obj).expect("preview");
    assert_eq!(
        text,
        concat!(
            "preview   sync-1\n",
            "          preview\n",
            "          generation 4\n",
            "          copy 1  reuse 0  present 0  blocked 0  ineligible 0\n",
            "copy       seedbox / movies/The.Matrix.(1999)/The.Matrix.(1999).mkv\n",
            "          movies/The.Matrix.(1999)/The.Matrix.(1999).mkv"
        )
    );
    assert!(!text.contains("pending"), "{text}");
    assert!(!text.contains("Other.Title"), "{text}");
}

#[test]
fn captured_live_request_stays_pending() {
    let obj = object(SyncPhase::Captured, Vec::new(), "");
    let text = format_human("sync-1", false, &obj).expect("sync status");
    assert!(text.contains("pending"), "{text}");
    assert!(text.contains("captured"), "{text}");
}

#[test]
fn pending_phase_is_explicit_not_success() {
    let obj = object(SyncPhase::WaitingInventory, Vec::new(), "");
    let text = format_human("sync-1", false, &obj).expect("sync status");
    assert!(text.contains("pending"), "{text}");
    assert!(!text.contains("planned"), "{text}");
}

#[test]
fn failed_phase_prints_error_summary() {
    let obj = object(SyncPhase::Failed, Vec::new(), "inventory stale");
    let text = format_human("sync-1", false, &obj).expect("sync status");
    assert!(text.contains("failed  inventory stale"), "{text}");
}

#[test]
fn invalid_status_does_not_print_success() {
    let obj = HomeObject::new(
        Kind::Want,
        "movie:tmdb:1",
        Spec::Want(WantSpec {
            title_id: "movie:tmdb:1".into(),
        }),
        StatusBody::empty(Kind::Want),
    );
    assert!(format_human("sync-1", true, &obj).is_none());
    assert!(render_sync("sync-1", true, &obj, Output::Table).is_err());
    assert!(render_sync("sync-1", true, &obj, Output::Json).is_err());
}

#[test]
fn json_modes_match_home_object_contracts() {
    let obj = object(
        SyncPhase::Scheduled,
        vec![entry(SyncDisposition::WouldQueue, "")],
        "",
    );
    let raw: serde_json::Value =
        serde_json::from_str(&render_sync("sync-1", true, &obj, Output::Json).expect("raw"))
            .expect("json");
    assert_eq!(raw["kind"], "Sync");
    assert!(raw.get("ok").is_none(), "{raw}");
    let envelope: serde_json::Value = serde_json::from_str(
        &render_sync("sync-1", true, &obj, Output::LegacyJson).expect("envelope json"),
    )
    .expect("envelope");
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["data"]["kind"], "Sync");
    assert_eq!(envelope.get("error"), Some(&serde_json::Value::Null));
}
