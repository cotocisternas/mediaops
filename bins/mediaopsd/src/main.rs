use std::io::{self, IsTerminal, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::anyhow;
use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand};
use mediaops_arr::{KeyPaths, LocalhostGrabOps, ReqwestTransport};
use mediaops_core::{Allowlist, DesiredState, ExitCode, GrabOps, Grabber};
use mediaops_net::{DaemonRole, IdentityBundle, Seedbox, serve_tcp};
use std::sync::Arc;

const BIN_NAME: &str = "mediaopsd";

#[derive(Parser, Debug)]
#[command(name = BIN_NAME, version)]
struct Cli {
    /// Emit a raw JSON identity or error on stdout.
    #[arg(short = 'o', long = "output", global = true, value_parser = ["json"])]
    output: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Bind ControlService + TransferService (seedbox role).
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
    /// Seedbox: read grabber from config.toml.
    #[arg(long = "config", value_name = "PATH")]
    desired_state: Option<PathBuf>,
    /// Allowlisted root as `id=path`. Repeatable. Seedbox role only.
    #[arg(long = "root", value_parser = parse_root)]
    roots: Vec<(String, PathBuf)>,
    /// Nginx app conf dir for panel fingerprint (seedbox). Tests use a tempdir.
    #[arg(long)]
    nginx_dir: Option<PathBuf>,
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

#[derive(Debug)]
enum AppError {
    Usage(String),
    Runtime(anyhow::Error),
    Emitted(ExitCode),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(message) => write!(f, "{message}"),
            Self::Runtime(err) => write!(f, "{err}"),
            Self::Emitted(_) => write!(f, "error already printed"),
        }
    }
}

enum ParseOutcome {
    Parsed(Cli),
    HelpOrVersion,
}

fn args_request_json(args: impl IntoIterator<Item = String>) -> bool {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if (arg == "-o" || arg == "--output") && args.next().as_deref() == Some("json") {
            return true;
        }
        if arg == "--output=json" || arg == "-ojson" || arg == "-o=json" {
            return true;
        }
    }
    false
}

fn json_requested() -> bool {
    args_request_json(
        std::env::args_os()
            .skip(1)
            .filter_map(|arg| arg.into_string().ok()),
    )
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
        AppError::Emitted(code) => *code,
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
    if matches!(err, AppError::Emitted(_)) {
        return code;
    }
    if !json_flag {
        eprintln!("error: {err}");
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
                    return Err(AppError::Emitted(ExitCode::Usage));
                }
                Err(AppError::Usage(err.to_string()))
            }
        },
    }
}

async fn serve_seedbox(args: ServeArgs) -> Result<(), AppError> {
    if args.roots.is_empty() {
        return Err(AppError::Usage(
            "serve --role seedbox requires at least one --root id=path".into(),
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
    let (grabber, grab_ops) = seedbox_grab_ops(args.desired_state.as_deref())?;
    let mut seedbox =
        Seedbox::new(allowlist, env!("CARGO_PKG_VERSION"), grabber).with_grab_ops(grab_ops);
    if let Some(dir) = args.nginx_dir {
        seedbox = seedbox.with_nginx_dir(dir);
    }
    serve_tcp(listener, server, seedbox)
        .await
        .map_err(|err| AppError::Runtime(anyhow!(err)))
}

fn seedbox_grab_ops(
    desired_state: Option<&Path>,
) -> Result<(Grabber, Option<Arc<dyn GrabOps>>), AppError> {
    let Some(path) = desired_state else {
        return Ok((Grabber::None, None));
    };
    let text = std::fs::read_to_string(path).map_err(|err| AppError::Runtime(err.into()))?;
    let ds = DesiredState::from_toml(&text).map_err(|err| AppError::Runtime(anyhow!(err)))?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Runtime(anyhow!("HOME unset")))?;
    let transport =
        ReqwestTransport::new().map_err(|err| AppError::Runtime(anyhow!(err.to_string())))?;
    let ops = LocalhostGrabOps::new(transport, KeyPaths::from_home(&home), &ds);
    Ok((ds.grabber(), Some(Arc::new(ops))))
}

async fn serve(args: ServeArgs) -> Result<(), AppError> {
    let role = DaemonRole::parse(&args.role).map_err(|err| AppError::Usage(err.to_string()))?;
    match role {
        DaemonRole::Seedbox => serve_seedbox(args).await,
        DaemonRole::Home => Err(AppError::Usage(
            "role `home` moved to mediaops-gateway; start mediaops-home".into(),
        )),
        DaemonRole::ReverseConnect => Err(AppError::Usage(
            "role `reverse-connect` is a designed-unused mode of this binary".into(),
        )),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    let json_flag = json_requested();
    match parse_cli(json_flag) {
        Ok(ParseOutcome::HelpOrVersion) => ExitCode::Ok,
        Ok(ParseOutcome::Parsed(cli)) => {
            let json = json_flag || cli.output.as_deref() == Some("json");
            let result = match cli.command {
                None => emit_success(json),
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
    fn output_json_is_detected_before_reporting_parse_errors() {
        let requested =
            |args: &[&str]| args_request_json(args.iter().map(|arg| (*arg).to_string()));
        assert!(requested(&["--bad", "-o", "json"]));
        assert!(requested(&["--output=json"]));
        assert!(requested(&["-ojson"]));
        assert!(requested(&["-o=json"]));
        assert!(!requested(&["--json"]));
        assert!(!requested(&["--", "-o", "json"]));
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
        assert!(parse_root("=/tmp").is_err());
    }

    fn serve_args() -> ServeArgs {
        ServeArgs {
            role: "seedbox".into(),
            bind: "127.0.0.1:0".into(),
            tls_dir: PathBuf::from("/tmp"),
            desired_state: None,
            roots: Vec::new(),
            nginx_dir: None,
        }
    }

    #[tokio::test]
    async fn serve_reverse_connect_and_seedbox_without_root_are_usage() {
        let mut args = serve_args();
        args.role = "reverse-connect".into();
        let err = serve(args).await.expect_err("reverse");
        assert!(
            matches!(err, AppError::Usage(ref m) if m.contains("reverse-connect")),
            "{err}"
        );

        let err = serve(serve_args()).await.expect_err("no root");
        assert!(
            matches!(err, AppError::Usage(ref m) if m.contains("--root")),
            "{err}"
        );
    }
}
