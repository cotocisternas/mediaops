use mediaops_core::{
    ClusterSpec, ClusterStatus, HomeObject, JobPhase, JobSpec, JobStatus, Kind, NodeSpec,
    NodeStatus, Spec, StatusBody, WorkerKind,
};
use mediaops_tui::keys::Command;
use mediaops_tui::projection::project;
use mediaops_tui::update::{Update, apply};
use mediaops_tui::{Screen, SyncState, UiModel, cache::ObjectCache, disk::DiskObservation};
use ratatui::{Terminal, backend::TestBackend};

fn cache(objects: Vec<HomeObject>) -> ObjectCache {
    let mut cache = ObjectCache::default();
    let epoch = cache.bump_epoch();
    cache.install_baseline(epoch, objects);
    cache
}

fn job(phase: JobPhase) -> HomeObject {
    HomeObject::new(
        Kind::Job,
        "pull-up",
        Spec::Job(JobSpec {
            title_id: "movie:key:up.2009".into(),
            file_len: 4096,
            ..Default::default()
        }),
        StatusBody::Job(JobStatus {
            phase,
            bytes_done: 4096,
            attempts: 1,
            ..Default::default()
        }),
    )
}

fn draw(ui: &UiModel, cache: &ObjectCache, sync: SyncState) -> Vec<String> {
    let projection = project(cache, ui.screen, ui.selected, 100);
    let mut terminal = Terminal::new(TestBackend::new(ui.cols, ui.rows)).unwrap();
    terminal
        .draw(|frame| {
            mediaops_tui::view::render(
                frame,
                ui,
                sync,
                &projection,
                &DiskObservation::unavailable(),
                false,
                false,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..ui.rows)
        .map(|y| {
            (0..ui.cols)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .into()
        })
        .collect()
}

#[test]
fn minimum_job_screen_preserves_title_verification_percent_and_navigation() {
    let ui = UiModel {
        screen: Screen::Jobs,
        cols: 60,
        rows: 16,
        ..Default::default()
    };
    let screen = draw(
        &ui,
        &cache(vec![job(JobPhase::Verifying)]),
        SyncState::Current,
    );
    assert_eq!(
        screen,
        [
            "mediaops  Current  |  disk unavailable",
            "Tab resource  j/k rows  g/G first/last  d detail",
            "1 Overview 2 Wants 3 Jobs 4 Holds 5 Titles 6 Nodes 7 Box",
            "┌ Jobs (1) [list] ─────────────────────────────────────────┐",
            "│  TITLE                       PHASE     PROGRESS BYTES    │",
            "│> Up (2009)                   verifying     100%     4 KiB│",
            "│                                                          │",
            "│                                                          │",
            "│                                                          │",
            "│                                                          │",
            "│                                                          │",
            "│                                                          │",
            "│                                                          │",
            "└──────────────────────────────────────────────────────────┘",
            "row 1 of 1  |  Enter detail",
            "Enter detail  /  :  p preview  S sync  ? help  q quit",
        ]
    );
    let detail = project(&cache(vec![job(JobPhase::Verifying)]), Screen::Jobs, 0, 100).detail;
    assert!(
        detail.iter().any(|line| line.label == "stage"
            && line.value == "verifying data and completing installation")
    );
    assert!(
        detail
            .iter()
            .any(|line| line.label == "bytes" && line.value == "4 KiB / 4 KiB (100%)")
    );
}

#[test]
fn overview_exposes_pauses_and_absent_workers() {
    let cache = cache(vec![HomeObject::new(
        Kind::Cluster,
        "home",
        Spec::Cluster(ClusterSpec {
            lock: true,
            encode_pause: true,
            ..Default::default()
        }),
        StatusBody::Cluster(ClusterStatus::default()),
    )]);
    let projection = project(&cache, Screen::Overview, 0, 100);
    assert_eq!(
        projection.rows[0].cells,
        ["home", "home", "scheduling/encode paused"]
    );
    assert_eq!(
        projection
            .rows
            .iter()
            .filter(|row| row.cells[2] == "missing")
            .count(),
        3
    );
    assert!(
        projection
            .detail
            .iter()
            .any(|line| line.value.contains("new Jobs and bindings are blocked"))
    );
}

#[test]
fn stale_worker_has_readable_age_and_recovery_command() {
    let cache = cache(vec![HomeObject::new(
        Kind::Node,
        "inventory",
        Spec::Node(NodeSpec {
            worker_kind: WorkerKind::Inventory,
        }),
        StatusBody::Node(NodeStatus {
            ready: true,
            last_heartbeat_unix: 50,
            ..Default::default()
        }),
    )]);
    let projection = project(&cache, Screen::Nodes, 0, 100);
    assert_eq!(
        projection.rows[0].cells,
        ["inventory", "not-ready", "50s ago"]
    );
    assert!(projection.detail.iter().any(|line| line.label == "inspect"
        && line.value == "systemctl --user status mediaops-home.service"));
}

#[test]
fn unavailable_listing_and_disconnected_actions_explain_next_step() {
    let ui = UiModel {
        screen: Screen::Holds,
        cols: 60,
        rows: 16,
        ..Default::default()
    };
    let cache = cache(Vec::new());
    let current = draw(&ui, &cache, SyncState::Current);
    assert!(current[4].contains("unavailable"));
    assert_eq!(
        current[14],
        "listing unavailable; check inventory on 6 Nodes"
    );
    let stale = draw(&ui, &cache, SyncState::Stale);
    assert_eq!(
        stale[14],
        "actions disabled; reconnecting automatically; ? help"
    );
    assert!(!stale[15].contains("S sync"));
}

#[test]
fn long_errors_remain_readable_in_help_and_scrolling_never_changes_selection() {
    let mut ui = UiModel {
        screen: Screen::Jobs,
        cols: 60,
        rows: 16,
        selected: 4,
        connection_message: Some(format!(
            "{} final error detail",
            "connection diagnostics ".repeat(50)
        )),
        ..Default::default()
    };
    for command in [Command::Help, Command::RowEnd] {
        apply(
            Update {
                ui: &mut ui,
                sync: SyncState::Stale,
                row_count: 10,
                page: 10,
            },
            command,
        );
    }
    assert!(ui.help_offset > 0 && ui.help_offset < u16::MAX);
    assert_eq!(ui.selected, 4);
    let screen = draw(&ui, &cache(Vec::new()), SyncState::Stale);
    let text = screen.join("\n");
    assert!(text.contains("final") && text.contains("detail"), "{text}");
    let end = ui.help_offset;
    apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Stale,
            row_count: 10,
            page: 10,
        },
        Command::RowDelta(-1),
    );
    assert_eq!(ui.help_offset, end - 1);
    apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Stale,
            row_count: 10,
            page: 10,
        },
        Command::RowHome,
    );
    assert_eq!(ui.help_offset, 0);
    assert_eq!(ui.selected, 4);
}

