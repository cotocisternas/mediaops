use mediaops_tui::keys::Command;
use mediaops_tui::update::{Update, UpdateEffect, apply};
use mediaops_tui::{SyncState, UiModel};

fn key(ui: &mut UiModel, command: Command) -> UpdateEffect {
    apply(
        Update {
            ui,
            sync: SyncState::Current,
            row_count: 0,
            page: 10,
        },
        command,
    )
}

#[test]
fn legacy_repeated_press_cannot_start_another_sync_after_completion() {
    let mut ui = UiModel::default();
    assert!(matches!(
        key(&mut ui, Command::RunSync),
        UpdateEffect::RequestSync { dry_run: false }
    ));
    ui.sync_pending = false;
    assert!(matches!(key(&mut ui, Command::RunSync), UpdateEffect::None));
    key(&mut ui, Command::Back);
    assert!(matches!(key(&mut ui, Command::RunSync), UpdateEffect::None));
    assert!(matches!(
        key(&mut ui, Command::PreviewSync),
        UpdateEffect::RequestSync { dry_run: true }
    ));
}

#[test]
fn event_reporting_requires_release_before_next_sync_press() {
    let mut ui = UiModel {
        keyboard_event_types: true,
        ..Default::default()
    };
    assert!(matches!(
        key(&mut ui, Command::RunSync),
        UpdateEffect::RequestSync { dry_run: false }
    ));
    ui.sync_pending = false;
    assert!(matches!(key(&mut ui, Command::RunSync), UpdateEffect::None));
    key(&mut ui, Command::ReleaseSync);
    assert!(matches!(
        key(&mut ui, Command::RunSync),
        UpdateEffect::RequestSync { dry_run: false }
    ));
}
