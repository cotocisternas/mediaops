use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::anyhow;
use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand};
use mediaops_core::{ExitCode, ProviderKind};
use mediaops_ssh::SystemExec;

mod api_cmd;
mod apply_cmd;
mod bootstrap;
mod doctor;
mod encode_cmd;
mod home;
mod home_library;
mod library;
mod new_machine;
mod out;
mod progress;
mod reclaim;
mod repair;
mod sync_cmd;
mod sync_format;

#[cfg(test)]
mod test_support;

const BIN_NAME: &str = "mediaops";

#[derive(Parser, Debug)]
#[command(name = BIN_NAME, version)]
struct Cli {
    /// Output: table (default), wide, or json (raw object).
    #[arg(short = 'o', long = "output", global = true)]
    output: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    Seedbox(SeedboxArgs),
    /// List allowlisted remotes through the home unix-socket gateway.
    List {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
        #[arg(long)]
        config_dir: Option<PathBuf>,
    },
    /// Pull one remote file into `_incoming/` with `.partial` resume.
    Pull {
        #[arg(long)]
        root: String,
        #[arg(long)]
        path: PathBuf,
        #[arg(long)]
        title_id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        library_root: Option<PathBuf>,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long)]
        install: bool,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        year: Option<u16>,
        #[arg(long)]
        season: Option<u8>,
        #[arg(long)]
        episode: Option<u8>,
    },
    Library(LibraryArgs),
    /// Record a Want and exit. Does not wait for playable.
    Watch {
        title: String,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Show this title's recorded Want, Hold, Job, and library facts.
    Why {
        title: String,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Show Home activity, worker readiness, and library disk space.
    Status {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    Encode(EncodeArgs),
    /// Ranked dry-run / exclusive unlink of surplus remotes after install_b3 proof.
    Reclaim(ReclaimArgs),
    /// Import-blocked inbox (lock-free).
    Hold(HoldArgs),
    /// Export/import config, credentials, Home runtime objects, and file proofs.
    NewMachine(NewMachineArgs),
    /// Check edge, credentials, PEM locations, and default Home worker readiness.
    Doctor {
        #[arg(long)]
        repair: bool,
        #[arg(long)]
        confirm: bool,
        #[arg(long)]
        pin: Option<PathBuf>,
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long)]
        state_db: Option<PathBuf>,
        #[arg(long = "api-socket")]
        api_socket: Option<PathBuf>,
    },
    Repair(RepairArgs),
    /// Get one Home object or list a kind. `-o json` is the raw object.
    Get {
        kind: String,
        name: Option<String>,
        #[arg(long)]
        watch: bool,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Apply a Cluster/Secret/Want/Title document (TOML or JSON).
    Apply {
        #[arg(short = 'f', long)]
        file: PathBuf,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Delete one Home object.
    Delete {
        kind: String,
        name: String,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Increment Cluster.status.reconcileGeneration and request reconciliation.
    Reconcile {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Copy completed eligible seedbox files home. `--dry-run` previews without writing.
    Sync {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        request_id: Option<String>,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

#[derive(Args, Debug)]
struct RepairArgs {
    #[command(subcommand)]
    command: RepairCommand,
}

#[derive(Subcommand, Debug)]
enum RepairCommand {
    /// Confirmed nginx + API edge transaction.
    Edge {
        #[arg(long)]
        repair: bool,
        #[arg(long)]
        confirm: bool,
        #[arg(long)]
        pin: Option<PathBuf>,
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long)]
        state_db: Option<PathBuf>,
        #[arg(long)]
        ssh_config: Option<PathBuf>,
    },
}

#[derive(Args, Debug)]
struct EncodeArgs {
    #[command(subcommand)]
    command: EncodeCommand,
}

#[derive(Args, Debug)]
struct ReclaimArgs {
    #[command(subcommand)]
    command: ReclaimCommand,
}

#[derive(Subcommand, Debug)]
enum ReclaimCommand {
    /// Ranked dry-run of surplus remotes. Lock-free.
    Preview {
        #[arg(long)]
        library_root: Option<PathBuf>,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
        #[arg(long)]
        config_dir: Option<PathBuf>,
    },
    /// Unlink ranked surplus remotes. Exclusive flock.
    Apply {
        #[arg(long)]
        library_root: Option<PathBuf>,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        /// Delete at most N ranked candidates.
        #[arg(long)]
        max: Option<usize>,
    },
}

#[derive(Args, Debug)]
struct HoldArgs {
    #[command(subcommand)]
    command: HoldCommand,
}

#[derive(Args, Debug)]
struct NewMachineArgs {
    #[command(subcommand)]
    command: NewMachineCommand,
}

#[derive(Subcommand, Debug)]
enum NewMachineCommand {
    /// Write config, TLS, file proofs, and Home runtime objects into a private bundle.
    Export {
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
    },
    /// Restore a bundle into Home state, resuming compatible Title proofs.
    Import {
        #[arg(long)]
        from: PathBuf,
        #[arg(long)]
        library_root: PathBuf,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum HoldCommand {
    /// List undecided import-blocked releases.
    List {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Record approval; the Hold controller creates a Pull Job to install it.
    Approve {
        /// Title id or row number from `hold list`.
        target: String,
        /// Release id when the same title has several held releases.
        release_id: Option<String>,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Reject a held release and notify the grabber.
    Reject {
        /// Title id or row number from `hold list`.
        target: String,
        release_id: Option<String>,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum EncodeCommand {
    /// Classify movies/ under EncodePolicy. Lock-free.
    Scan {
        #[arg(long)]
        library_root: Option<PathBuf>,
    },
    /// Run ready encode jobs, or one title.
    Run {
        title: Option<String>,
        #[arg(long)]
        library_root: Option<PathBuf>,
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
        #[arg(long)]
        config_dir: Option<PathBuf>,
    },
    /// Set or clear Cluster.spec.encodePause.
    Pause {
        #[arg(long)]
        off: bool,
    },
}

#[derive(Args, Debug)]
struct SeedboxArgs {
    #[command(subcommand)]
    command: SeedboxCommand,
}

#[derive(Args, Debug)]
struct LibraryArgs {
    #[command(subcommand)]
    command: LibraryCommand,
}

#[derive(Subcommand, Debug)]
enum LibraryCommand {
    /// Create schema dirs, lock, sqlite, NVENC probe, systemd-user units.
    Bootstrap {
        #[arg(long)]
        library_root: PathBuf,
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        /// Enable and start the always-on Home service.
        #[arg(long = "enable-service")]
        enable_timer: bool,
        #[arg(long)]
        unit_dir: Option<PathBuf>,
    },
    /// Retarget the one home library: layout, `library_root`, units. Does not copy media.
    Relocate {
        #[arg(long)]
        library_root: PathBuf,
        /// Enable and start the always-on Home service.
        #[arg(long = "enable-service")]
        enable_timer: bool,
        #[arg(long)]
        unit_dir: Option<PathBuf>,
    },
    /// Rebuild title-index proof by hashing on-disk schema files.
    Reindex {
        #[arg(long)]
        library_root: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum SeedboxCommand {
    /// Re-run bootstrap install step: copy musl mediaopsd + restart unit.
    Upgrade {
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
        #[arg(long)]
        ssh_config: Option<PathBuf>,
        #[arg(long)]
        state_db: Option<PathBuf>,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
    },
    /// Apply grabber indexer/client sets from config.toml (Control GrabApply).
    Apply {
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long)]
        state_db: Option<PathBuf>,
    },
    /// Install mediaopsd on Host seedbox and mint mTLS (destructive; needs --yes).
    Bootstrap {
        #[arg(long, default_value = "already-there")]
        provider: String,
        /// Actually mint, copy, and probe. Without this, print the plan and refuse.
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
        #[arg(long)]
        ssh_config: Option<PathBuf>,
        #[arg(long)]
        state_db: Option<PathBuf>,
        #[arg(long)]
        address: Option<String>,
        #[arg(long)]
        skip_probe: bool,
        /// Allowlisted root as `id=path`. Repeatable. Required for SwizzinBox.
        #[arg(long = "root", value_parser = bootstrap::parse_root)]
        roots: Vec<(String, PathBuf)>,
    },
}

#[derive(Debug)]
pub(crate) enum AppError {
    Usage(String),
    Runtime(anyhow::Error),
    Policy(String),
    LockConflict(String),
    DriftVerify(String),
    Emitted(ExitCode),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(message)
            | Self::Policy(message)
            | Self::LockConflict(message)
            | Self::DriftVerify(message) => write!(f, "{message}"),
            Self::Runtime(err) => write!(f, "{err}"),
            Self::Emitted(_) => write!(f, "already emitted"),
        }
    }
}

enum ParseOutcome {
    Parsed(Cli),
    HelpOrVersion,
}

fn args_request_json(args: impl IntoIterator<Item = String>) -> bool {
    let mut output_value = false;
    for arg in args {
        if arg == "--" {
            break;
        }
        if output_value && arg == "json" {
            return true;
        }
        output_value = arg == "-o" || arg == "--output";
        if arg == "--output=json" || arg == "-ojson" || arg == "-o=json" {
            return true;
        }
    }
    false
}

fn json_requested() -> bool {
    args_request_json(std::env::args().skip(1))
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
        AppError::Policy(_) => ExitCode::PolicyRefusal,
        AppError::LockConflict(_) => ExitCode::LockConflict,
        AppError::DriftVerify(_) => ExitCode::DriftVerify,
        AppError::Emitted(code) => *code,
    }
}

fn write_stdout(line: &str) -> Result<(), AppError> {
    let normalized = serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .filter(|value| {
            value.get("ok").is_some_and(serde_json::Value::is_boolean)
                && value.get("data").is_some()
                && value.get("error").is_some()
        })
        .map(|value| {
            if value["ok"] == true {
                value["data"].to_string()
            } else {
                let mut error = serde_json::json!({"error": value["error"]});
                if !value["data"].is_null() {
                    error["report"] = value["data"].clone();
                }
                error.to_string()
            }
        });
    let line = normalized.as_deref().unwrap_or(line);
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

async fn run(cli: Cli) -> Result<(), AppError> {
    let output = api_cmd::Output::parse(cli.output.as_deref())?;
    let label = match &cli.command {
        Some(Command::Seedbox(_)) => Some("seedbox maintenance"),
        Some(Command::Library(LibraryArgs {
            command: LibraryCommand::Bootstrap { .. },
        })) => Some("library bootstrap"),
        Some(Command::Library(LibraryArgs {
            command: LibraryCommand::Relocate { .. },
        })) => Some("library relocate"),
        Some(Command::Encode(EncodeArgs {
            command: EncodeCommand::Scan { .. },
        })) => Some("encode scan"),
        Some(Command::Encode(EncodeArgs {
            command: EncodeCommand::Run { .. },
        })) => Some("encode run"),
        Some(Command::Encode(EncodeArgs {
            command: EncodeCommand::Pause { .. },
        })) => Some("encode pause"),
        Some(Command::Reclaim(_)) => Some("reclaim"),
        Some(Command::NewMachine(_)) => Some("new-machine"),
        Some(Command::Doctor { .. }) => Some("doctor"),
        Some(Command::Repair(_)) => Some("edge repair"),
        Some(Command::List { .. }) => Some("list remote files"),
        Some(Command::Status { .. }) => Some("Home status"),
        Some(Command::Why { .. }) => Some("title status"),
        Some(Command::Hold(_)) => Some("hold"),
        Some(Command::Sync { .. }) => Some("sync"),
        Some(Command::Get { watch: false, .. }) => Some("get Home objects"),
        Some(Command::Apply { .. }) => Some("apply Home object"),
        Some(Command::Delete { .. }) => Some("delete Home object"),
        Some(Command::Watch { .. }) => Some("record Want"),
        Some(Command::Reconcile { .. }) => Some("reconcile"),
        _ => None,
    };
    let mut progress = label
        .map(|label| progress::OperationProgress::lines(output != api_cmd::Output::Json, label));
    if let Some(progress) = progress.as_mut() {
        progress.stage("working", "");
    }
    let result = dispatch(cli, output, &mut progress).await;
    if result.is_ok()
        && let Some(progress) = progress.as_mut()
    {
        progress.finish();
    }
    result
}

fn finish_output(
    line: &str,
    progress: &mut Option<progress::OperationProgress>,
) -> Result<(), AppError> {
    if let Some(progress) = progress.as_mut() {
        progress.finish();
    }
    write_stdout(line)
}

async fn dispatch(
    cli: Cli,
    output: api_cmd::Output,
    progress: &mut Option<progress::OperationProgress>,
) -> Result<(), AppError> {
    let json = output == api_cmd::Output::Json;
    match cli.command {
        None => emit_success(json),
        Some(Command::Seedbox(SeedboxArgs {
            command:
                SeedboxCommand::Upgrade {
                    yes,
                    config_dir,
                    desired_state,
                    ssh_config,
                    state_db,
                    socket,
                    tls_dir,
                },
        })) => {
            let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
            let args = bootstrap::UpgradeArgs {
                yes,
                desired_state: desired_state
                    .unwrap_or_else(|| bootstrap::default_desired_state(&config_dir)),
                ssh_config: ssh_config.unwrap_or_else(bootstrap::default_ssh_config),
                state_db: state_db.unwrap_or_else(bootstrap::default_state_db),
                socket: socket.unwrap_or_else(bootstrap::default_socket),
                tls_dir: tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir)),
                config_dir,
                skip_edge: false,
            };
            match bootstrap::upgrade(args, &SystemExec).await {
                Ok(report) => {
                    let line = bootstrap::render_upgrade(json, &report)
                        .map_err(|e| AppError::Runtime(anyhow!(e)))?;
                    finish_output(&line, progress)
                }
                Err(err) => {
                    let mapped = match err.exit_code() {
                        ExitCode::Usage => AppError::Usage(err.to_string()),
                        ExitCode::PolicyRefusal => AppError::Policy(err.to_string()),
                        ExitCode::LockConflict => AppError::LockConflict(err.to_string()),
                        _ => AppError::Runtime(anyhow!(err.to_string())),
                    };
                    Err(mapped)
                }
            }
        }
        Some(Command::Seedbox(SeedboxArgs {
            command:
                SeedboxCommand::Apply {
                    desired_state,
                    socket,
                    tls_dir,
                    config_dir,
                    state_db,
                },
        })) => {
            let line = apply_cmd::seedbox_apply(
                json,
                desired_state,
                socket,
                tls_dir,
                config_dir,
                state_db,
            )
            .await?;
            finish_output(&line, progress)
        }
        Some(Command::Seedbox(SeedboxArgs {
            command:
                SeedboxCommand::Bootstrap {
                    provider,
                    yes,
                    config_dir,
                    desired_state,
                    ssh_config,
                    state_db,
                    address,
                    skip_probe,
                    roots,
                },
        })) => {
            let provider =
                ProviderKind::parse(&provider).map_err(|err| AppError::Usage(err.to_string()))?;
            let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
            let desired_state =
                desired_state.unwrap_or_else(|| bootstrap::default_desired_state(&config_dir));
            let args = bootstrap::BootstrapArgs {
                provider,
                yes,
                desired_state,
                ssh_config: ssh_config.unwrap_or_else(bootstrap::default_ssh_config),
                state_db: state_db.unwrap_or_else(bootstrap::default_state_db),
                config_dir,
                address,
                skip_probe,
                roots,
                socket: bootstrap::default_socket(),
                skip_edge: false,
            };
            match bootstrap::bootstrap(args, &SystemExec).await {
                Ok(report) => {
                    let line = bootstrap::render_report(json, &report)
                        .map_err(|e| AppError::Runtime(anyhow!(e)))?;
                    finish_output(&line, progress)
                }
                Err(bootstrap::BootstrapError::NeedsConfirm(report)) => {
                    let line = bootstrap::render_needs_confirm(json, &report)
                        .map_err(|e| AppError::Runtime(anyhow!(e)))?;
                    drop(progress.take());
                    write_stdout(&line)?;
                    Err(AppError::Emitted(ExitCode::PolicyRefusal))
                }
                Err(err) => {
                    let mapped = match err.exit_code() {
                        ExitCode::Usage => AppError::Usage(err.to_string()),
                        ExitCode::PolicyRefusal => AppError::Policy(err.to_string()),
                        ExitCode::LockConflict => AppError::LockConflict(err.to_string()),
                        _ => AppError::Runtime(anyhow!(err.to_string())),
                    };
                    Err(mapped)
                }
            }
        }
        Some(Command::List {
            socket,
            tls_dir,
            config_dir,
        }) => {
            let line = home::list(json, socket, tls_dir, config_dir).await?;
            finish_output(&line, progress)
        }
        Some(Command::Pull {
            root,
            path,
            title_id,
            name,
            library_root,
            socket,
            tls_dir,
            config_dir,
            install,
            title,
            year,
            season,
            episode,
        }) => {
            let line = home::pull(
                json,
                root,
                path,
                title_id,
                name,
                library_root,
                socket,
                tls_dir,
                config_dir,
                None,
                install,
                title,
                year,
                season,
                episode,
            )
            .await?;
            finish_output(&line, progress)
        }
        Some(Command::Library(LibraryArgs {
            command:
                LibraryCommand::Bootstrap {
                    library_root,
                    desired_state,
                    config_dir,
                    enable_timer,
                    unit_dir,
                },
        })) => {
            let line = library::bootstrap_library(
                json,
                library_root,
                desired_state,
                config_dir,
                None,
                enable_timer,
                unit_dir,
            )
            .await?;
            finish_output(&line, progress)
        }
        Some(Command::Library(LibraryArgs {
            command:
                LibraryCommand::Relocate {
                    library_root,
                    enable_timer,
                    unit_dir,
                },
        })) => {
            let line =
                library::relocate_library(json, library_root, None, enable_timer, unit_dir).await?;
            finish_output(&line, progress)
        }
        Some(Command::Library(LibraryArgs {
            command: LibraryCommand::Reindex { library_root },
        })) => {
            let line = library::reindex_library(json, library_root, None).await?;
            finish_output(&line, progress)
        }
        Some(Command::Watch { title, socket }) => finish_output(
            &api_cmd::watch_title(title, output, socket).await?,
            progress,
        ),
        Some(Command::Why { title, socket }) => {
            finish_output(&api_cmd::why_pretty(title, output, socket).await?, progress)
        }
        Some(Command::Status { socket }) => {
            finish_output(&api_cmd::status_pretty(output, socket).await?, progress)
        }
        Some(Command::Encode(EncodeArgs {
            command: EncodeCommand::Scan { library_root },
        })) => {
            let line = encode_cmd::scan(&SystemExec, json, library_root).await?;
            finish_output(&line, progress)
        }
        Some(Command::Encode(EncodeArgs {
            command:
                EncodeCommand::Run {
                    title,
                    library_root,
                    desired_state,
                    config_dir,
                },
        })) => {
            let line = encode_cmd::run(
                &SystemExec,
                json,
                title,
                None,
                library_root,
                desired_state,
                config_dir,
            )
            .await?;
            finish_output(&line, progress)
        }
        Some(Command::Encode(EncodeArgs {
            command: EncodeCommand::Pause { off },
        })) => {
            let line = encode_cmd::pause(json, off).await?;
            finish_output(&line, progress)
        }
        Some(Command::Reclaim(ReclaimArgs {
            command:
                ReclaimCommand::Preview {
                    library_root,
                    socket,
                    tls_dir,
                    config_dir,
                },
        })) => {
            let line =
                reclaim::preview(json, None, library_root, socket, tls_dir, config_dir).await?;
            finish_output(&line, progress)
        }
        Some(Command::Reclaim(ReclaimArgs {
            command:
                ReclaimCommand::Apply {
                    library_root,
                    socket,
                    tls_dir,
                    config_dir,
                    max,
                },
        })) => {
            let line =
                reclaim::apply(json, None, library_root, socket, tls_dir, config_dir, max).await?;
            finish_output(&line, progress)
        }
        Some(Command::Hold(HoldArgs { command })) => {
            let line = match command {
                HoldCommand::List { socket } => api_cmd::hold_list(output, socket).await?,
                HoldCommand::Approve {
                    target,
                    release_id,
                    socket,
                } => {
                    api_cmd::hold_decide(
                        target,
                        release_id,
                        mediaops_core::HoldDecisionSpec::Approved,
                        output,
                        socket,
                    )
                    .await?
                }
                HoldCommand::Reject {
                    target,
                    release_id,
                    socket,
                } => {
                    api_cmd::hold_decide(
                        target,
                        release_id,
                        mediaops_core::HoldDecisionSpec::Rejected,
                        output,
                        socket,
                    )
                    .await?
                }
            };
            finish_output(&line, progress)
        }
        Some(Command::NewMachine(NewMachineArgs {
            command:
                NewMachineCommand::Export {
                    out,
                    config_dir,
                    desired_state,
                    tls_dir,
                },
        })) => {
            let line =
                new_machine::export_machine(json, out, config_dir, desired_state, tls_dir, None)
                    .await?;
            finish_output(&line, progress)
        }
        Some(Command::NewMachine(NewMachineArgs {
            command:
                NewMachineCommand::Import {
                    from,
                    library_root,
                    config_dir,
                    desired_state,
                    tls_dir,
                },
        })) => {
            let line = new_machine::import_machine(
                json,
                from,
                library_root,
                config_dir,
                desired_state,
                tls_dir,
                None,
            )
            .await?;
            finish_output(&line, progress)
        }
        Some(Command::Doctor {
            repair,
            confirm,
            pin,
            desired_state,
            socket,
            tls_dir,
            config_dir,
            state_db,
            api_socket,
        }) => {
            // Readiness supplements the edge/key/PEM checks; a running API
            // cannot turn those security checks into a successful Node list.
            let mut line = doctor::doctor(
                json || output == api_cmd::Output::Json,
                repair,
                confirm,
                pin,
                desired_state,
                socket,
                tls_dir,
                config_dir,
                state_db,
            )
            .await?;
            api_cmd::doctor_nodes(api_socket).await?;
            if output == api_cmd::Output::Json {
                let envelope: serde_json::Value =
                    serde_json::from_str(&line).map_err(|err| AppError::Runtime(err.into()))?;
                line = serde_json::to_string(&envelope["data"])
                    .map_err(|err| AppError::Runtime(err.into()))?;
            }
            finish_output(&line, progress)
        }
        Some(Command::Repair(RepairArgs {
            command:
                RepairCommand::Edge {
                    repair,
                    confirm,
                    pin,
                    desired_state,
                    socket,
                    tls_dir,
                    config_dir,
                    state_db,
                    ssh_config,
                },
        })) => {
            let line = repair::repair_edge(
                json,
                repair,
                confirm,
                pin,
                desired_state,
                socket,
                tls_dir,
                config_dir,
                state_db,
                ssh_config,
                &SystemExec,
            )
            .await?;
            finish_output(&line, progress)
        }
        Some(Command::Get {
            kind,
            name,
            watch,
            socket,
        }) => {
            if watch {
                return api_cmd::watch_kind(Some(kind), name, output, socket).await;
            }
            let line = if let Some(name) = name {
                api_cmd::get(kind, name, output, socket).await?
            } else {
                api_cmd::list_kind(Some(kind), output, socket).await?
            };
            finish_output(&line, progress)
        }
        Some(Command::Apply { file, socket }) => {
            let line = api_cmd::apply_file(file, output, socket).await?;
            finish_output(&line, progress)
        }
        Some(Command::Delete { kind, name, socket }) => {
            let line = api_cmd::delete(kind, name, output, socket).await?;
            finish_output(&line, progress)
        }
        Some(Command::Reconcile { socket }) => {
            let line = api_cmd::reconcile(output, socket).await?;
            finish_output(&line, progress)
        }
        Some(Command::Sync {
            dry_run,
            request_id,
            socket,
        }) => {
            let line = sync_cmd::run(dry_run, request_id, output, socket).await?;
            finish_output(&line, progress)
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    let json_flag = json_requested();
    match parse_cli(json_flag) {
        Ok(ParseOutcome::HelpOrVersion) => ExitCode::Ok,
        Ok(ParseOutcome::Parsed(cli)) => {
            let json = json_flag;
            match run(cli).await {
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
    fn json_detection_accepts_output_syntax_only() {
        for args in [vec!["-o", "json"], vec!["--output=json"], vec!["-ojson"]] {
            assert!(args_request_json(args.into_iter().map(str::to_owned)));
        }
        assert!(!args_request_json(["--json".to_owned()]));
        assert!(!args_request_json(["--output=wide".to_owned()]));
        assert!(!args_request_json(["--".to_owned(), "-ojson".to_owned()]));
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

    #[test]
    fn policy_maps_to_exit_5() {
        let err = AppError::Policy("need --yes".into());
        assert_eq!(to_exit_code(&err), ExitCode::PolicyRefusal);
    }

    #[test]
    fn lock_conflict_maps_to_exit_3() {
        let err = AppError::LockConflict("held".into());
        assert_eq!(to_exit_code(&err), ExitCode::LockConflict);
        assert_eq!(i32::from(to_exit_code(&err)), 3);
    }

    #[test]
    fn drift_verify_maps_to_exit_4() {
        let err = AppError::DriftVerify("snapshot".into());
        assert_eq!(to_exit_code(&err), ExitCode::DriftVerify);
        assert_eq!(i32::from(to_exit_code(&err)), 4);
    }
}
