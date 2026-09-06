use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};

use mediaops_core::{
    HomeObject, JobPhase, JobSpec, JobStatus, Kind, Spec, StatusBody, WantSpec, WantStatus,
};
use mediaops_tui::SyncState;
use mediaops_tui::cache::ObjectCache;
use mediaops_tui::disk::DiskObservation;
use mediaops_tui::model::{Screen, UiModel};
use mediaops_tui::projection::project;
use mediaops_tui::view::render;

const SIZES: [(u16, u16); 4] = [(140, 40), (80, 24), (60, 16), (40, 10)];

fn cache() -> ObjectCache {
    let mut cache = ObjectCache::default();
    let epoch = cache.bump_epoch();
    let mut job = HomeObject::new(
        Kind::Job,
        "pull-matrix",
        Spec::Job(JobSpec {
            title_id: "movie:tmdb:603".into(),
            file_len: 6_800_000_000,
            ..JobSpec::default()
        }),
        StatusBody::Job(JobStatus {
            phase: JobPhase::Pulling,
            bytes_done: 1_200_000_000,
            attempts: 1,
            message: "range timeout then more words for wrapping".into(),
            ..JobStatus::default()
        }),
    );
    job.metadata.uid = "job-1".into();
    job.metadata.resource_version = 2;
    cache.install_baseline(
        epoch,
        vec![
            HomeObject::new(
                Kind::Want,
                "movie:tmdb:603",
                Spec::Want(WantSpec {
                    title_id: "movie:tmdb:603".into(),
                }),
                StatusBody::Want(WantStatus::default()),
            ),
            job,
        ],
    );
    cache
}

fn draw(
    cols: u16,
    rows: u16,
    screen: Screen,
    sync: SyncState,
    in_detail: bool,
    help: bool,
    color: bool,
) -> Terminal<TestBackend> {
    let cache = cache();
    let projection = project(&cache, screen, 0, 10);
    let ui = UiModel {
        screen,
        cols,
        rows,
        in_detail,
        help,
        selected_key: Some(mediaops_tui::cache::ObjectKey::new(
            Kind::Want,
            "movie:tmdb:603",
        )),
        ..UiModel::default()
    };
    let backend = TestBackend::new(cols, rows);
    let mut terminal = Terminal::new(backend).expect("term");
    terminal
        .draw(|f| {
            render(
                f,
                &ui,
                sync,
                &projection,
                &DiskObservation::unavailable(),
                color,
                false,
            );
        })
        .expect("draw");
    terminal
}

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

#[test]
fn every_screen_at_required_sizes() {
    for screen in Screen::ALL {
        for (cols, rows) in SIZES {
            let term = draw(cols, rows, screen, SyncState::Current, false, false, true);
            let text = lines(&term).join("\n");
            if cols < 60 || rows < 16 {
                assert!(
                    text.contains("too small"),
                    "{screen:?} {cols}x{rows}: {text}"
                );
            } else {
                assert!(text.contains("mediaops"), "{screen:?} {cols}x{rows}");
                assert!(text.contains(screen.title()), "{screen:?} {cols}x{rows}");
            }
        }
    }
}

#[test]
fn connecting_does_not_paint_empty_english() {
    let cache = ObjectCache::default();
    let projection = project(&cache, Screen::Wants, 0, 0);
    let ui = UiModel {
        screen: Screen::Wants,
        cols: 80,
        rows: 24,
        ..UiModel::default()
    };
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("term");
    terminal
        .draw(|f| {
            render(
                f,
                &ui,
                SyncState::Connecting,
                &projection,
                &DiskObservation::unavailable(),
                false,
                false,
            );
        })
        .expect("draw");
    let text = lines(&terminal).join("\n");
    assert!(text.contains("reconnecting"), "{text}");
    assert!(!text.contains("nothing happening"), "{text}");
}

#[test]
fn jobs_header_keeps_separate_columns_at_60() {
    let term = draw(
        60,
        16,
        Screen::Jobs,
        SyncState::Current,
        false,
        false,
        false,
    );
    let header = &lines(&term)[2];
    let title = header.find("TITLE").expect("TITLE");
    let phase = header.find("PHASE").expect("PHASE");
    assert!(phase > title + 5, "{header}");
}

#[test]
fn selected_row_is_reverse_cyan() {
    let term = draw(
        80,
        24,
        Screen::Wants,
        SyncState::Current,
        false,
        false,
        true,
    );
    let buf = term.backend().buffer();
    let cell = &buf[(0, 3)];
    assert!(cell.modifier.contains(Modifier::REVERSED), "{cell:?}");
    assert_eq!(cell.fg, Color::Cyan);
}

#[test]
fn current_is_green_stale_is_yellow_undersize_is_red() {
    let current = draw(
        80,
        24,
        Screen::Wants,
        SyncState::Current,
        false,
        false,
        true,
    );
    let buf = current.backend().buffer();
    let mut saw_green = false;
    for x in 0..80 {
        if buf[(x, 0)].symbol() == "C" && buf[(x, 0)].fg == Color::Green {
            saw_green = true;
        }
    }
    assert!(saw_green);
    let stale = draw(80, 24, Screen::Wants, SyncState::Stale, false, false, true);
    let buf = stale.backend().buffer();
    let mut saw_yellow = false;
    for x in 0..80 {
        if buf[(x, 0)].fg == Color::Yellow {
            saw_yellow = true;
        }
    }
    assert!(saw_yellow);
    let small = draw(
        40,
        10,
        Screen::Wants,
        SyncState::Current,
        false,
        false,
        true,
    );
    assert_eq!(small.backend().buffer()[(0, 0)].fg, Color::Red);
}
