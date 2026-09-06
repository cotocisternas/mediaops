//! Pure key handling. Mutation keys never queue.

use crate::actions::Mutation;
use crate::keys::Command;
use crate::model::{SyncState, UiModel};

pub struct Update<'a> {
    pub ui: &'a mut UiModel,
    pub sync: SyncState,
    pub row_count: usize,
    pub page: usize,
}

pub enum UpdateEffect {
    None,
    Quit,
    RequestMutation(Mutation),
    RequestSync { dry_run: bool },
}

pub fn apply(update: Update<'_>, command: Command) -> UpdateEffect {
    match command {
        Command::Ignore => UpdateEffect::None,
        Command::Quit => UpdateEffect::Quit,
        Command::Help => {
            update.ui.help = !update.ui.help;
            update.ui.rendered_target = None;
            UpdateEffect::None
        }
        Command::Resize { cols, rows } => {
            update.ui.cols = cols;
            update.ui.rows = rows;
            update.ui.rendered_target = None;
            clamp_report_offset(update.ui);
            UpdateEffect::None
        }
        Command::Screen(screen) => {
            update.ui.screen = screen;
            update.ui.select_row(0, update.row_count);
            update.ui.in_detail = false;
            update.ui.help = false;
            update.ui.report = None;
            update.ui.report_offset = 0;
            UpdateEffect::None
        }
        Command::NextScreen => {
            let screen = update.ui.screen.next();
            apply(update, Command::Screen(screen))
        }
        Command::PrevScreen => {
            let screen = update.ui.screen.prev();
            apply(update, Command::Screen(screen))
        }
        Command::RowDelta(delta) => {
            if update.ui.help {
                return UpdateEffect::None;
            }
            if update.ui.report.is_some() {
                let signed =
                    i16::try_from(delta).unwrap_or(if delta < 0 { i16::MIN } else { i16::MAX });
                update.ui.report_offset = update.ui.report_offset.saturating_add_signed(signed);
                clamp_report_offset(update.ui);
                return UpdateEffect::None;
            }
            if update.ui.in_detail {
                let signed =
                    i16::try_from(delta).unwrap_or(if delta < 0 { i16::MIN } else { i16::MAX });
                let next = update.ui.detail_offset.saturating_add_signed(signed);
                update.ui.detail_offset = next;
                update.ui.rendered_target = None;
                return UpdateEffect::None;
            }
            let next = update
                .ui
                .selected
                .saturating_add_signed(isize::try_from(delta).unwrap_or(0));
            update.ui.select_row(next, update.row_count);
            UpdateEffect::None
        }
        Command::PageDelta(delta) => {
            let jump = i32::try_from(update.page)
                .unwrap_or(i32::MAX)
                .max(1)
                .saturating_mul(delta);
            apply(update, Command::RowDelta(jump))
        }
        Command::RowHome => {
            if update.ui.report.is_some() {
                update.ui.report_offset = 0;
            } else if update.ui.in_detail {
                update.ui.detail_offset = 0;
                update.ui.rendered_target = None;
            } else {
                update.ui.select_row(0, update.row_count);
            }
            UpdateEffect::None
        }
        Command::RowEnd => {
            if update.ui.report.is_some() {
                update.ui.report_offset = u16::MAX;
                clamp_report_offset(update.ui);
            } else if update.ui.in_detail {
                update.ui.detail_offset = u16::MAX;
                update.ui.rendered_target = None;
            } else {
                update
                    .ui
                    .select_row(update.row_count.saturating_sub(1), update.row_count);
            }
            UpdateEffect::None
        }
        Command::EnterDetail => {
            if update.row_count > 0 {
                update.ui.in_detail = true;
                update.ui.rendered_target = None;
            }
            UpdateEffect::None
        }
        Command::Back => {
            update.ui.rendered_target = None;
            if update.ui.help {
                update.ui.help = false;
            } else if update.ui.report.is_some() {
                update.ui.report = None;
                update.ui.report_offset = 0;
            } else {
                update.ui.in_detail = false;
            }
            UpdateEffect::None
        }
        Command::PreviewSync => request_sync(update, true),
        Command::RunSync => request_sync(update, false),
        Command::ReleaseSync => {
            if update.ui.keyboard_event_types {
                update.ui.sync_key_held = false;
            }
            UpdateEffect::None
        }
        Command::Mutate(mutation) => {
            if !mutation.allowed_on(update.ui.screen) {
                return UpdateEffect::None;
            }
            if !update.ui.mutations_enabled(update.sync) {
                return UpdateEffect::None;
            }
            update.ui.mutation_pending = true;
            UpdateEffect::RequestMutation(mutation)
        }
    }
}

fn clamp_report_offset(ui: &mut UiModel) {
    let Some(report) = ui.report.as_ref() else {
        return;
    };
    let max = report.max_offset(
        usize::from(ui.cols),
        usize::from(ui.rows.saturating_sub(5)).max(1),
    );
    ui.report_offset = ui.report_offset.min(max);
}

fn request_sync(update: Update<'_>, dry_run: bool) -> UpdateEffect {
    if !update.ui.sync_actions_enabled(update.sync) || (!dry_run && update.ui.sync_key_held) {
        return UpdateEffect::None;
    }
    if !dry_run {
        update.ui.sync_key_held = true;
    }
    update.ui.sync_pending = true;
    UpdateEffect::RequestSync { dry_run }
}
