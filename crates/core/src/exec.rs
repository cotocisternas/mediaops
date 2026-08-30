//! Exec port (AD-16). Signatures only; no tokio, no filesystem.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl ExecCommand {
    pub fn new(program: impl Into<String>, args: impl Into<Vec<String>>) -> Self {
        Self {
            program: program.into(),
            args: args.into(),
        }
    }

    pub fn program_name(&self) -> &str {
        std::path::Path::new(&self.program)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&self.program)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecError {
    #[error("bulk copy over SSH is forbidden")]
    BulkCopy,
    #[error("exec `{program}` failed: {message}")]
    Failed { program: String, message: String },
    #[error("exec `{program}` exited {status}")]
    Status { program: String, status: i32 },
}

/// Refuse rsync/rclone/ftp/sftp/lftp and recursive scp. SSH is bootstrap exec, not a copy pipe.
pub fn reject_bulk_copy(command: &ExecCommand) -> Result<(), ExecError> {
    let name = command.program_name();
    if matches!(name, "rsync" | "rclone" | "ftp" | "sftp" | "lftp") {
        return Err(ExecError::BulkCopy);
    }
    if name == "scp" && command.args.iter().any(|a| is_scp_recursive_flag(a)) {
        return Err(ExecError::BulkCopy);
    }
    Ok(())
}

/// Short clusters like `-rp` / `-Pr` are recursive; a lone `-P` is scp's port flag.
fn is_scp_recursive_flag(arg: &str) -> bool {
    if arg == "--recursive" {
        return true;
    }
    if arg.starts_with('-') && !arg.starts_with("--") {
        return arg != "-P" && (arg.contains('r') || arg.contains('R'));
    }
    false
}

#[allow(async_fn_in_trait)]
pub trait ExecPort: Send + Sync {
    async fn run(&self, command: &ExecCommand) -> Result<ExecOutput, ExecError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rsync_is_bulk_copy() {
        let cmd = ExecCommand::new("rsync", vec!["-a".into(), "a".into(), "b".into()]);
        assert_eq!(reject_bulk_copy(&cmd), Err(ExecError::BulkCopy));
    }

    #[test]
    fn recursive_scp_is_bulk_copy() {
        let cmd = ExecCommand::new("scp", vec!["-r".into(), "dir".into(), "seedbox:".into()]);
        assert_eq!(reject_bulk_copy(&cmd), Err(ExecError::BulkCopy));
    }

    #[test]
    fn single_file_scp_is_allowed() {
        let cmd = ExecCommand::new(
            "scp",
            vec!["mediaopsd".into(), "seedbox:.local/bin/mediaopsd".into()],
        );
        assert_eq!(reject_bulk_copy(&cmd), Ok(()));
    }

    #[test]
    fn scp_combined_recursive_flags_are_bulk_copy() {
        let cmd = ExecCommand::new("scp", vec!["-rp".into(), "a".into(), "b".into()]);
        assert_eq!(reject_bulk_copy(&cmd), Err(ExecError::BulkCopy));
    }

    #[test]
    fn scp_port_flag_is_allowed() {
        let cmd = ExecCommand::new(
            "scp",
            vec!["-P".into(), "2097".into(), "a".into(), "seedbox:x".into()],
        );
        assert_eq!(reject_bulk_copy(&cmd), Ok(()));
    }

    #[test]
    fn sftp_is_bulk_copy() {
        let cmd = ExecCommand::new("sftp", vec!["seedbox".into()]);
        assert_eq!(reject_bulk_copy(&cmd), Err(ExecError::BulkCopy));
    }
}
