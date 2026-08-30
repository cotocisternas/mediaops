//! Bootstrap exec via system ssh. SwizzinBox provider. No bulk copy (AD-16, AD-21).

use std::collections::HashMap;
use std::path::Path;

use mediaops_core::{
    ExecCommand, ExecError, ExecOutput, ExecPort, ProviderError, ProviderKind, already_there_install,
    reject_bulk_copy,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SshError {
    #[error(transparent)]
    Exec(#[from] ExecError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("host `{0}` not found in ssh config")]
    HostNotFound(String),
    #[error("refusing to mint TLS into a git work tree: {0}")]
    GitWorkTree(String),
    #[error("ssh: {0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
}

/// Import `Host seedbox` — the alias is the ssh config Host, not an invented format.
pub fn parse_ssh_config(text: &str, alias: &str) -> Result<SshHost, SshError> {
    let mut current: Option<String> = None;
    let mut host = SshHost {
        alias: alias.to_string(),
        hostname: None,
        user: None,
        port: None,
    };
    let mut found = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or(raw).trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else { continue };
        let value = parts.collect::<Vec<_>>().join(" ");
        if key.eq_ignore_ascii_case("Host") {
            current = Some(value.clone());
            if value == alias {
                found = true;
            }
            continue;
        }
        if current.as_deref() != Some(alias) {
            continue;
        }
        match key.to_ascii_lowercase().as_str() {
            "hostname" => host.hostname = Some(value),
            "user" => host.user = Some(value),
            "port" => {
                host.port = value.parse().ok();
            }
            _ => {}
        }
    }
    if !found {
        return Err(SshError::HostNotFound(alias.to_string()));
    }
    Ok(host)
}

pub fn is_git_work_tree(path: &Path) -> bool {
    let mut cur = Some(path.to_path_buf());
    while let Some(dir) = cur {
        if dir.join(".git").exists() {
            return true;
        }
        cur = dir.parent().map(Path::to_path_buf);
    }
    false
}

pub fn refuse_git_work_tree(path: &Path) -> Result<(), SshError> {
    if is_git_work_tree(path) {
        Err(SshError::GitWorkTree(path.display().to_string()))
    } else {
        Ok(())
    }
}

pub fn systemd_user_unit(exec_start: &str) -> String {
    format!(
        "[Unit]\nDescription=mediaopsd seedbox\n\n[Service]\nExecStart={exec_start}\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n"
    )
}

pub fn musl_build_command() -> ExecCommand {
    ExecCommand::new(
        "cargo",
        vec![
            "build".into(),
            "--release".into(),
            "--target".into(),
            "x86_64-unknown-linux-musl".into(),
            "--bin".into(),
            "mediaopsd".into(),
        ],
    )
}

pub fn scp_file_command(local: &Path, remote: &str) -> ExecCommand {
    ExecCommand::new(
        "scp",
        vec![
            local.display().to_string(),
            format!("seedbox:{remote}"),
        ],
    )
}

pub async fn install_provider(
    exec: &impl ExecPort,
    kind: ProviderKind,
    local_binary: &Path,
    unit_text: &str,
    unit_local: &Path,
) -> Result<(), SshError> {
    kind.ensure_installable()?;
    match kind {
        ProviderKind::AlreadyThere => {
            already_there_install()?;
            Ok(())
        }
        ProviderKind::SwizzinBox => {
            exec.run(&musl_build_command()).await?;
            exec.run(&scp_file_command(
                local_binary,
                ".local/bin/mediaopsd",
            ))
            .await?;
            std::fs::write(unit_local, unit_text).map_err(|err| SshError::Other(err.to_string()))?;
            exec.run(&scp_file_command(
                unit_local,
                ".config/systemd/user/mediaopsd.service",
            ))
            .await?;
            Ok(())
        }
        other => Err(ProviderError::Unimplemented(other).into()),
    }
}

pub struct SystemExec;

impl ExecPort for SystemExec {
    async fn run(&self, command: &ExecCommand) -> Result<ExecOutput, ExecError> {
        reject_bulk_copy(command)?;
        let output = tokio::process::Command::new(&command.program)
            .args(&command.args)
            .output()
            .await
            .map_err(|err| ExecError::Failed {
                program: command.program.clone(),
                message: err.to_string(),
            })?;
        if !output.status.success() {
            return Err(ExecError::Status {
                program: command.program.clone(),
                status: output.status.code().unwrap_or(1),
            });
        }
        Ok(ExecOutput {
            status: output.status.code().unwrap_or(0),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

/// Test double: argv transcript, canned results. Bulk copy still fails.
#[derive(Default)]
pub struct TranscriptExec {
    pub calls: std::sync::Mutex<Vec<ExecCommand>>,
    replies: HashMap<String, ExecOutput>,
}

impl TranscriptExec {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reply(mut self, program: &str, output: ExecOutput) -> Self {
        self.replies.insert(program.to_string(), output);
        self
    }

    pub fn recorded(&self) -> Vec<ExecCommand> {
        self.calls.lock().expect("mutex").clone()
    }
}

impl ExecPort for TranscriptExec {
    async fn run(&self, command: &ExecCommand) -> Result<ExecOutput, ExecError> {
        reject_bulk_copy(command)?;
        self.calls.lock().expect("mutex").push(command.clone());
        Ok(self
            .replies
            .get(command.program_name())
            .cloned()
            .unwrap_or(ExecOutput {
                status: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_seedbox_is_imported_not_invented() {
        let text = "Host seedbox\n  HostName box.example\n  User foo\n  Port 2097\nHost other\n  HostName no\n";
        let host = parse_ssh_config(text, "seedbox").expect("host");
        assert_eq!(host.alias, "seedbox");
        assert_eq!(host.hostname.as_deref(), Some("box.example"));
        assert_eq!(host.user.as_deref(), Some("foo"));
        assert_eq!(host.port, Some(2097));
        assert!(parse_ssh_config(text, "missing").is_err());
    }

    #[test]
    fn git_work_tree_is_refused() {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-ssh-git-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join(".git")).expect("mkdir");
        assert!(refuse_git_work_tree(&dir).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn unimplemented_provider_never_ok() {
        let exec = TranscriptExec::new();
        let err = install_provider(
            &exec,
            ProviderKind::DockerCompose,
            Path::new("/tmp/mediaopsd"),
            "",
            Path::new("/tmp/unit"),
        )
        .await
        .expect_err("unimplemented");
        assert!(matches!(
            err,
            SshError::Provider(ProviderError::Unimplemented(ProviderKind::DockerCompose))
        ));
        assert!(exec.recorded().is_empty());
    }

    #[tokio::test]
    async fn already_there_is_noop() {
        let exec = TranscriptExec::new();
        install_provider(
            &exec,
            ProviderKind::AlreadyThere,
            Path::new("/tmp/mediaopsd"),
            "",
            Path::new("/tmp/unit"),
        )
        .await
        .expect("noop");
        assert!(exec.recorded().is_empty());
    }

    #[tokio::test]
    async fn bulk_copy_is_a_test_failure() {
        let exec = TranscriptExec::new();
        let err = exec
            .run(&ExecCommand::new(
                "rsync",
                vec!["-a".into(), "./".into(), "seedbox:".into()],
            ))
            .await
            .expect_err("bulk");
        assert_eq!(err, ExecError::BulkCopy);
    }

    #[tokio::test]
    async fn swizzin_copies_binary_and_unit_not_a_tree() {
        let exec = TranscriptExec::new().reply(
            "cargo",
            ExecOutput {
                status: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
        );
        let dir = std::env::temp_dir().join(format!(
            "mediaops-ssh-swizzin-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let bin = dir.join("mediaopsd");
        std::fs::write(&bin, b"fake").expect("bin");
        let unit = dir.join("mediaopsd.service");
        install_provider(
            &exec,
            ProviderKind::SwizzinBox,
            &bin,
            &systemd_user_unit("/home/x/.local/bin/mediaopsd serve --config /cfg"),
            &unit,
        )
        .await
        .expect("install");
        let calls = exec.recorded();
        assert_eq!(calls[0].program_name(), "cargo");
        assert!(calls[0].args.iter().any(|a| a.contains("musl")));
        assert_eq!(calls[1].program_name(), "scp");
        assert!(!calls.iter().any(|c| reject_bulk_copy(c).is_err()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
