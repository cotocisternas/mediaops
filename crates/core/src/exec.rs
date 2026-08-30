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

/// Refuse rsync/rclone/ftp and recursive scp. SSH is bootstrap exec, not a copy pipe.
pub fn reject_bulk_copy(command: &ExecCommand) -> Result<(), ExecError> {
    let name = command.program_name();
    if matches!(name, "rsync" | "rclone" | "ftp") {
        return Err(ExecError::BulkCopy);
    }
    if name == "scp"
        && command
            .args
            .iter()
            .any(|a| a == "-r" || a == "-R" || a == "--recursive")
    {
        return Err(ExecError::BulkCopy);
    }
    Ok(())
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
}
