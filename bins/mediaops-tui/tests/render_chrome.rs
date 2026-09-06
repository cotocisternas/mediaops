use ratatui::Terminal;
use ratatui::backend::TestBackend;

use mediaops_core::{
    HoldSpec, HoldStatus, HomeObject, Kind, Spec, StatusBody, WantSpec, WantStatus,
};
use mediaops_tui::SyncState;
use mediaops_tui::cache::ObjectCache;
use mediaops_tui::disk::DiskObservation;
use mediaops_tui::model::{Screen, UiModel};
use mediaops_tui::projection::{HOLD_CAPTION, project};
use mediaops_tui::view::render;

fn lines(term: &Terminal<TestBackend>) -> Vec<String> {
    let buf = term.backend().buffer();
    let area = buf.area();
    (0..area.height)
        .map(|y| {
            let mut s = String::new();
            for x in 0..area.width {
                s.push_str(buf[(x, y)].symbol());
            }
            s.trim_end().to_string()
        })
        .collect()
}

fn paint(ui: UiModel, screen: Screen, sync: SyncState) -> Vec<String> {
    let mut cache = ObjectCache::default();
    let epoch = cache.bump_epoch();
    cache.install_baseline(
        epoch,
        vec![HomeObject::new(
            Kind::Want,
            "movie:tmdb:603",
            Spec::Want(WantSpec {
                title_id: "movie:tmdb:603".into(),
            }),
            StatusBody::Want(WantStatus::default()),
        )],
    );
    let projection = project(&cache, screen, 0, 0);
    let backend = TestBackend::new(ui.cols, ui.rows);
    let mut terminal = Terminal::new(backend).expect("term");
    terminal
        .draw(|f| {
            render(
                f,
                &ui,
                sync,
                &projection,
                &DiskObservation::unavailable(),
                false,
                false,
            );
        })
        .expect("draw");
    lines(&terminal)
}

#[test]
fn footer_allowlist_and_help_omits_mutations() {
    let base = |screen, detail, help| UiModel {
        screen,
        cols: 80,
        rows: 24,
        in_detail: detail,
        help,
        selected_key: Some(mediaops_tui::cache::ObjectKey::new(
            Kind::Want,
            "movie:tmdb:603",
        )),
        ..UiModel::default()
    };
    let jobs = paint(
        base(Screen::Jobs, true, false),
        Screen::Jobs,
        SyncState::Current,
    );
    let footer = jobs.last().expect("footer");
    assert!(!footer.contains("W apply"), "{footer}");
    assert!(!footer.contains("D delete"), "{footer}");
    let overview = paint(
        base(Screen::Overview, true, false),
        Screen::Overview,
        SyncState::Current,
    );
    let footer = overview.last().expect("footer");
    assert!(!footer.contains("W apply"), "{footer}");
    let titles = paint(
        base(Screen::Titles, true, false),
        Screen::Titles,
        SyncState::Current,
    );
    let footer = titles.last().expect("footer");
    assert!(footer.contains("W apply"), "{footer}");
    assert!(!footer.contains("D delete"), "{footer}");
    let help = paint(
        base(Screen::Wants, true, true),
        Screen::Wants,
        SyncState::Current,
    );
    let footer = help.last().expect("footer");
    assert!(footer.contains("Esc dismiss"), "{footer}");
    assert!(!footer.contains("W apply"), "{footer}");
}

#[test]
fn minimum_detail_footers_preserve_quit_and_help() {
    for screen in [Screen::Wants, Screen::Holds, Screen::Titles] {
        let ui = UiModel {
            screen,
            cols: 60,
            rows: 16,
            in_detail: true,
            selected_key: Some(mediaops_tui::cache::ObjectKey::new(
                Kind::Want,
                "movie:tmdb:603",
            )),
            ..Default::default()
        };
        let output = paint(ui, screen, SyncState::Current);
        let footer = output.last().expect("footer");
        assert!(footer.contains("q quit"), "{screen:?}: {footer}");
        assert!(footer.contains("? help"), "{screen:?}: {footer}");
        assert!(footer.contains("Esc"), "{screen:?}: {footer}");
    }
}

#[test]
fn message_and_pending_live_on_status_row() {
    let mut ui = UiModel {
        screen: Screen::Wants,
        cols: 80,
        rows: 24,
        in_detail: false,
        message: Some("conflict; refreshed".into()),
        mutation_pending: true,
        ..UiModel::default()
    };
    let rows = paint(ui.clone(), Screen::Wants, SyncState::Current);
    let status = &rows[rows.len() - 2];
    assert!(status.contains("pending"), "{status}");
    assert!(status.contains("conflict"), "{status}");
    ui.mutation_pending = false;
    let rows = paint(ui, Screen::Wants, SyncState::Current);
    let status = &rows[rows.len() - 2];
    assert!(status.contains("conflict"), "{status}");
}

#[test]
fn hold_caption_reserved_while_scrolled() {
    let mut cache = ObjectCache::default();
    let epoch = cache.bump_epoch();
    cache.install_baseline(
        epoch,
        vec![
            HomeObject::new(
                Kind::Node,
                "inventory",
                Spec::Node(mediaops_core::NodeSpec {
                    worker_kind: mediaops_core::WorkerKind::Inventory,
                }),
                StatusBody::Node(mediaops_core::NodeStatus {
                    list_generation: 1,
                    list_completed_unix: 1,
                    ready: true,
                    last_heartbeat_unix: 1,
                }),
            ),
            HomeObject::new(
                Kind::Hold,
                "movie:tmdb:1-a",
                Spec::Hold(HoldSpec {
                    title_id: "movie:tmdb:1".into(),
                    release_id: "a".into(),
                    decision: mediaops_core::HoldDecisionSpec::Empty,
                }),
                StatusBody::Hold(HoldStatus {
                    list_generation: 1,
                    reason: "Found matching movie via grab history, but release was matched to movie by ID. Manual Import required.".into(),
                    ..HoldStatus::default()
                }),
            ),
        ],
    );
    let projection = project(&cache, Screen::Holds, 0, 1);
    let ui = UiModel {
        screen: Screen::Holds,
        cols: 80,
        rows: 24,
        in_detail: true,
        detail_offset: 8,
        ..UiModel::default()
    };
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("term");
    terminal
        .draw(|f| {
            render(
                f,
                &ui,
                SyncState::Current,
                &projection,
                &DiskObservation::unavailable(),
                false,
                false,
            );
        })
        .expect("draw");
    let text = lines(&terminal).join("\n");
    assert!(text.contains(HOLD_CAPTION), "{text}");
    let ui = UiModel {
        screen: Screen::Holds,
        cols: 80,
        rows: 24,
        in_detail: true,
        detail_offset: 0,
        ..UiModel::default()
    };
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("term");
    terminal
        .draw(|f| {
            render(
                f,
                &ui,
                SyncState::Current,
                &projection,
                &DiskObservation::unavailable(),
                false,
                false,
            );
        })
        .expect("draw");
    let text = lines(&terminal).join("\n");
    assert!(
        text.contains("reason") || text.contains("Manual Import") || text.contains("grab history"),
        "{text}"
    );
    assert!(text.contains(HOLD_CAPTION), "{text}");
}
