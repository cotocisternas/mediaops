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
            UpdateEffect::None
        }
        Command::Screen(screen) => {
            update.ui.screen = screen;
            update.ui.select_row(0, update.row_count);
            update.ui.in_detail = false;
            update.ui.help = false;
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
            if update.ui.in_detail {
                update.ui.detail_offset = 0;
                update.ui.rendered_target = None;
            } else {
                update.ui.select_row(0, update.row_count);
            }
            UpdateEffect::None
        }
        Command::RowEnd => {
            if update.ui.in_detail {
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
            } else {
                update.ui.in_detail = false;
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

#[cfg(test)]
mod tests {
    use mediaops_core::Kind;

    use super::*;
    use crate::cache::ObjectKey;
    use crate::model::Screen;

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
}
