use std::io::{self, IsTerminal, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

use anyhow::anyhow;
use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand};
use mediaops_core::{
    Allowlist, DesiredState, ExitCode, Grabber, UnderlayMode, endpoint_fingerprint,
};
use mediaops_net::{DaemonRole, HomeGateway, IdentityBundle, Seedbox, serve_home_unix, serve_tcp};
use tokio::net::{UnixListener, UnixStream};

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
    /// Home role UDS path. Ignored for seedbox.
    #[arg(long)]
    socket: Option<PathBuf>,
    #[arg(long)]
    tls_dir: PathBuf,
    /// Home role: seedbox `HOST:PORT`. Alternative to `--desired-state`.
    #[arg(long)]
    upstream: Option<String>,
    /// Home role: read `seedbox_address` + underlay from desired-state.
    #[arg(long)]
    desired_state: Option<PathBuf>,
    /// Allowlisted root as `id=path`. Repeatable. Seedbox role only.
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

fn default_home_socket() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|dir| PathBuf::from(dir).join("mediaopsd.sock"))
        .unwrap_or_else(|| default_state_dir().join("mediaopsd.sock"))
}

fn default_state_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| {
            b.state_dir()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| b.home_dir().join(".local").join("state"))
                .join("mediaops")
        })
        .unwrap_or_else(|| PathBuf::from(".mediaops-state"))
}

fn parse_grpc_addr(raw: &str) -> Result<SocketAddr, AppError> {
    if let Ok(addr) = raw.parse::<SocketAddr>() {
        return Ok(addr);
    }
    raw.to_socket_addrs()
        .map_err(|err| AppError::Usage(format!("bad seedbox address `{raw}`: {err}")))?
        .next()
        .ok_or_else(|| AppError::Usage(format!("seedbox address `{raw}` did not resolve")))
}

fn resolve_upstream(args: &ServeArgs) -> Result<(String, SocketAddr, UnderlayMode), AppError> {
    if let Some(raw) = args.upstream.as_deref() {
        let addr = parse_grpc_addr(raw)?;
        return Ok((raw.to_string(), addr, UnderlayMode::Direct));
    }
    let path = args.desired_state.as_ref().ok_or_else(|| {
        AppError::Usage("home role requires --upstream HOST:PORT or --desired-state".into())
    })?;
    let text = std::fs::read_to_string(path).map_err(|err| AppError::Runtime(err.into()))?;
    let ds = DesiredState::from_toml(&text).map_err(|err| AppError::Runtime(anyhow!(err)))?;
    let raw = ds.seedbox_address().ok_or_else(|| {
        AppError::Usage("desired-state has no seedbox_address; pass --upstream".into())
    })?;
    let addr = parse_grpc_addr(raw)?;
    Ok((raw.to_string(), addr, ds.underlay()))
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
    let seedbox = Seedbox::new(allowlist, env!("CARGO_PKG_VERSION"), Grabber::None);
    serve_tcp(listener, server, seedbox)
        .await
        .map_err(|err| AppError::Runtime(anyhow!(err)))
}

async fn serve_home(args: ServeArgs) -> Result<(), AppError> {
    let (raw, upstream, underlay) = resolve_upstream(&args)?;
    let identity =
        IdentityBundle::from_dir(&args.tls_dir).map_err(|err| AppError::Runtime(anyhow!(err)))?;
    let server = identity
        .server_config()
        .map_err(|err| AppError::Runtime(anyhow!(err)))?;
    let client = identity
        .client_config()
        .map_err(|err| AppError::Runtime(anyhow!(err)))?;
    let fingerprint = endpoint_fingerprint(&raw, underlay);
    let gateway = HomeGateway::connect(upstream, client, fingerprint, 1)
        .await
        .map_err(|err| AppError::Runtime(anyhow!(err)))?;
    let socket = args.socket.unwrap_or_else(default_home_socket);
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent).map_err(|err| AppError::Runtime(err.into()))?;
    }
    if Path::new(&socket).exists() {
        match UnixStream::connect(&socket).await {
            Ok(_) => {
                return Err(AppError::Runtime(anyhow!(
                    "home socket {} is live; not replacing a running gateway",
                    socket.display()
                )));
            }
            Err(_) => {
                std::fs::remove_file(&socket).map_err(|err| AppError::Runtime(err.into()))?;
            }
        }
    }
    let listener = UnixListener::bind(&socket).map_err(|e| AppError::Runtime(e.into()))?;
    tracing::info!(socket = %socket.display(), upstream = %upstream, "home gateway listen");
    serve_home_unix(listener, server, gateway)
        .await
        .map_err(|err| AppError::Runtime(anyhow!(err)))
}

async fn serve(args: ServeArgs) -> Result<(), AppError> {
    let role = DaemonRole::parse(&args.role).map_err(|err| AppError::Usage(err.to_string()))?;
    match role {
        DaemonRole::Seedbox => serve_seedbox(args).await,
        DaemonRole::Home => serve_home(args).await,
        DaemonRole::ReverseConnect => Err(AppError::Usage(
            "role `reverse-connect` is a designed-unused mode of this binary".into(),
        )),
    }
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
