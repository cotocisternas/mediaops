//! Closed key set. Repeat, release, and paste never mutate.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};

use crate::actions::Mutation;
use crate::model::Screen;

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
    Mutate(Mutation),
    Resize { cols: u16, rows: u16 },
    Ignore,
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
        KeyCode::Home => Command::RowHome,
        KeyCode::End => Command::RowEnd,
        KeyCode::Enter => Command::EnterDetail,
        KeyCode::Esc => Command::Back,
        KeyCode::Char('W') => Command::Mutate(Mutation::ApplyWant),
        KeyCode::Char('D') => Command::Mutate(Mutation::DeleteWant),
        KeyCode::Char('A') => Command::Mutate(Mutation::ApproveHold),
        KeyCode::Char('X') => Command::Mutate(Mutation::RejectHold),
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
    }
}
