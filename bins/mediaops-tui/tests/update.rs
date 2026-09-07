use mediaops_core::Kind;
use mediaops_tui::actions::Mutation;
use mediaops_tui::cache::ObjectKey;
use mediaops_tui::keys::Command;
use mediaops_tui::model::{Screen, SyncState, UiModel};
use mediaops_tui::update::{Update, UpdateEffect, apply};

fn ready_ui() -> UiModel {
    UiModel {
        screen: Screen::Wants,
        in_detail: true,
        cols: 80,
        rows: 24,
        selected_key: Some(ObjectKey::new(Kind::Want, "movie:tmdb:1")),
        selected_uid: Some("u".into()),
        selected_rv: Some(1),
        ..UiModel::default()
    }
}

#[test]
fn mutation_does_not_queue_while_pending() {
    let mut ui = ready_ui();
    let effect = apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Current,
            row_count: 1,
            page: 10,
        },
        Command::Mutate(Mutation::ApplyWant),
    );
    assert!(matches!(
        effect,
        UpdateEffect::RequestMutation(Mutation::ApplyWant)
    ));
    let effect = apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Current,
            row_count: 1,
            page: 10,
        },
        Command::Mutate(Mutation::DeleteWant),
    );
    assert!(matches!(effect, UpdateEffect::None));
}

#[test]
fn enter_never_writes() {
    let mut ui = ready_ui();
    ui.in_detail = false;
    let effect = apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Current,
            row_count: 2,
            page: 10,
        },
        Command::EnterDetail,
    );
    assert!(matches!(effect, UpdateEffect::None));
    assert!(ui.in_detail);
}

#[test]
fn global_sync_requires_current_idle_and_does_not_queue() {
    let mut ui = ready_ui();
    let effect = apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Current,
            row_count: 1,
            page: 10,
        },
        Command::PreviewSync,
    );
    assert!(matches!(
        effect,
        UpdateEffect::RequestSync { dry_run: true }
    ));
    let effect = apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Current,
            row_count: 1,
            page: 10,
        },
        Command::RunSync,
    );
    assert!(matches!(effect, UpdateEffect::None));
}

#[test]
fn global_sync_ignored_when_help_stale_or_undersize() {
    let mut ui = ready_ui();
    ui.help = true;
    assert!(matches!(
        apply(
            Update {
                ui: &mut ui,
                sync: SyncState::Current,
                row_count: 1,
                page: 10,
            },
            Command::RunSync,
        ),
        UpdateEffect::None
    ));
    ui.help = false;
    assert!(matches!(
        apply(
            Update {
                ui: &mut ui,
                sync: SyncState::Stale,
                row_count: 1,
                page: 10,
            },
            Command::RunSync,
        ),
        UpdateEffect::None
    ));
    ui.cols = 40;
    assert!(matches!(
        apply(
            Update {
                ui: &mut ui,
                sync: SyncState::Current,
                row_count: 1,
                page: 10,
            },
            Command::PreviewSync,
        ),
        UpdateEffect::None
    ));
}

#[test]
fn esc_closes_report_without_writing() {
    let mut ui = ready_ui();
    ui.report = Some(mediaops_tui::report::SyncReport::from_object(
        "sync-1".into(),
        true,
        &mediaops_core::HomeObject::new(
            Kind::Want,
            "x",
            mediaops_core::Spec::Want(mediaops_core::WantSpec {
                title_id: "x".into(),
            }),
            mediaops_core::StatusBody::empty(Kind::Want),
        ),
    ));
    let effect = apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Current,
            row_count: 1,
            page: 10,
        },
        Command::Back,
    );
    assert!(matches!(effect, UpdateEffect::None));
    assert!(ui.report.is_none());
    assert!(ui.in_detail);
}

#[test]
fn report_end_clamps_offset_to_visible_page() {
    let mut ui = ready_ui();
    ui.cols = 60;
    ui.rows = 16;
    ui.report = Some(mediaops_tui::report::SyncReport::from_object(
        "sync-1".into(),
        true,
        &mediaops_core::HomeObject::new(
            Kind::Sync,
            "sync-1",
            mediaops_core::Spec::Sync(mediaops_core::SyncSpec::default()),
            mediaops_core::StatusBody::Sync(mediaops_core::SyncStatus {
                phase: mediaops_core::SyncPhase::Captured,
                list_generation: 4,
                entries: vec![mediaops_core::SyncEntry {
                    remote_root: "seedbox".into(),
                    remote_path: "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv".repeat(20),
                    job: Some(mediaops_core::JobSpec {
                        dest_rel: "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
                        ..mediaops_core::JobSpec::default()
                    }),
                    disposition: mediaops_core::SyncDisposition::WouldQueue,
                    ..mediaops_core::SyncEntry::default()
                }],
                ..mediaops_core::SyncStatus::default()
            }),
        ),
    ));
    apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Current,
            row_count: 1,
            page: 2,
        },
        Command::RowEnd,
    );
    let report = ui.report.as_ref().expect("report");
    let pane = mediaops_tui::geometry::Shell::for_ui(&ui).overlay.inner;
    let max = report.max_offset(usize::from(pane.width), usize::from(pane.height));
    assert!(max > 0);
    assert_eq!(ui.report_offset, max);
    assert!(ui.report_offset < u16::MAX);
    apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Current,
            row_count: 1,
            page: 2,
        },
        Command::RowDelta(-1),
    );
    assert_eq!(ui.report_offset, max - 1);
}
