//! Pure key handling. Mutation keys never queue.

use crate::actions::Mutation;
use crate::keys::Command;
use crate::model::{InputMode, Screen, SyncState, UiModel};

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
    let was_editing = update.ui.input.is_some();
    let effect = apply_inner(
        Update {
            ui: update.ui,
            sync: update.sync,
            row_count: update.row_count,
            page: update.page,
        },
        command,
    );
    if was_editing && update.ui.input.is_none() {
        update.ui.present_deferred_report();
    }
    effect
}

fn apply_inner(update: Update<'_>, command: Command) -> UpdateEffect {
    // Defense in depth: callers cannot bypass the input decoder to write or
    // navigate underneath an editor. Ctrl-C is represented by Quit.
    if update.ui.input.is_some()
        && !matches!(
            command,
            Command::Quit
                | Command::Resize { .. }
                | Command::ReleaseSync
                | Command::InputChar(_)
                | Command::InputBackspace
                | Command::InputClear
                | Command::InputAccept
                | Command::InputCancel
                | Command::Ignore
        )
    {
        return UpdateEffect::None;
    }
    if !update.ui.mutation_pending
        && !update.ui.sync_pending
        && !update.ui.message_unseen
        && matches!(
            command,
            Command::Screen(_)
                | Command::NextScreen
                | Command::PrevScreen
                | Command::RowDelta(_)
                | Command::PageDelta(_)
                | Command::RowHome
                | Command::RowEnd
                | Command::EnterDetail
                | Command::Back
                | Command::Filter
                | Command::CommandInput
                | Command::InputChar(_)
                | Command::InputBackspace
                | Command::InputClear
                | Command::InputAccept
                | Command::InputCancel
        )
        && let Some(message) = update.ui.message.take()
    {
        update.ui.last_message = Some(message);
    }
    match command {
        Command::Ignore => UpdateEffect::None,
        Command::Quit => UpdateEffect::Quit,
        Command::Filter => {
            update.ui.input = Some(InputMode::Filter {
                original: update.ui.filter.clone(),
            });
            update.ui.help = false;
            update.ui.report = None;
            update.ui.clear_action_selection();
            UpdateEffect::None
        }
        Command::CommandInput => {
            update.ui.input = Some(InputMode::Command {
                text: String::new(),
            });
            update.ui.rendered_target = None;
            UpdateEffect::None
        }
        Command::InputChar(_) | Command::InputBackspace | Command::InputClear => {
            update.ui.rendered_target = None;
            let text = match update.ui.input.as_mut() {
                Some(InputMode::Filter { .. }) => {
                    update.ui.table_offset = 0;
                    &mut update.ui.filter
                }
                Some(InputMode::Command { text }) => text,
                None => return UpdateEffect::None,
            };
            match command {
                Command::InputChar(c) if !c.is_control() => text.push(c),
                Command::InputBackspace => {
                    text.pop();
                }
                Command::InputClear => text.clear(),
                _ => {}
            }
            UpdateEffect::None
        }
        Command::InputCancel => {
            if let Some(InputMode::Filter { original }) = update.ui.input.take() {
                update.ui.filter = original;
            }
            update.ui.rendered_target = None;
            UpdateEffect::None
        }
        Command::InputAccept => {
            update.ui.rendered_target = None;
            match update.ui.input.take() {
                Some(InputMode::Command { text }) => {
                    let text = text.trim().to_lowercase();
                    if let Some(screen) = Screen::from_alias(&text) {
                        apply(update, Command::Screen(screen))
                    } else {
                        match text.as_str() {
                            "help" => {
                                update.ui.help = true;
                                update.ui.help_offset = 0;
                                UpdateEffect::None
                            }
                            "q" | "quit" => UpdateEffect::Quit,
                            _ => {
                                let feedback = format!(
                                    "Unknown command :{text}; use :help for resource aliases"
                                );
                                update.ui.message = Some(if update.ui.message_unseen {
                                    format!(
                                        "{}; {feedback}",
                                        update.ui.message.as_deref().unwrap_or_default()
                                    )
                                } else {
                                    feedback
                                });
                                UpdateEffect::None
                            }
                        }
                    }
                }
                _ => UpdateEffect::None,
            }
        }
        Command::Help => {
            update.ui.help = !update.ui.help;
            update.ui.help_offset = 0;
            update.ui.rendered_target = None;
            UpdateEffect::None
        }
        Command::Resize { cols, rows } => {
            update.ui.cols = cols;
            update.ui.rows = rows;
            update.ui.rendered_target = None;
            clamp_report_offset(update.ui);
            clamp_help_offset(update.ui);
            UpdateEffect::None
        }
        Command::Screen(screen) => {
            update.ui.screen = screen;
            update.ui.select_row(0, update.row_count);
            update.ui.in_detail = false;
            update.ui.help = false;
            update.ui.report = None;
            update.ui.report_offset = 0;
            update.ui.filter.clear();
            update.ui.table_offset = 0;
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
                let signed =
                    i16::try_from(delta).unwrap_or(if delta < 0 { i16::MIN } else { i16::MAX });
                update.ui.help_offset = update.ui.help_offset.saturating_add_signed(signed);
                clamp_help_offset(update.ui);
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
            if update.ui.help {
                update.ui.help_offset = 0;
            } else if update.ui.report.is_some() {
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
            if update.ui.help {
                update.ui.help_offset = u16::MAX;
                clamp_help_offset(update.ui);
            } else if update.ui.report.is_some() {
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
            if update.row_count > 0 && !update.ui.help && update.ui.report.is_none() {
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
            } else if update.ui.in_detail {
                update.ui.in_detail = false;
            } else {
                update.ui.filter.clear();
                update.ui.table_offset = 0;
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
            update.ui.pending_started = Some(std::time::Instant::now());
            UpdateEffect::RequestMutation(mutation)
        }
    }
}

pub(crate) fn clamp_overlays(ui: &mut UiModel) {
    clamp_help_offset(ui);
    clamp_report_offset(ui);
}

/// Completion may arrive while an editor owns the body and status row. Keep
/// the result until the editor closes, and retain feedback until a real draw.
pub fn complete_sync(ui: &mut UiModel, result: Result<crate::report::SyncReport, String>) {
    ui.sync_pending = false;
    ui.pending_started = None;
    match result {
        Ok(report) => {
            let message = (!report.dry_run && !ui.keyboard_event_types)
                .then(|| "For another sync, restart TUI or use the CLI".into());
            if ui.input.is_some() {
                ui.deferred_report = Some(report);
            } else {
                ui.report = Some(report);
                ui.report_offset = 0;
            }
            ui.completion_message(message);
        }
        Err(message) => ui.completion_message(Some(message)),
    }
}

fn clamp_help_offset(ui: &mut UiModel) {
    let lines = crate::view_text::help_lines(ui).len();
    let visible = usize::from(crate::geometry::Shell::for_ui(ui).overlay.inner.height);
    ui.help_offset = ui
        .help_offset
        .min(u16::try_from(lines.saturating_sub(visible)).unwrap_or(u16::MAX));
}

fn clamp_report_offset(ui: &mut UiModel) {
    let Some(report) = ui.report.as_ref() else {
        return;
    };
    let area = crate::geometry::Shell::for_ui(ui).overlay.inner;
    let max = report.max_offset(usize::from(area.width), usize::from(area.height));
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
    update.ui.pending_started = Some(std::time::Instant::now());
    UpdateEffect::RequestSync { dry_run }
}
