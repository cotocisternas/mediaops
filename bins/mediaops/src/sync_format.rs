use crate::out::{fmt_bytes, inert};
use mediaops_core::{HomeObject, StatusBody, SyncDisposition, SyncEntry, SyncPhase, SyncStatus};

pub(crate) fn format_human(request_id: &str, dry_run: bool, obj: &HomeObject) -> Option<String> {
    let status = match &obj.status {
        StatusBody::Sync(status) => status,
        _ => return None,
    };
    let mut lines = vec![format!("{:<10}{}", verb(dry_run), inert(request_id))];
    lines.push(format!("          {}", phase_line(dry_run, status)));
    if status.list_generation != 0
        || !matches!(
            status.phase,
            SyncPhase::WaitingInventory | SyncPhase::Failed
        )
    {
        lines.push(format!("          generation {}", status.list_generation));
    }
    let counts = Counts::from_entries(&status.entries);
    lines.push(format!(
        "          copy {}  reuse {}  present {}  blocked {}  ineligible {}",
        counts.copy, counts.reuse, counts.present, counts.blocked, counts.ineligible
    ));
    for entry in &status.entries {
        lines.extend(entry_lines(entry));
    }
    match status.phase {
        SyncPhase::WaitingInventory | SyncPhase::Captured if !dry_run => {
            lines.push(format!(
                "inspect   mediaops get Sync {} -o wide",
                crate::api_cmd::shell_arg(request_id)
            ));
        }
        SyncPhase::Scheduled if !dry_run && counts.copy + counts.reuse > 0 => {
            lines.push("progress  mediaops get Job --watch -o wide".into());
        }
        SyncPhase::Scheduled if counts.copy + counts.reuse == 0 => {
            lines.push("          nothing to copy".into());
        }
        SyncPhase::Failed => {
            lines.push("check     mediaops doctor".into());
        }
        _ => {}
    }
    Some(lines.join("\n"))
}

pub(crate) fn summary(status: &SyncStatus) -> String {
    let counts = Counts::from_entries(&status.entries);
    let counts = format!(
        "copy {}  reuse {}  present {}  blocked {}  ineligible {}",
        counts.copy, counts.reuse, counts.present, counts.blocked, counts.ineligible
    );
    if !status.message.is_empty() {
        format!("{counts}  {}", inert(&status.message))
    } else if status.phase == SyncPhase::WaitingInventory {
        "waiting for a fresh completed inventory listing".into()
    } else {
        counts
    }
}

const fn verb(dry_run: bool) -> &'static str {
    if dry_run { "preview" } else { "sync" }
}

fn phase_line(dry_run: bool, status: &SyncStatus) -> String {
    match status.phase {
        SyncPhase::WaitingInventory => {
            "pending  waiting for a fresh completed inventory listing".into()
        }
        SyncPhase::Captured if dry_run => "preview".into(),
        SyncPhase::Captured => "pending  inventory captured; creating copy jobs".into(),
        SyncPhase::Failed if status.message.is_empty() => "failed".into(),
        SyncPhase::Failed => format!("failed  {}", inert(&status.message)),
        SyncPhase::Scheduled if dry_run => "preview".into(),
        SyncPhase::Scheduled => "planned".into(),
    }
}

struct Counts {
    copy: usize,
    reuse: usize,
    present: usize,
    blocked: usize,
    ineligible: usize,
}

impl Counts {
    fn from_entries(entries: &[SyncEntry]) -> Self {
        let mut counts = Self {
            copy: 0,
            reuse: 0,
            present: 0,
            blocked: 0,
            ineligible: 0,
        };
        for entry in entries {
            match entry.disposition {
                SyncDisposition::WouldQueue | SyncDisposition::Queued => counts.copy += 1,
                SyncDisposition::AlreadyQueued => counts.reuse += 1,
                SyncDisposition::Present => counts.present += 1,
                SyncDisposition::Blocked => counts.blocked += 1,
                SyncDisposition::Ineligible => counts.ineligible += 1,
            }
        }
        counts
    }
}

fn entry_lines(entry: &SyncEntry) -> Vec<String> {
    let mut lines = vec![format!(
        "{:<10}{} / {}  {}",
        disposition_verb(entry.disposition),
        inert(&entry.remote_root),
        inert(&entry.remote_path),
        fmt_bytes(entry.file_len),
    )];
    let dest = entry
        .job
        .as_ref()
        .map(|job| inert(&job.dest_rel))
        .filter(|dest| !dest.is_empty())
        .unwrap_or_default();
    if !dest.is_empty() {
        lines.push(format!("          {dest}"));
    }
    if !entry.reason.is_empty() {
        lines.push(format!("          {}", inert(&entry.reason)));
    }
    if !entry.job_name.is_empty() {
        lines.push(format!("          Job {}", inert(&entry.job_name)));
    }
    lines
}

const fn disposition_verb(disposition: SyncDisposition) -> &'static str {
    match disposition {
        SyncDisposition::WouldQueue | SyncDisposition::Queued => "copy",
        SyncDisposition::AlreadyQueued => "reuse",
        SyncDisposition::Present => "present",
        SyncDisposition::Blocked => "blocked",
        SyncDisposition::Ineligible => "ineligible",
    }
}

#[cfg(test)]
#[path = "sync_format_tests.rs"]
mod tests;
