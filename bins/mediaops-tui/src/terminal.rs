//! Restore raw/alternate/cursor/paste on return, error, and panic.

use std::io::{self, Stdout, stdout};
use std::panic::{self, PanicHookInfo};
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::cursor;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub struct TerminalGuard {
    raw: bool,
    alt: bool,
    paste: bool,
    cursor_hidden: bool,
    enhanced: bool,
}

static KEYBOARD_EVENT_TYPES: AtomicBool = AtomicBool::new(false);

pub fn keyboard_event_types_enabled() -> bool {
    KEYBOARD_EVENT_TYPES.load(Ordering::SeqCst)
}

fn restore_keyboard() {
    if KEYBOARD_EVENT_TYPES.swap(false, Ordering::SeqCst) {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
}

impl TerminalGuard {
    pub fn enter() -> io::Result<(Self, Terminal<CrosstermBackend<Stdout>>)> {
        let mut guard = Self {
            raw: false,
            alt: false,
            paste: false,
            cursor_hidden: false,
            enhanced: false,
        };
        enable_raw_mode()?;
        guard.raw = true;
        guard.alt = true;
        execute!(stdout(), EnterAlternateScreen)?;
        if supports_keyboard_enhancement().unwrap_or(false) {
            guard.enhanced = true;
            KEYBOARD_EVENT_TYPES.store(true, Ordering::SeqCst);
            execute!(
                stdout(),
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                )
            )?;
        }
        guard.paste = true;
        execute!(stdout(), EnableBracketedPaste)?;
        guard.cursor_hidden = true;
        execute!(stdout(), cursor::Hide)?;
        let backend = CrosstermBackend::new(stdout());
        let terminal = Terminal::new(backend)?;
        Ok((guard, terminal))
    }

    fn restore(&mut self) {
        if self.enhanced {
            restore_keyboard();
            self.enhanced = false;
        }
        if self.cursor_hidden {
            let _ = execute!(stdout(), cursor::Show);
            self.cursor_hidden = false;
        }
        if self.paste {
            let _ = execute!(stdout(), DisableBracketedPaste);
            self.paste = false;
        }
        if self.alt {
            let _ = execute!(stdout(), LeaveAlternateScreen);
            self.alt = false;
        }
        if self.raw {
            let _ = disable_raw_mode();
            self.raw = false;
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

pub fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info: &PanicHookInfo<'_>| {
        restore_keyboard();
        let _ = disable_raw_mode();
        let _ = execute!(
            stdout(),
            cursor::Show,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_starts_uninitialized() {
        let guard = TerminalGuard {
            raw: false,
            alt: false,
            paste: false,
            cursor_hidden: false,
            enhanced: false,
        };
        drop(guard);
    }
}
