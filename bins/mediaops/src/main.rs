use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::anyhow;
use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand};
use mediaops_core::{ExitCode, ProviderKind};
use mediaops_ssh::SystemExec;

mod api_cmd;
mod api_legacy;
mod apply_cmd;
mod bootstrap;
mod doctor;
mod encode_cmd;
mod hold;
mod home;
mod library;
mod new_machine;
mod out;
mod reclaim;
mod repair;
mod status;
mod watch;

#[cfg(test)]
mod test_support;

const BIN_NAME: &str = "mediaops";

#[derive(Parser, Debug)]
#[command(name = BIN_NAME, version)]
struct Cli {
    /// Emit a single JSON envelope on stdout (legacy verbs).
    #[arg(long, global = true)]
    json: bool,
    /// Home API output: table (default), wide, or json (raw object).
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
        state_db: Option<PathBuf>,
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
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
        #[arg(long)]
        state_db: Option<PathBuf>,
    },
    /// Why is this title stuck (grab / import / hold / pull / watermark / lock / encode-queue).
    Why {
        title: String,
        #[arg(long)]
        state_db: Option<PathBuf>,
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
        #[arg(long)]
        library_root: Option<PathBuf>,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
        #[arg(long = "api-socket")]
        api_socket: Option<PathBuf>,
    },
    /// Open Wants, bound Jobs, Node readiness. Home API when the apiserver is up.
    Status {
        #[arg(long)]
        state_db: Option<PathBuf>,
        #[arg(long)]
        plans_dir: Option<PathBuf>,
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
        #[arg(long)]
        library_root: Option<PathBuf>,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
        #[arg(long = "api-socket")]
        api_socket: Option<PathBuf>,
    },
    Encode(EncodeArgs),
    /// Ranked dry-run / exclusive unlink of surplus remotes after install_b3 proof.
    Reclaim(ReclaimArgs),
    /// Import-blocked inbox (lock-free).
    Hold(HoldArgs),
    /// Export/import config.toml + tls/ + title-index into the active XDG dirs.
    NewMachine(NewMachineArgs),
    /// Read-only EdgeInvariant + key presence + PEM-in-git scan.
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
    /// Increment Cluster.status.reconcile_generation.
    Reconcile {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Import old config.toml + state.db into the Home API.
    ImportLegacy {
        #[arg(long = "config", value_name = "PATH")]
        config: Option<PathBuf>,
        #[arg(long)]
        state_db: Option<PathBuf>,
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
        state_db: Option<PathBuf>,
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
        state_db: Option<PathBuf>,
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
    /// Write config.toml, tls/, and title-index.json into `--out`.
    Export {
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
        #[arg(long)]
        state_db: Option<PathBuf>,
    },
    /// Populate the active config dir and state.db from a directory bundle.
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
        #[arg(long)]
        state_db: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum HoldCommand {
    /// List undecided import-blocked releases (live ⊖ decided). Lock-free.
    List {
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
    /// Persist Approved. Does not install; the next Pull Job copies. Lock-free.
    Approve {
        /// Title id from `hold list` (`movie:tmdb:…` / `series:tvdb:…` / `album:mbid:…`).
        target: String,
        /// Release id. Only needed when the same title id is in the inbox twice.
        release_id: Option<String>,
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
    /// Persist Rejected and tell *arr never-this-release. Lock-free.
    Reject {
        /// Title id from `hold list` (`movie:tmdb:…` / `series:tvdb:…` / `album:mbid:…`).
        target: String,
        /// Release id. Only needed when the same title id is in the inbox twice.
        release_id: Option<String>,
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
}

#[derive(Subcommand, Debug)]
enum EncodeCommand {
    /// Classify movies/ under EncodePolicy. Lock-free.
    Scan {
        #[arg(long)]
        library_root: Option<PathBuf>,
        #[arg(long)]
        state_db: Option<PathBuf>,
    },
    /// Run ready encode jobs, or one title.
    Run {
        title: Option<String>,
        #[arg(long)]
        state_db: Option<PathBuf>,
        #[arg(long)]
        library_root: Option<PathBuf>,
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
        #[arg(long)]
        config_dir: Option<PathBuf>,
    },
    /// Set or clear the encode_pause machine flag. Lock-free.
    Pause {
        #[arg(long)]
        off: bool,
        #[arg(long)]
        state_db: Option<PathBuf>,
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
        #[arg(long)]
        state_db: Option<PathBuf>,
        #[arg(long)]
        enable_timer: bool,
        #[arg(long)]
        unit_dir: Option<PathBuf>,
    },
    /// Retarget the one home library: layout, `library_root`, units. Does not copy media.
    Relocate {
        #[arg(long)]
        library_root: PathBuf,
        #[arg(long = "config", value_name = "PATH")]
        desired_state: Option<PathBuf>,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long)]
        state_db: Option<PathBuf>,
        #[arg(long)]
        enable_timer: bool,
        #[arg(long)]
        unit_dir: Option<PathBuf>,
    },
    /// Rebuild title-index proof by hashing on-disk schema files.
    Reindex {
        #[arg(long)]
        library_root: Option<PathBuf>,
        #[arg(long)]
        state_db: Option<PathBuf>,
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
        AppError::Policy(_) => ExitCode::PolicyRefusal,
        AppError::LockConflict(_) => ExitCode::LockConflict,
        AppError::DriftVerify(_) => ExitCode::DriftVerify,
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

async fn run(cli: Cli) -> Result<(), AppError> {
    match cli.command {
        None => emit_success(cli.json),
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
                    let line = bootstrap::render_upgrade(cli.json, &report)
                        .map_err(|e| AppError::Runtime(anyhow!(e)))?;
                    write_stdout(&line)
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
                cli.json,
                desired_state,
                socket,
                tls_dir,
                config_dir,
                state_db,
            )
            .await?;
            write_stdout(&line)
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
                    let line = bootstrap::render_report(cli.json, &report)
                        .map_err(|e| AppError::Runtime(anyhow!(e)))?;
                    write_stdout(&line)
                }
                Err(bootstrap::BootstrapError::NeedsConfirm(report)) => {
                    let line = bootstrap::render_needs_confirm(cli.json, &report)
                        .map_err(|e| AppError::Runtime(anyhow!(e)))?;
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
            let line = home::list(cli.json, socket, tls_dir, config_dir).await?;
            write_stdout(&line)
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
            state_db,
            desired_state,
            install,
            title,
            year,
            season,
            episode,
        }) => {
            let line = home::pull(
                cli.json,
                root,
                path,
                title_id,
                name,
                library_root,
                socket,
                tls_dir,
                config_dir,
                state_db,
                desired_state,
                install,
                title,
                year,
                season,
                episode,
            )
            .await?;
            write_stdout(&line)
        }
        Some(Command::Library(LibraryArgs {
            command:
                LibraryCommand::Bootstrap {
                    library_root,
                    desired_state,
                    config_dir,
                    state_db,
                    enable_timer,
                    unit_dir,
                },
        })) => {
            let line = library::bootstrap_library(
                cli.json,
                library_root,
                desired_state,
                config_dir,
                state_db,
                enable_timer,
                unit_dir,
            )
            .await?;
            write_stdout(&line)
        }
        Some(Command::Library(LibraryArgs {
            command:
                LibraryCommand::Relocate {
                    library_root,
                    desired_state,
                    config_dir,
                    state_db,
                    enable_timer,
                    unit_dir,
                },
        })) => {
            let line = library::relocate_library(
                cli.json,
                library_root,
                desired_state,
                config_dir,
                state_db,
                enable_timer,
                unit_dir,
            )
            .await?;
            write_stdout(&line)
        }
        Some(Command::Library(LibraryArgs {
            command:
                LibraryCommand::Reindex {
                    library_root,
                    state_db,
                },
        })) => {
            let line = library::reindex_library(cli.json, library_root, state_db).await?;
            write_stdout(&line)
        }
        Some(Command::Watch {
            title,
            socket,
            state_db,
        }) => {
            let output = api_cmd::Output::parse(cli.output.as_deref(), cli.json)?;
            let line = if !api_legacy::use_home(&state_db) && socket.is_none() {
                watch::watch(cli.json, title, state_db).await?
            } else {
                api_cmd::watch_title(title, output, socket).await?
            };
            write_stdout(&line)
        }
        Some(Command::Why {
            title,
            state_db,
            desired_state,
            library_root,
            config_dir,
            socket,
            tls_dir,
            api_socket,
        }) => {
            let output = api_cmd::Output::parse(cli.output.as_deref(), cli.json)?;
            let line = if !api_legacy::use_home(&state_db) && api_socket.is_none() {
                status::why(
                    cli.json,
                    title,
                    state_db,
                    desired_state,
                    library_root,
                    config_dir,
                    socket,
                    tls_dir,
                )
                .await?
            } else {
                api_cmd::why_pretty(title, output, api_socket).await?
            };
            write_stdout(&line)
        }
        Some(Command::Status {
            state_db,
            plans_dir,
            desired_state,
            library_root,
            config_dir,
            socket,
            tls_dir,
            api_socket,
        }) => {
            let output = api_cmd::Output::parse(cli.output.as_deref(), cli.json)?;
            let line = if !api_legacy::use_home(&state_db) && api_socket.is_none() {
                status::status(
                    cli.json,
                    state_db,
                    plans_dir,
                    desired_state,
                    library_root,
                    config_dir,
                    socket,
                    tls_dir,
                )
                .await?
            } else {
                api_cmd::status_pretty(output, api_socket).await?
            };
            write_stdout(&line)
        }
        Some(Command::Encode(EncodeArgs {
            command:
                EncodeCommand::Scan {
                    library_root,
                    state_db,
                },
        })) => {
            let line = encode_cmd::scan(&SystemExec, cli.json, library_root, state_db).await?;
            write_stdout(&line)
        }
        Some(Command::Encode(EncodeArgs {
            command:
                EncodeCommand::Run {
                    title,
                    state_db,
                    library_root,
                    desired_state,
                    config_dir,
                },
        })) => {
            let line = encode_cmd::run(
                &SystemExec,
                cli.json,
                title,
                state_db,
                library_root,
                desired_state,
                config_dir,
            )
            .await?;
            write_stdout(&line)
        }
        Some(Command::Encode(EncodeArgs {
            command: EncodeCommand::Pause { off, state_db },
        })) => {
            let line = encode_cmd::pause(cli.json, off, state_db).await?;
            write_stdout(&line)
        }
        Some(Command::Reclaim(ReclaimArgs {
            command:
                ReclaimCommand::Preview {
                    state_db,
                    library_root,
                    socket,
                    tls_dir,
                    config_dir,
                },
        })) => {
            let line = reclaim::preview(
                cli.json,
                state_db,
                library_root,
                socket,
                tls_dir,
                config_dir,
            )
            .await?;
            write_stdout(&line)
        }
        Some(Command::Reclaim(ReclaimArgs {
            command:
                ReclaimCommand::Apply {
                    state_db,
                    library_root,
                    socket,
                    tls_dir,
                    config_dir,
                    max,
                },
        })) => {
            let line = reclaim::apply(
                cli.json,
                state_db,
                library_root,
                socket,
                tls_dir,
                config_dir,
                max,
            )
            .await?;
            write_stdout(&line)
        }
        Some(Command::Hold(HoldArgs {
            command:
                HoldCommand::List {
                    socket,
                    tls_dir,
                    config_dir,
                    state_db,
                    api_socket,
                },
        })) => {
            let output = api_cmd::Output::parse(cli.output.as_deref(), cli.json)?;
            let line = if !api_legacy::use_home(&state_db) && api_socket.is_none() {
                hold::list(cli.json, socket, tls_dir, config_dir, state_db).await?
            } else {
                api_cmd::hold_list(output, api_socket).await?
            };
            write_stdout(&line)
        }
        Some(Command::Hold(HoldArgs {
            command:
                HoldCommand::Approve {
                    target: title_id,
                    release_id,
                    socket,
                    tls_dir,
                    config_dir,
                    state_db,
                    api_socket,
                },
        })) => {
            let output = api_cmd::Output::parse(cli.output.as_deref(), cli.json)?;
            let line = if api_legacy::use_home(&state_db) || api_socket.is_some() {
                api_cmd::hold_decide(
                    title_id,
                    release_id,
                    mediaops_core::HoldDecisionSpec::Approved,
                    output,
                    api_socket,
                )
                .await?
            } else {
                hold::decide(
                    cli.json,
                    mediaops_core::HoldDecision::Approved,
                    title_id,
                    release_id,
                    socket,
                    tls_dir,
                    config_dir,
                    state_db,
                )
                .await?
            };
            write_stdout(&line)
        }
        Some(Command::Hold(HoldArgs {
            command:
                HoldCommand::Reject {
                    target: title_id,
                    release_id,
                    socket,
                    tls_dir,
                    config_dir,
                    state_db,
                    api_socket,
                },
        })) => {
            let output = api_cmd::Output::parse(cli.output.as_deref(), cli.json)?;
            let line = if api_legacy::use_home(&state_db) || api_socket.is_some() {
                api_cmd::hold_decide(
                    title_id,
                    release_id,
                    mediaops_core::HoldDecisionSpec::Rejected,
                    output,
                    api_socket,
                )
                .await?
            } else {
                hold::decide(
                    cli.json,
                    mediaops_core::HoldDecision::Rejected,
                    title_id,
                    release_id,
                    socket,
                    tls_dir,
                    config_dir,
                    state_db,
                )
                .await?
            };
            write_stdout(&line)
        }
        Some(Command::NewMachine(NewMachineArgs {
            command:
                NewMachineCommand::Export {
                    out,
                    config_dir,
                    desired_state,
                    tls_dir,
                    state_db,
                },
        })) => {
            let line = new_machine::export_machine(
                cli.json,
                out,
                config_dir,
                desired_state,
                tls_dir,
                state_db,
            )
            .await?;
            write_stdout(&line)
        }
        Some(Command::NewMachine(NewMachineArgs {
            command:
                NewMachineCommand::Import {
                    from,
                    library_root,
                    config_dir,
                    desired_state,
                    tls_dir,
                    state_db,
                },
        })) => {
            let line = new_machine::import_machine(
                cli.json,
                from,
                library_root,
                config_dir,
                desired_state,
                tls_dir,
                state_db,
            )
            .await?;
            write_stdout(&line)
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
            let output = api_cmd::Output::parse(cli.output.as_deref(), cli.json)?;
            let check_nodes = api_socket.is_some() || state_db.is_none();
            // Readiness supplements the edge/key/PEM checks; a running API
            // cannot turn those security checks into a successful Node list.
            let mut line = doctor::doctor(
                cli.json || output == api_cmd::Output::Json,
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
            if check_nodes {
                api_cmd::doctor_nodes(api_socket).await?;
            }
            if output == api_cmd::Output::Json {
                let envelope: serde_json::Value =
                    serde_json::from_str(&line).map_err(|err| AppError::Runtime(err.into()))?;
                line = serde_json::to_string(&envelope["data"])
                    .map_err(|err| AppError::Runtime(err.into()))?;
            }
            write_stdout(&line)
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
                cli.json,
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
            write_stdout(&line)
        }
        Some(Command::Get {
            kind,
            name,
            watch,
            socket,
        }) => {
            let output = api_cmd::Output::parse(cli.output.as_deref(), cli.json)?;
            if watch {
                return api_cmd::watch_kind(Some(kind), name, output, socket).await;
            }
            let line = if let Some(name) = name {
                api_cmd::get(kind, name, output, socket).await?
            } else {
                api_cmd::list_kind(Some(kind), output, socket).await?
            };
            write_stdout(&line)
        }
        Some(Command::Apply { file, socket }) => {
            let output = api_cmd::Output::parse(cli.output.as_deref(), cli.json)?;
            let line = api_cmd::apply_file(file, output, socket).await?;
            write_stdout(&line)
        }
        Some(Command::Delete { kind, name, socket }) => {
            let output = api_cmd::Output::parse(cli.output.as_deref(), cli.json)?;
            let line = api_cmd::delete(kind, name, output, socket).await?;
            write_stdout(&line)
        }
        Some(Command::Reconcile { socket }) => {
            let output = api_cmd::Output::parse(cli.output.as_deref(), cli.json)?;
            let line = api_cmd::reconcile(output, socket).await?;
            write_stdout(&line)
        }
        Some(Command::ImportLegacy {
            config,
            state_db,
            socket,
        }) => {
            let output = api_cmd::Output::parse(cli.output.as_deref(), cli.json)?;
            let line = api_cmd::import_legacy(config, state_db, output, socket).await?;
            write_stdout(&line)
        }
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
