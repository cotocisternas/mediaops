use std::io::{self, IsTerminal, Write};
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::anyhow;
use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand};
use mediaops_core::{Allowlist, ExitCode, Grabber};
use mediaops_net::{DaemonRole, IdentityBundle, Seedbox, serve_tcp};

const BIN_NAME: &str = "mediaopsd";

#[derive(Parser, Debug)]
#[command(name = BIN_NAME, version)]
struct Cli {
    /// Emit a single JSON envelope on stdout.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Bind Control + Transfer (seedbox role).
    Serve(ServeArgs),
}

#[derive(Args, Debug)]
struct ServeArgs {
    #[arg(long, default_value = "seedbox")]
    role: String,
    #[arg(long, default_value = "0.0.0.0:50051")]
    bind: String,
    #[arg(long)]
    tls_dir: PathBuf,
    /// Allowlisted root as `id=path`. Repeatable.
    #[arg(long = "root", value_parser = parse_root)]
    roots: Vec<(String, PathBuf)>,
}

fn parse_root(raw: &str) -> Result<(String, PathBuf), String> {
    let (id, path) = raw
        .split_once('=')
        .ok_or_else(|| "expected id=path".to_string())?;
    if id.is_empty() {
        return Err("empty root id".into());
    }
    Ok((id.to_string(), PathBuf::from(path)))
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

async fn serve(args: ServeArgs) -> Result<(), AppError> {
    let role = DaemonRole::parse(&args.role).map_err(|err| AppError::Usage(err.to_string()))?;
    role.ensure_seedbox()
        .map_err(|err| AppError::Usage(err.to_string()))?;
    if args.roots.is_empty() {
        return Err(AppError::Usage(
            "serve requires at least one --root id=path".into(),
        ));
    }
    let mut allowlist = Allowlist::new();
    for (id, path) in args.roots {
        allowlist
            .add_root(id, path)
            .map_err(|err| AppError::Runtime(anyhow!(err)))?;
    }
    let identity =
        IdentityBundle::from_dir(&args.tls_dir).map_err(|err| AppError::Runtime(anyhow!(err)))?;
    let server = identity
        .server_config()
        .map_err(|err| AppError::Runtime(anyhow!(err)))?;
    let bind: SocketAddr = args
        .bind
        .parse()
        .map_err(|err| AppError::Usage(format!("bad --bind: {err}")))?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| AppError::Runtime(e.into()))?;
    let addr = listener
        .local_addr()
        .map_err(|e| AppError::Runtime(e.into()))?;
    tracing::info!(%addr, "seedbox listen");
    let seedbox = Seedbox::new(allowlist, env!("CARGO_PKG_VERSION"), Grabber::None);
    serve_tcp(listener, server, seedbox)
        .await
        .map_err(|err| AppError::Runtime(anyhow!(err)))
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    tracing::info!(bin = BIN_NAME, "start");

    let json_flag = json_requested();
    match parse_cli(json_flag) {
        Ok(ParseOutcome::HelpOrVersion) => ExitCode::Ok,
        Ok(ParseOutcome::Parsed(cli)) => {
            let json = json_flag || cli.json;
            let result = match cli.command {
                None => emit_success(cli.json),
                Some(Command::Serve(args)) => serve(args).await,
            };
            match result {
                Ok(()) => ExitCode::Ok,
                Err(err) => finish_error(json, &err),
            }
        }
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
        assert!(!json_token_requests_json("--json=false"));
        assert!(!json_token_requests_json("--help"));
    }

    #[test]
    fn runtime_maps_to_exit_1() {
        let err = AppError::Runtime(anyhow!("stdout closed"));
        assert_eq!(to_exit_code(&err), ExitCode::Runtime);
    }

    #[test]
    fn usage_maps_to_exit_2() {
        let err = AppError::Usage("unexpected argument".into());
        assert_eq!(to_exit_code(&err), ExitCode::Usage);
    }

    #[test]
    fn parse_root_splits_id_and_path() {
        assert_eq!(
            parse_root("seedbox=/data/media").expect("root"),
            ("seedbox".into(), PathBuf::from("/data/media"))
        );
        assert!(parse_root("nopath").is_err());
    }
}
