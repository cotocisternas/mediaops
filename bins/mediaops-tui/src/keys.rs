//! Closed key set. Repeat, release, and paste never mutate.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};

use crate::actions::Mutation;
use crate::model::{Screen, UiModel};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Quit,
    Help,
    Screen(Screen),
    NextScreen,
    PrevScreen,
    RowDelta(i32),
    PageDelta(i32),
    RowHome,
    RowEnd,
    EnterDetail,
    Back,
    Filter,
    CommandInput,
    InputChar(char),
    InputBackspace,
    InputClear,
    InputAccept,
    InputCancel,
    Mutate(Mutation),
    PreviewSync,
    RunSync,
    ReleaseSync,
    Resize { cols: u16, rows: u16 },
    Ignore,
}

/// Text input is decoded before global shortcuts. Release events still reach
/// the sync latch, but cannot insert text or queue an operation.
pub fn command_for_ui(event: &Event, ui: &UiModel) -> Command {
    // The resize notice replaces every pane, including the editor. Honor its
    // advertised exit key without interpreting hidden input or action keys.
    if ui.undersize() {
        return match command_from_event(event) {
            command @ (Command::Quit | Command::ReleaseSync | Command::Resize { .. }) => command,
            _ => Command::Ignore,
        };
    }
    if ui.input.is_none() {
        return command_from_event(event);
    }
    match event {
        Event::Resize(cols, rows) => Command::Resize {
            cols: *cols,
            rows: *rows,
        },
        Event::Key(key) => {
            if key.kind == KeyEventKind::Release && matches!(key.code, KeyCode::Char('S' | 's')) {
                return Command::ReleaseSync;
            }
            if key.kind != KeyEventKind::Press {
                return Command::Ignore;
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                return match key.code {
                    KeyCode::Char('c') => Command::Quit,
                    KeyCode::Char('u') => Command::InputClear,
                    _ => Command::Ignore,
                };
            }
            match key.code {
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::ALT) && !c.is_control() =>
                {
                    Command::InputChar(c)
                }
                KeyCode::Backspace => Command::InputBackspace,
                KeyCode::Enter => Command::InputAccept,
                KeyCode::Esc => Command::InputCancel,
                _ => Command::Ignore,
            }
        }
        _ => Command::Ignore,
    }
}

pub fn command_from_event(event: &Event) -> Command {
    match event {
        Event::Resize(cols, rows) => Command::Resize {
            cols: *cols,
            rows: *rows,
        },
        Event::Key(key) => command_from_key(*key),
        Event::Paste(_) | Event::FocusGained | Event::FocusLost => Command::Ignore,
        Event::Mouse(mouse) => match mouse.kind {
            MouseEventKind::ScrollUp => Command::RowDelta(-1),
            MouseEventKind::ScrollDown => Command::RowDelta(1),
            _ => Command::Ignore,
        },
    }
}

fn command_from_key(key: KeyEvent) -> Command {
    if key.kind == KeyEventKind::Release && matches!(key.code, KeyCode::Char('S' | 's')) {
        return Command::ReleaseSync;
    }
    if key.kind == KeyEventKind::Release || key.kind == KeyEventKind::Repeat {
        return Command::Ignore;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Command::Quit;
    }
    match key.code {
        KeyCode::Char('q') => Command::Quit,
        KeyCode::Char('?') => Command::Help,
        KeyCode::Tab => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                Command::PrevScreen
            } else {
                Command::NextScreen
            }
        }
        KeyCode::BackTab => Command::PrevScreen,
        KeyCode::Char(d @ '1'..='7') => Screen::from_digit(d as u8 - b'0')
            .map(Command::Screen)
            .unwrap_or(Command::Ignore),
        KeyCode::Up | KeyCode::Char('k') => Command::RowDelta(-1),
        KeyCode::Down | KeyCode::Char('j') => Command::RowDelta(1),
        KeyCode::PageUp => Command::PageDelta(-1),
        KeyCode::PageDown => Command::PageDelta(1),
        KeyCode::Home | KeyCode::Char('g') => Command::RowHome,
        KeyCode::End | KeyCode::Char('G') => Command::RowEnd,
        KeyCode::Enter | KeyCode::Char('d') => Command::EnterDetail,
        KeyCode::Esc => Command::Back,
        KeyCode::Char('/') => Command::Filter,
        KeyCode::Char(':') => Command::CommandInput,
        KeyCode::Char('W') => Command::Mutate(Mutation::ApplyWant),
        KeyCode::Char('D') => Command::Mutate(Mutation::DeleteWant),
        KeyCode::Char('A') => Command::Mutate(Mutation::ApproveHold),
        KeyCode::Char('X') => Command::Mutate(Mutation::RejectHold),
        KeyCode::Char('p') => Command::PreviewSync,
        KeyCode::Char('S') => Command::RunSync,
        _ => Command::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn mutation_keys_are_shift_letters_on_press_only() {
        assert_eq!(
            command_from_key(press(KeyCode::Char('W'))),
            Command::Mutate(Mutation::ApplyWant)
        );
        let mut repeat = press(KeyCode::Char('W'));
        repeat.kind = KeyEventKind::Repeat;
        assert_eq!(command_from_key(repeat), Command::Ignore);
        assert_eq!(
            command_from_event(&Event::Paste("WWW".into())),
            Command::Ignore
        );
        assert_eq!(
            command_from_key(press(KeyCode::Enter)),
            Command::EnterDetail
        );
        assert_eq!(
            command_from_key(press(KeyCode::Char('p'))),
            Command::PreviewSync
        );
        assert_eq!(
            command_from_key(press(KeyCode::Char('S'))),
            Command::RunSync
        );
        assert_eq!(command_from_key(press(KeyCode::Char('s'))), Command::Ignore);
        let mut sync_repeat = press(KeyCode::Char('S'));
        sync_repeat.kind = KeyEventKind::Repeat;
        assert_eq!(command_from_key(sync_repeat), Command::Ignore);
        assert_eq!(
            command_from_event(&Event::Paste("pS".into())),
            Command::Ignore
        );
    }
}
