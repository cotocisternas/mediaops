use std::io::{self, IsTerminal, Write};

use anyhow::anyhow;
use clap::Parser;
use clap::error::ErrorKind;
use mediaops_core::ExitCode;

const BIN_NAME: &str = "mediaopsd";

#[derive(Parser, Debug)]
#[command(name = BIN_NAME, version)]
struct Cli {
    /// Emit a single JSON envelope on stdout.
    #[arg(long)]
    json: bool,
}

enum AppError {
    Usage(String),
    Runtime(anyhow::Error),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(message) => write!(f, "{message}"),
            Self::Runtime(err) => write!(f, "{err}"),
        }
    }
}

enum ParseOutcome {
    Parsed(Cli),
    HelpOrVersion,
}

fn json_token_requests_json(arg: &str) -> bool {
    if arg == "--json" {
        return true;
    }
    let Some(value) = arg.strip_prefix("--json=") else {
        return false;
    };
    matches!(
        value.to_ascii_lowercase().as_str(),
        "true" | "t" | "yes" | "y" | "1" | "on"
    )
}

fn json_requested() -> bool {
    std::env::args_os().any(|arg| arg.to_str().is_some_and(json_token_requests_json))
}

fn init_tracing() {
    let subscriber = tracing_subscriber::fmt().with_writer(io::stderr);
    if io::stderr().is_terminal() {
        subscriber.init();
    } else {
        subscriber.json().init();
    }
}

fn to_exit_code(err: &AppError) -> ExitCode {
    match err {
        AppError::Usage(_) => ExitCode::Usage,
        AppError::Runtime(_) => ExitCode::Runtime,
    }
}

fn write_stdout(line: &str) -> Result<(), AppError> {
    let mut out = io::stdout().lock();
    writeln!(out, "{line}").map_err(|e| AppError::Runtime(e.into()))?;
    out.flush().map_err(|e| AppError::Runtime(e.into()))?;
    Ok(())
}

fn emit_success(json: bool) -> Result<(), AppError> {
    let version = env!("CARGO_PKG_VERSION");
    let line = if json {
        mediaops_core::render_success_json(BIN_NAME, version)
            .map_err(|e| AppError::Runtime(anyhow!(e)))?
    } else {
        mediaops_core::identity_line(BIN_NAME, version)
    };
    write_stdout(&line)
}

fn emit_error(json: bool, code: ExitCode, err: &AppError) -> Result<(), AppError> {
    if json {
        let line = mediaops_core::render_error_json(code, &err.to_string())
            .map_err(|e| AppError::Runtime(anyhow!(e)))?;
        write_stdout(&line)?;
    }
    Ok(())
}

fn finish_error(json_flag: bool, err: &AppError) -> ExitCode {
    let code = to_exit_code(err);
    if !json_flag {
        tracing::error!(error = %err, "command failed");
    }
    if let Err(emit_err) = emit_error(json_flag, code, err) {
        tracing::error!(error = %emit_err, "failed to emit error output");
    }
    code
}

fn parse_cli(json_flag: bool) -> Result<ParseOutcome, AppError> {
    match Cli::try_parse() {
        Ok(cli) => Ok(ParseOutcome::Parsed(cli)),
        Err(err) => match err.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                err.print().map_err(|e| AppError::Runtime(anyhow!(e)))?;
                Ok(ParseOutcome::HelpOrVersion)
            }
            _ => {
                if !json_flag {
                    err.print().map_err(|e| AppError::Runtime(anyhow!(e)))?;
                }
                Err(AppError::Usage(err.to_string()))
            }
        },
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    tracing::info!(bin = BIN_NAME, "start");

    let json_flag = json_requested();
    match parse_cli(json_flag) {
        Ok(ParseOutcome::HelpOrVersion) => ExitCode::Ok,
        Ok(ParseOutcome::Parsed(cli)) => match emit_success(cli.json) {
            Ok(()) => ExitCode::Ok,
            Err(err) => finish_error(json_flag || cli.json, &err),
        },
        Err(err) => finish_error(json_flag, &err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_token_matches_clap_boolish_true() {
        assert!(json_token_requests_json("--json"));
        assert!(json_token_requests_json("--json=true"));
        assert!(json_token_requests_json("--json=TRUE"));
        assert!(json_token_requests_json("--json=1"));
        assert!(!json_token_requests_json("--json=false"));
        assert!(!json_token_requests_json("--json=0"));
        assert!(!json_token_requests_json("--json=maybe"));
        assert!(!json_token_requests_json("--help"));
    }

    #[test]
    fn runtime_maps_to_exit_1() {
        let err = AppError::Runtime(anyhow!("stdout closed"));
        assert_eq!(to_exit_code(&err), ExitCode::Runtime);
        assert_eq!(i32::from(to_exit_code(&err)), 1);
    }

    #[test]
    fn usage_maps_to_exit_2() {
        let err = AppError::Usage("unexpected argument".into());
        assert_eq!(to_exit_code(&err), ExitCode::Usage);
        assert_eq!(i32::from(to_exit_code(&err)), 2);
    }
}
