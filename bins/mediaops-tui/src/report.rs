//! Read-only sync preview/report overlay.

use mediaops_core::{HomeObject, StatusBody, SyncDisposition, SyncEntry, SyncPhase, SyncStatus};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::sanitize::sanitize;
use crate::view_text::wrap_text;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub request_id: String,
    pub dry_run: bool,
    pub phase: SyncPhase,
    pub generation: i64,
    pub message: String,
    pub copy: usize,
    pub reuse: usize,
    pub present: usize,
    pub blocked: usize,
    pub ineligible: usize,
    pub rows: Vec<ReportRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportRow {
    pub source: String,
    pub destination: String,
    pub reason: String,
}

impl SyncReport {
    pub fn from_object(request_id: String, dry_run: bool, obj: &HomeObject) -> Self {
        match &obj.status {
            StatusBody::Sync(status) => Self::from_status(request_id, dry_run, status),
            _ => Self {
                request_id,
                dry_run,
                phase: SyncPhase::Failed,
                generation: 0,
                message: "unexpected response".into(),
                copy: 0,
                reuse: 0,
                present: 0,
                blocked: 0,
                ineligible: 0,
                rows: Vec::new(),
            },
        }
    }

    fn from_status(request_id: String, dry_run: bool, status: &SyncStatus) -> Self {
        let mut report = Self {
            request_id,
            dry_run,
            phase: status.phase,
            generation: status.list_generation,
            message: status.message.clone(),
            copy: 0,
            reuse: 0,
            present: 0,
            blocked: 0,
            ineligible: 0,
            rows: status.entries.iter().map(report_row).collect(),
        };
        for entry in &status.entries {
            match entry.disposition {
                SyncDisposition::WouldQueue | SyncDisposition::Queued => report.copy += 1,
                SyncDisposition::AlreadyQueued => report.reuse += 1,
                SyncDisposition::Present => report.present += 1,
                SyncDisposition::Blocked => report.blocked += 1,
                SyncDisposition::Ineligible => report.ineligible += 1,
            }
        }
        report
    }

    pub fn lines(&self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let mut lines = Vec::new();
        push_wrapped(&mut lines, &self.header(), width);
        if self.pending() {
            push_wrapped(
                &mut lines,
                &format!("pending  {}", self.phase.as_str()),
                width,
            );
        }
        if self.phase == SyncPhase::Failed {
            let detail = if self.message.is_empty() {
                "failed".into()
            } else {
                format!("failed  {}", self.message)
            };
            push_wrapped(&mut lines, &detail, width);
        }
        push_wrapped(&mut lines, &self.counts_line(), width);
        for row in &self.rows {
            push_wrapped(&mut lines, &row.source, width);
            if !row.destination.is_empty() {
                push_indented(&mut lines, &row.destination, width);
            }
            if !row.reason.is_empty() {
                push_indented(&mut lines, &row.reason, width);
            }
        }
        lines
    }

    pub fn max_offset(&self, width: usize, visible: usize) -> u16 {
        let n = self.lines(width).len();
        let max = n.saturating_sub(visible.max(1));
        u16::try_from(max).unwrap_or(u16::MAX)
    }

    fn pending(&self) -> bool {
        matches!(self.phase, SyncPhase::WaitingInventory)
            || (self.phase == SyncPhase::Captured && !self.dry_run)
    }

    fn header(&self) -> String {
        let kind = if self.dry_run { "preview" } else { "sync" };
        format!(
            "{kind}  {}  generation {}",
            self.request_id, self.generation
        )
    }

    fn counts_line(&self) -> String {
        format!(
            "copy {}  reuse {}  present {}  blocked {}  ineligible {}",
            self.copy, self.reuse, self.present, self.blocked, self.ineligible
        )
    }
}

fn report_row(entry: &SyncEntry) -> ReportRow {
    let destination = entry
        .job
        .as_ref()
        .map(|job| job.dest_rel.clone())
        .filter(|dest| !dest.is_empty())
        .unwrap_or_default();
    let reason = if entry.reason.is_empty() {
        entry.disposition.as_str().to_string()
    } else {
        entry.reason.clone()
    };
    ReportRow {
        source: format!("{} / {}", entry.remote_root, entry.remote_path),
        destination,
        reason,
    }
}