#[test]
fn navigating_after_an_outcome_restores_position_and_keeps_message_in_help() {
    let cache = cache(vec![job(JobPhase::Installed)]);
    let mut ui = UiModel {
        screen: Screen::Jobs,
        message: Some("outcome unknown; refreshing (not resent)".into()),
        ..Default::default()
    };
    assert!(draw(&ui, &cache, SyncState::Current)[22].contains("outcome unknown"));
    apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Current,
            row_count: 1,
            page: 10,
        },
        Command::RowHome,
    );
    assert_eq!(
        draw(&ui, &cache, SyncState::Current)[22],
        "row 1 of 1  |  Enter detail"
    );
    apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Current,
            row_count: 1,
            page: 10,
        },
        Command::Help,
    );
    apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Current,
            row_count: 1,
            page: 10,
        },
        Command::RowEnd,
    );
    let screen = draw(&ui, &cache, SyncState::Current);
    assert!(
        screen
            .join("\n")
            .contains("Last status: outcome unknown; refreshing (not resent)")
    );
    assert!(screen[22].starts_with("lines "));
}

#[test]
fn sync_report_position_takes_precedence_over_underlying_unavailable_inventory() {
    let object = HomeObject::new(
        Kind::Sync,
        "sync-test",
        Spec::Sync(Default::default()),
        StatusBody::Sync(Default::default()),
    );
    let ui = UiModel {
        screen: Screen::Holds,
        report: Some(mediaops_tui::report::SyncReport::from_object(
            "sync-test".into(),
            true,
            &object,
        )),
        ..Default::default()
    };
    let screen = draw(&ui, &cache(Vec::new()), SyncState::Current);
    assert!(screen[22].starts_with("lines "), "{}", screen[22]);
    assert!(!screen[22].contains("listing unavailable"));
}
