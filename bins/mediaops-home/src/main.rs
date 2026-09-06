//! Supervisor: execs the five home role binaries and restarts a dead child.
//! On SIGTERM/SIGINT it forwards SIGTERM, then bounds the wait before SIGKILL.

use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use clap::Parser;
use tokio::process::{Child, Command};
use tokio::signal::unix::{SignalKind, signal};

const ROLES: &[&str] = &[
    "mediaops-api",
    "mediaops-scheduler",
    "mediaops-gateway",
    "mediaops-inventory",
    "mediaops-pull",
];
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Parser, Debug)]
#[command(name = "mediaops-home", version)]
struct Cli {
    /// Home API socket, forwarded to every role.
    #[arg(long)]
    socket: Option<PathBuf>,
    #[arg(long)]
    api_db: Option<PathBuf>,
    #[arg(long)]
    gateway_socket: Option<PathBuf>,
    #[arg(long)]
    tls_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let bin_dir = std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    // Register before starting children: even early startup must be stoppable.
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut children = spawn_children(|role| spawn_role(&bin_dir, role, &cli)).await?;
    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                terminate_all(&mut children).await;
                return Ok(());
            }
            _ = sigint.recv() => {
                terminate_all(&mut children).await;
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                for (i, child) in children.iter_mut().enumerate() {
                    if let Ok(Some(status)) = child.try_wait() {
                        tracing::warn!(role = ROLES[i], %status, "child exited; restarting");
                        // Never `?` here: returning would drop the supervisor
                        // and leave the other four running unsupervised with
                        // no way to stop them through the service unit.
                        match spawn_role(&bin_dir, ROLES[i], &cli) {
                            Ok(next) => *child = next,
                            Err(err) => {
                                tracing::error!(role = ROLES[i], error = %err, "respawn failed");
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn spawn_children(
    mut spawn: impl FnMut(&str) -> anyhow::Result<Child>,
) -> anyhow::Result<Vec<Child>> {
    let mut children = Vec::with_capacity(ROLES.len());
    for role in ROLES {
        match spawn(role) {
            Ok(child) => children.push(child),
            Err(err) => {
                terminate_all(&mut children).await;
                return Err(err.context(format!("start {role}")));
            }
        }
    }
    Ok(children)
}

fn spawn_role(bin_dir: &Path, role: &str, cli: &Cli) -> anyhow::Result<Child> {
    let path = resolve_bin(bin_dir, role)?;
    let mut cmd = Command::new(path);
    cmd.arg("serve").stdin(Stdio::null()).kill_on_drop(true);
    if role == "mediaops-api" {
        if let Some(socket) = &cli.socket {
            cmd.arg("--socket").arg(socket);
        }
        if let Some(db) = &cli.api_db {
            cmd.arg("--api-db").arg(db);
        }
    } else if role == "mediaops-gateway" {
        if let Some(socket) = &cli.socket {
            cmd.arg("--api-socket").arg(socket);
        }
        if let Some(gw) = &cli.gateway_socket {
            cmd.arg("--socket").arg(gw);
        }
        if let Some(tls) = &cli.tls_dir {
            cmd.arg("--tls-dir").arg(tls);
        }
    } else if role == "mediaops-scheduler" {
        if let Some(socket) = &cli.socket {
            cmd.arg("--socket").arg(socket);
        }
    } else {
        if let Some(socket) = &cli.socket {
            cmd.arg("--socket").arg(socket);
        }
        if let Some(gw) = &cli.gateway_socket {
            cmd.arg("--gateway-socket").arg(gw);
        }
        if let Some(tls) = &cli.tls_dir {
            cmd.arg("--tls-dir").arg(tls);
        }
    }
    tracing::info!(role, "spawn");
    Ok(cmd.spawn()?)
}

fn resolve_bin(bin_dir: &Path, role: &str) -> anyhow::Result<PathBuf> {
    let next_to = bin_dir.join(role);
    if next_to.is_file() {
        return Ok(next_to);
    }
    Ok(PathBuf::from(role))
}

async fn terminate_all(children: &mut [Child]) {
    terminate_with_grace(children, SHUTDOWN_GRACE).await;
}

async fn terminate_with_grace(children: &mut [Child], grace: Duration) {
    for child in children.iter_mut() {
        if let Some(pid) = child.id() {
            // The owned Child has not been reaped, so its PID cannot be reused.
            let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            if result != 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::ESRCH) {
                    tracing::warn!(pid, error = %err, "child SIGTERM failed");
                }
            }
        }
    }
    if tokio::time::timeout(grace, async {
        for child in children.iter_mut() {
            let _ = child.wait().await;
        }
    })
    .await
    .is_err()
    {
        for child in children.iter_mut() {
            let _ = child.start_kill();
        }
    }
    for child in children.iter_mut() {
        let _ = child.wait().await;
    }
}

fn init_tracing() {
    let subscriber = tracing_subscriber::fmt().with_writer(io::stderr);
    if io::stderr().is_terminal() {
        subscriber.init();
    } else {
        subscriber.json().init();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    async fn child_with_term_handler(handler: &str) -> Child {
        // Child::wait closes stdin, so a blocking `read` would exit without
        // exercising shutdown. Ignored SIGTERM survives exec into sleep.
        let stay_alive = if handler.is_empty() {
            "exec sleep 60"
        } else {
            "while :; do :; done"
        };
        let mut child = Command::new("sh")
            .args([
                "-c",
                &format!("trap '{handler}' TERM; printf ready; {stay_alive}"),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("shell");
        let mut ready = [0; 5];
        child
            .stdout
            .as_mut()
            .expect("stdout")
            .read_exact(&mut ready)
            .await
            .expect("ready");
        assert_eq!(&ready, b"ready");
        child
    }

    #[tokio::test]
    async fn shutdown_forwards_term_before_using_kill() {
        let child = child_with_term_handler("exit 42").await;
        let mut children = [child];
        terminate_with_grace(&mut children, Duration::from_secs(1)).await;
        assert_eq!(
            children[0].try_wait().expect("wait").expect("exit").code(),
            Some(42)
        );
    }

    #[tokio::test]
    async fn uncooperative_child_is_killed_after_grace() {
        use std::os::unix::process::ExitStatusExt;
        let child = child_with_term_handler("").await;
        let mut children = [child];
        terminate_with_grace(&mut children, Duration::from_millis(50)).await;
        assert_eq!(
            children[0]
                .try_wait()
                .expect("wait")
                .expect("exit")
                .signal(),
            Some(libc::SIGKILL)
        );
    }

    #[tokio::test]
    async fn partial_startup_failure_reaps_the_children_already_started() {
        let mut child = Some(child_with_term_handler("exit 0").await);
        let pid = child.as_ref().expect("child").id().expect("pid");
        let result = spawn_children(|_| {
            child
                .take()
                .ok_or_else(|| anyhow::anyhow!("missing role binary"))
        })
        .await;
        assert!(result.is_err());
        assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }
}
