//! Invocation flags. Help and version never need a socket or TTY.

use std::io::{self, IsTerminal};
use std::path::PathBuf;

use clap::{Parser, ValueEnum};

pub const EXIT_OK: i32 = 0;
pub const EXIT_RUNTIME: i32 = 1;
pub const EXIT_USAGE: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Parser)]
#[command(
    name = "mediaops-tui",
    version,
    about = "Home API terminal UI. Additive; the CLI is unchanged."
)]
pub struct Args {
    /// Home API unix socket. Defaults to the account runtime socket.
    #[arg(long = "api-socket", value_name = "PATH")]
    pub api_socket: Option<PathBuf>,
    /// Color when the terminal supports it.
    #[arg(long, value_enum, default_value_t = ColorMode::Auto)]
    pub color: ColorMode,
}

impl Args {
    pub fn color_enabled(&self) -> bool {
        match self.color {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => std::env::var_os("NO_COLOR").is_none(),
        }
    }
}

/// Interactive stdin/stdout must be terminals; TERM=dumb is a usage refusal.
pub fn refuse_non_interactive() -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("mediaops-tui needs a terminal on stdin and stdout".into());
    }
    match std::env::var("TERM") {
        Ok(term) if term == "dumb" || term.is_empty() => {
            Err("mediaops-tui refuses TERM=dumb".into())
        }
        Err(_) => Err("mediaops-tui refuses a missing TERM".into()),
        Ok(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;

    #[test]
    fn help_and_version_are_clap_success() {
        let mut cmd = Args::command();
        let help = cmd.render_help().to_string();
        assert!(help.contains("--api-socket"));
        assert!(help.contains("--color"));
        let err = Args::try_parse_from(["mediaops-tui", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        let err = Args::try_parse_from(["mediaops-tui", "--version"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    #[test]
    fn color_never_and_socket_parse() {
        let args = Args::try_parse_from([
            "mediaops-tui",
            "--color",
            "never",
            "--api-socket",
            "/tmp/api.sock",
        ])
        .expect("parse");
        assert_eq!(args.color, ColorMode::Never);
        assert!(!args.color_enabled());
        assert_eq!(
            args.api_socket.as_deref(),
            Some(std::path::Path::new("/tmp/api.sock"))
        );
    }
}
