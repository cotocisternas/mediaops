use mediaops_core::SyncPhase;
use mediaops_tui::report::{ReportRow, SyncReport};
use unicode_width::UnicodeWidthStr;

#[test]
fn wrapped_destinations_and_reasons_keep_their_indent() {
    let report = SyncReport {
        request_id: "sync-1".into(),
        dry_run: true,
        phase: SyncPhase::Captured,
        generation: 1,
        message: String::new(),
        copy: 1,
        reuse: 0,
        present: 0,
        blocked: 0,
        ineligible: 0,
        rows: vec![ReportRow {
            source: "source".into(),
            destination: "long destination ".repeat(10),
            reason: "理由が長い 日本語とASCII reason ".repeat(10),
        }],
    };
    let lines = report.lines(60);
    assert_eq!(lines[2], "source");
    assert!(lines[3..].iter().all(|line| line.starts_with("  ")));
    assert!(lines.iter().all(|line| line.width() <= 60));
}
