//! Home TUI library target for API-side integration tests.

pub mod actions;
pub mod args;
pub mod cache;
mod clock;
pub mod disk;
pub mod format;
pub mod interaction;
pub mod inventory;
pub mod keys;
pub mod model;
pub mod projection;
pub mod runtime;
pub mod sanitize;
pub mod session;
mod subscription;
pub mod terminal;
pub mod update;
pub mod view;
mod view_chrome;
mod view_text;

pub use args::{Args, ColorMode, EXIT_OK, EXIT_RUNTIME, EXIT_USAGE, refuse_non_interactive};
pub use model::{Screen, SyncState, UiModel};
pub use session::{Session, SessionEvent};