fn push_wrapped(lines: &mut Vec<String>, text: &str, width: usize) {
    for part in wrap_text(&sanitize(text), width) {
        lines.push(part);
    }
}

fn push_indented(lines: &mut Vec<String>, text: &str, width: usize) {
    let indent = " ".repeat(width.saturating_sub(1).min(2));
    for part in wrap_text(&sanitize(text), width.saturating_sub(indent.len()).max(1)) {
        lines.push(format!("{indent}{part}"));
    }
}

pub(crate) fn render_report(frame: &mut Frame<'_>, area: Rect, report: &SyncReport, offset: u16) {
    let width = usize::from(area.width);
    let lines = report.lines(width);
    let visible = usize::from(area.height).max(1);
    let skip = usize::from(offset).min(report.max_offset(width, visible) as usize);
    let visible: Vec<Line<'_>> = lines
        .into_iter()
        .skip(skip)
        .map(|line| Line::from(sanitize(&line)))
        .collect();
    frame.render_widget(Paragraph::new(visible), area);
}

#[cfg(test)]
mod tests {
    use mediaops_core::{JobSpec, Kind, Placement, Spec, SyncSpec};
    use unicode_width::UnicodeWidthStr;

    use super::*;

    fn sample(phase: SyncPhase) -> HomeObject {
        HomeObject::new(
            Kind::Sync,
            "sync-1",
            Spec::Sync(SyncSpec::default()),
            StatusBody::Sync(SyncStatus {
                phase,
                list_generation: 4,
                entries: vec![SyncEntry {
                    remote_root: "seedbox".into(),
                    remote_path: "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
                    title_id: "movie:key:the.matrix.1999".into(),
                    placement: Some(Placement::movie("Other.Title", 2001, "mkv")),
                    job: Some(JobSpec {
                        dest_rel: "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
                        ..JobSpec::default()
                    }),
                    disposition: SyncDisposition::WouldQueue,
                    ..SyncEntry::default()
                }],
                ..SyncStatus::default()
            }),
        )
    }

    #[test]
    fn preview_report_counts_source_and_destination() {
        let report = SyncReport::from_object("sync-1".into(), true, &sample(SyncPhase::Scheduled));
        assert!(report.dry_run);
        assert_eq!(report.copy, 1);
        assert_eq!(report.generation, 4);
        let text = report.lines(80).join("\n");
        assert!(text.contains("preview"), "{text}");
        assert!(
            text.contains("seedbox / movies/The.Matrix.(1999)/The.Matrix.(1999).mkv"),
            "{text}"
        );
        assert!(
            text.contains("movies/The.Matrix.(1999)/The.Matrix.(1999).mkv"),
            "{text}"
        );
        assert!(!text.contains("Other.Title"), "{text}");
        assert!(!text.to_ascii_lowercase().contains("scheduled"), "{text}");
    }

    #[test]
    fn captured_dry_run_is_complete_preview() {
        let report = SyncReport::from_object("sync-1".into(), true, &sample(SyncPhase::Captured));
        let text = report.lines(80).join("\n");
        assert!(!text.contains("pending"), "{text}");
        let live = SyncReport::from_object("sync-1".into(), false, &sample(SyncPhase::Captured));
        assert!(live.lines(80).join("\n").contains("pending"));
    }

    #[test]
    fn header_counts_and_source_wrap_to_narrow_width() {
        let report = SyncReport::from_object("sync-1".into(), true, &sample(SyncPhase::Captured));
        let lines = report.lines(20);
        assert!(lines.len() > 3, "{lines:?}");
        assert!(lines.iter().all(|line| line.width() <= 20), "{lines:?}");
    }

    #[test]
    fn max_offset_clamps_end_to_last_visible_page() {
        let report = SyncReport::from_object("sync-1".into(), true, &sample(SyncPhase::Captured));
        let lines = report.lines(40);
        let max = report.max_offset(40, 2);
        assert_eq!(usize::from(max), lines.len().saturating_sub(2));
        assert!(max < u16::MAX);
    }
}
