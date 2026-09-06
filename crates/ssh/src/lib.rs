//! Bootstrap exec via system ssh. SwizzinBox provider. No bulk copy (AD-16, AD-21).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mediaops_core::{
    ExecCommand, ExecError, ExecOutput, ExecPort, ProviderError, ProviderKind,
    already_there_install, reject_bulk_copy,
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
///
/// A `Host` line may list several aliases (`Host seedbox other`); the block applies
/// when any token equals the requested alias. `Include`, `Host *`, and `Match` are
/// ignored (deferred).
pub fn parse_ssh_config(text: &str, alias: &str) -> Result<SshHost, SshError> {
    let mut applies = false;
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
        if key.eq_ignore_ascii_case("Include") {
            continue;
        }
        if key.eq_ignore_ascii_case("Match") {
            applies = false;
            continue;
        }
        if key.eq_ignore_ascii_case("Host") {
            let tokens: Vec<&str> = parts.collect();
            if tokens.iter().all(|token| *token == "*") {
                applies = false;
                continue;
            }
            applies = tokens.iter().any(|token| *token == alias);
            if applies {
                found = true;
            }
            continue;
        }
        if !applies {
            continue;
        }
        let value = parts.collect::<Vec<_>>().join(" ");
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
    checked_git_work_tree(path).unwrap_or(true)
}

fn checked_git_work_tree(path: &Path) -> std::io::Result<bool> {
    // The destination may not exist yet, and an existing ancestor may be a
    // symlink into dotfiles. Check both spellings before writing any PEMs.
    for ancestor in path.ancestors() {
        if has_git_marker(ancestor)? {
            return Ok(true);
        }
        match ancestor.canonicalize() {
            Ok(real) => {
                for directory in real.ancestors() {
                    if has_git_marker(directory)? {
                        return Ok(true);
                    }
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(false)
}

fn has_git_marker(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path.join(".git")) {
        Ok(_) => Ok(true),
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(false)
        }
        Err(err) => Err(err),
    }
}

pub fn refuse_git_work_tree(path: &Path) -> Result<(), SshError> {
    if checked_git_work_tree(path)
        .map_err(|err| SshError::Other(format!("inspect {}: {err}", path.display())))?
    {
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

/// `make musl`: builds the static daemon with the musl target.
pub fn musl_build_command() -> ExecCommand {
    ExecCommand::new("make", vec!["musl".into()])
}

/// Where `make musl` leaves the daemon. Honours `CARGO_TARGET_DIR`.
pub fn musl_binary_path() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target"));
    target
        .join("x86_64-unknown-linux-musl")
        .join("release")
        .join("mediaopsd")
}

pub fn scp_file_command(local: &Path, remote: &str, ssh_config: &Path) -> ExecCommand {
    ExecCommand::new(
        "scp",
        vec![
            "-F".into(),
            ssh_config.display().to_string(),
            local.display().to_string(),
            format!("seedbox:{remote}"),
        ],
    )
}

/// Desired Swizzin nginx app snippet. Host `$host` is EdgeInvariant.
pub fn desired_nginx_app(url_base: &str, port: u16) -> String {
    format!(
        "location {url_base} {{\n    proxy_pass http://127.0.0.1:{port}{url_base};\n    proxy_set_header Host $host;\n}}\n"
    )
}

/// Keep live Swizzin conf; only ensure `proxy_set_header Host $host`.
pub fn splice_host_dollar_host(live: &str) -> String {
    if mediaops_core::nginx_host_ok(live) {
        return live.to_string();
    }
    let mut out = Vec::new();
    let mut replaced = false;
    for line in live.lines() {
        let trimmed = line.trim_start();
        let indent_len = line.len() - trimmed.len();
        let indent = &line[..indent_len];
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("proxy_set_header host ") {
            out.push(format!("{indent}proxy_set_header Host $host;"));
            replaced = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !replaced {
        let mut inserted = false;
        let mut with_insert = Vec::new();
        for line in &out {
            with_insert.push(line.clone());
            if !inserted
                && line
                    .trim_start()
                    .to_ascii_lowercase()
                    .starts_with("proxy_pass ")
            {
                let indent_len = line.len() - line.trim_start().len();
                with_insert.push(format!(
                    "{}proxy_set_header Host $host;",
                    " ".repeat(indent_len)
                ));
                inserted = true;
            }
        }
        out = with_insert;
        if !inserted {
            out.push("    proxy_set_header Host $host;".into());
        }
    }
    let mut text = out.join("\n");
    if live.ends_with('\n') && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Write a small remote file over ssh (not bulk copy). Contents ride argv.
pub async fn write_remote_file(
    exec: &impl ExecPort,
    ssh_config: &Path,
    remote_path: &str,
    contents: &str,
) -> Result<String, SshError> {
    let old = exec
        .run(&ssh_exec(ssh_config, &["sudo", "cat", remote_path]))
        .await?;
    let old_text = if old.status == 0 {
        String::from_utf8_lossy(&old.stdout).into_owned()
    } else {
        let err = String::from_utf8_lossy(&old.stderr);
        if err.contains("No such file") || err.contains("not found") {
            String::new()
        } else {
            return Err(SshError::Other(format!(
                "sudo cat {remote_path} failed: {err}"
            )));
        }
    };
    let diff = mediaops_core::unified_diff(&old_text, contents, remote_path);
    if diff.is_empty() {
        return Ok(diff);
    }
    let escaped = contents.replace('\'', "'\\''");
    let script = format!("printf '%s' '{escaped}' > {remote_path}");
    exec.run(&ssh_exec(ssh_config, &["sudo", "/bin/sh", "-c", &script]))
        .await?;
    Ok(diff)
}

/// Read live app conf, splice `Host $host`, write if needed.
pub async fn write_spliced_nginx_app(
    exec: &impl ExecPort,
    ssh_config: &Path,
    remote_path: &str,
    url_base: &str,
    port: u16,
) -> Result<String, SshError> {
    let old = exec
        .run(&ssh_exec(ssh_config, &["sudo", "cat", remote_path]))
        .await?;
    let live = if old.status == 0 {
        String::from_utf8_lossy(&old.stdout).into_owned()
    } else {
        let err = String::from_utf8_lossy(&old.stderr);
        if err.contains("No such file") || err.contains("not found") {
            String::new()
        } else {
            return Err(SshError::Other(format!(
                "sudo cat {remote_path} failed: {err}"
            )));
        }
    };
    let desired = if live.trim().is_empty() {
        desired_nginx_app(url_base, port)
    } else {
        splice_host_dollar_host(&live)
    };
    write_remote_file(exec, ssh_config, remote_path, &desired).await
}

/// `nginx -t` then reload. Repair calls this after writing app confs.
pub async fn nginx_test_and_reload(
    exec: &impl ExecPort,
    ssh_config: &Path,
) -> Result<(), SshError> {
    exec.run(&ssh_exec(ssh_config, &["sudo", "nginx", "-t"]))
        .await?;
    exec.run(&ssh_exec(ssh_config, &["sudo", "nginx", "-s", "reload"]))
        .await?;
    Ok(())
}

pub fn ssh_exec(ssh_config: &Path, remote_argv: &[&str]) -> ExecCommand {
    let mut args = vec![
        "-F".into(),
        ssh_config.display().to_string(),
        "seedbox".into(),
    ];
    args.extend(remote_argv.iter().map(|arg| (*arg).to_string()));
    ExecCommand::new("ssh", args)
}

/// Copy the daemon binary next to its live name and rename over it. A running
/// `mediaopsd` keeps its old inode mapped; writing *into* that inode is
/// `ETXTBSY`, renaming a new one over it is not.
async fn copy_binary_atomically(
    exec: &impl ExecPort,
    local_binary: &Path,
    ssh_config: &Path,
) -> Result<(), SshError> {
    exec.run(&scp_file_command(
        local_binary,
        ".local/bin/mediaopsd.new",
        ssh_config,
    ))
    .await?;
    exec.run(&ssh_exec(
        ssh_config,
        &[
            "chmod",
            "755",
            ".local/bin/mediaopsd.new",
            "&&",
            "mv",
            "-f",
            ".local/bin/mediaopsd.new",
            ".local/bin/mediaopsd",
        ],
    ))
    .await?;
    Ok(())
}

/// Bootstrap/upgrade install step: musl `mediaopsd` copy + unit restart. Never apt/panel.
pub async fn copy_binary_and_restart_unit(
    exec: &impl ExecPort,
    local_binary: &Path,
    ssh_config: &Path,
) -> Result<(), SshError> {
    exec.run(&musl_build_command()).await?;
    exec.run(&ssh_exec(ssh_config, &["mkdir", "-p", ".local/bin"]))
        .await?;
    copy_binary_atomically(exec, local_binary, ssh_config).await?;
    exec.run(&ssh_exec(
        ssh_config,
        &["systemctl", "--user", "daemon-reload"],
    ))
    .await?;
    exec.run(&ssh_exec(
        ssh_config,
        &["systemctl", "--user", "restart", "mediaopsd.service"],
    ))
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn install_provider(
    exec: &impl ExecPort,
    kind: ProviderKind,
    local_binary: &Path,
    unit_text: &str,
    unit_local: &Path,
    tls_dir: &Path,
    desired_state: &Path,
    ssh_config: &Path,
) -> Result<(), SshError> {
    kind.ensure_installable()?;
    match kind {
        ProviderKind::AlreadyThere => {
            already_there_install()?;
            Ok(())
        }
        ProviderKind::SwizzinBox => {
            exec.run(&musl_build_command()).await?;
            exec.run(&ssh_exec(
                ssh_config,
                &[
                    "mkdir",
                    "-p",
                    ".local/bin",
                    ".config/systemd/user",
                    ".config/mediaops/tls",
                ],
            ))
            .await?;
            copy_binary_atomically(exec, local_binary, ssh_config).await?;
            for name in ["ca.pem", "server.pem", "server.key"] {
                exec.run(&scp_file_command(
                    &tls_dir.join(name),
                    &format!(".config/mediaops/tls/{name}"),
                    ssh_config,
                ))
                .await?;
            }
            // The unit points the daemon at this file for its grabber mode
            // and roots; without it the service crash-loops on start.
            exec.run(&scp_file_command(
                desired_state,
                ".config/mediaops/config.toml",
                ssh_config,
            ))
            .await?;
            std::fs::write(unit_local, unit_text)
                .map_err(|err| SshError::Other(err.to_string()))?;
            exec.run(&scp_file_command(
                unit_local,
                ".config/systemd/user/mediaopsd.service",
                ssh_config,
            ))
            .await?;
            exec.run(&ssh_exec(
                ssh_config,
                &["systemctl", "--user", "daemon-reload"],
            ))
            .await?;
            exec.run(&ssh_exec(
                ssh_config,
                &[
                    "systemctl",
                    "--user",
                    "enable",
                    "--now",
                    "mediaopsd.service",
                ],
            ))
            .await?;
            // User units die with the last SSH session unless lingering is on.
            // SeedIt4Me has passwordless sudo for this; without linger the
            // daemon is gone the moment you disconnect.
            exec.run(&ssh_exec(
                ssh_config,
                &["sh", "-c", "sudo -n loginctl enable-linger \"$(id -un)\""],
            ))
            .await?;
            // `enable --now` is a no-op for a unit that is already running on
            // the old binary or the old bind; a re-bootstrap must restart it.
            exec.run(&ssh_exec(
                ssh_config,
                &["systemctl", "--user", "restart", "mediaopsd.service"],
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
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ExecError::Failed {
                program: command.program.clone(),
                message: format!("exited {}: {stderr}", output.status.code().unwrap_or(1)),
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
    fn host_line_with_multiple_aliases_imports_hostname() {
        let text = "Host seedbox other\n  HostName box.example\n  User foo\nHost elsewhere\n  HostName no\n";
        let host = parse_ssh_config(text, "seedbox").expect("host");
        assert_eq!(host.hostname.as_deref(), Some("box.example"));
        assert_eq!(host.user.as_deref(), Some("foo"));
        let other = parse_ssh_config(text, "other").expect("other alias");
        assert_eq!(other.hostname.as_deref(), Some("box.example"));
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

    #[cfg(unix)]
    #[test]
    fn symlinked_config_ancestor_cannot_bypass_pem_refusal() {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-ssh-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("checkout/.git")).expect("checkout");
        std::os::unix::fs::symlink(dir.join("checkout"), dir.join("config")).expect("config link");
        assert!(refuse_git_work_tree(&dir.join("config/mediaops/tls")).is_err());
        assert!(
            !dir.join("checkout/mediaops").exists(),
            "checking must not create files"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_existing_regular_config_file_is_not_a_git_marker() {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-ssh-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("directory");
        let config = dir.join("config.toml");
        std::fs::write(&config, b"config").expect("file");
        assert!(refuse_git_work_tree(&config).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn calls_scp_to(exec: &TranscriptExec, remote: &str) -> bool {
        exec.recorded().iter().any(|c| {
            c.program_name() == "scp" && c.args.iter().any(|a| a == &format!("seedbox:{remote}"))
        })
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
            Path::new("/tmp/tls"),
            Path::new("/tmp/config.toml"),
            Path::new("/tmp/ssh_config"),
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
            Path::new("/tmp/tls"),
            Path::new("/tmp/config.toml"),
            Path::new("/tmp/ssh_config"),
        )
        .await
        .expect("noop");
        assert!(exec.recorded().is_empty());
    }

    #[test]
    fn desired_nginx_app_uses_host_dollar_host() {
        let conf = desired_nginx_app("/sonarr", 8989);
        assert!(conf.contains("Host $host"));
        assert!(conf.contains("127.0.0.1:8989/sonarr"));
        assert!(!conf.contains("Host 127.0.0.1"));
    }

    #[tokio::test]
    async fn upgrade_copies_binary_and_restarts_never_apt_or_panel() {
        let exec = TranscriptExec::new().reply(
            "cargo",
            ExecOutput {
                status: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
        );
        copy_binary_and_restart_unit(&exec, Path::new("/tmp/mediaopsd"), Path::new("/tmp/ssh"))
            .await
            .expect("upgrade");
        let calls = exec.recorded();
        assert!(
            calls.iter().any(
                |c| c.program_name() == "scp" && c.args.iter().any(|a| a.contains("mediaopsd"))
            ),
            "{calls:?}"
        );
        assert!(
            calls.iter().any(|c| c.args.iter().any(|a| a == "restart")),
            "{calls:?}"
        );
        assert!(
            calls
                .iter()
                .all(|c| c.program_name() != "apt" && c.program_name() != "apt-get"),
            "{calls:?}"
        );
        assert!(
            !calls.iter().any(|c| c
                .args
                .iter()
                .any(|a| a.contains("panel") || a.contains("swizzin"))),
            "{calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|c| c.program_name() == "make" && c.args == ["musl"]),
            "musl build step missing: {calls:?}"
        );
    }

    #[test]
    fn splice_host_keeps_surrounding_conf() {
        let live = "location /sonarr {\n    proxy_pass http://127.0.0.1:8989/sonarr;\n    proxy_set_header Host 127.0.0.1;\n    proxy_http_version 1.1;\n}\n";
        let out = splice_host_dollar_host(live);
        assert!(out.contains("proxy_http_version 1.1"));
        assert!(out.contains("Host $host"));
        assert!(!out.contains("Host 127.0.0.1"));
    }

    #[tokio::test]
    async fn nginx_repair_is_exec_not_bulk_copy() {
        let exec = TranscriptExec::new();
        let diff = write_remote_file(
            &exec,
            Path::new("/tmp/ssh_config"),
            "/etc/nginx/apps/sonarr.conf",
            &desired_nginx_app("/sonarr", 8989),
        )
        .await
        .expect("write");
        assert!(!diff.is_empty());
        let calls = exec.recorded();
        assert!(calls.iter().all(|c| c.program_name() == "ssh"), "{calls:?}");
        assert!(
            calls
                .iter()
                .any(|c| c.args.iter().any(|a| a.contains("sonarr.conf"))),
            "{calls:?}"
        );
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
        let tls = dir.join("tls");
        std::fs::create_dir_all(&tls).expect("tls");
        for name in ["ca.pem", "server.pem", "server.key"] {
            std::fs::write(tls.join(name), b"pem").expect("tls file");
        }
        let ssh_config = dir.join("ssh_config");
        std::fs::write(&ssh_config, "Host seedbox\n").expect("ssh");
        let unit_text = systemd_user_unit(
            "%h/.local/bin/mediaopsd serve --role seedbox --bind 0.0.0.0:50051 --tls-dir %h/.config/mediaops/tls",
        );
        let desired_state = tls.parent().expect("dir").join("config.toml");
        std::fs::write(&desired_state, "schema_version = 1\n").expect("ds");
        install_provider(
            &exec,
            ProviderKind::SwizzinBox,
            &bin,
            &unit_text,
            &unit,
            &tls,
            &desired_state,
            &ssh_config,
        )
        .await
        .expect("install");
        assert!(
            calls_scp_to(&exec, ".config/mediaops/config.toml"),
            "config.toml must be shipped: the unit reads it on the box"
        );
        let calls = exec.recorded();
        assert_eq!(calls[0].program_name(), "make");
        assert_eq!(calls[0].args, ["musl"]);
        assert!(
            calls
                .iter()
                .any(|c| { c.program_name() == "ssh" && c.args.iter().any(|a| a == "mkdir") })
        );
        let scp_calls: Vec<_> = calls.iter().filter(|c| c.program_name() == "scp").collect();
        assert!(scp_calls.len() >= 2, "binary + unit + tls files");
        assert!(calls.iter().any(|c| {
            c.program_name() == "ssh"
                && c.args.iter().any(|a| a == "systemctl")
                && c.args.iter().any(|a| a == "enable")
        }));
        assert!(
            calls.iter().any(|c| {
                c.program_name() == "ssh"
                    && c.args.iter().any(|a| a.contains("loginctl enable-linger"))
            }),
            "linger so mediaopsd survives logout: {calls:?}"
        );
        assert!(!calls.iter().any(|c| reject_bulk_copy(c).is_err()));
        assert!(
            !calls
                .iter()
                .any(|c| c.args.iter().any(|a| a.contains("client.key")))
        );
        let written = std::fs::read_to_string(&unit).expect("unit on disk");
        assert!(written.contains("WantedBy=default.target"));
        assert!(written.contains("--tls-dir"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
