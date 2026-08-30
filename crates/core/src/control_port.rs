//! Control port. A trait, not I/O: no tokio, tonic, or filesystem.

use crate::ExitCode;
use crate::bytes::Bytes;
use crate::title_id::TitleId;
use crate::walker::RemoteRef;

/// Remote Control operations. Async signatures only; the canonical impl lives in `proto`.
#[allow(async_fn_in_trait)]
pub trait ControlPort: Send + Sync {
    async fn df(&self) -> Result<Bytes, ControlError>;
    async fn unmonitor(&self, title_id: &TitleId) -> Result<(), ControlError>;
    async fn delete_remote(&self, remote: &RemoteRef) -> Result<DeleteRemoteOutcome, ControlError>;
    async fn grab_apply(&self) -> Result<(), ControlError>;
    async fn edge_check(&self) -> Result<(), ControlError>;
    async fn key_discovery(&self) -> Result<(), ControlError>;
    async fn guard_preview(&self) -> Result<(), ControlError>;
}

/// Domain error carried across Control. Wire packing lives in `proto`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}", code = exit_code.error_code())]
pub struct ControlError {
    pub exit_code: ExitCode,
    pub message: String,
}

/// Outcome of `DeleteRemote`. `SkippedSeeding` is data, not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteRemoteOutcome {
    Deleted,
    SkippedSeeding,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_remote_outcome_match_is_exhaustive() {
        let outcomes = [
            DeleteRemoteOutcome::Deleted,
            DeleteRemoteOutcome::SkippedSeeding,
        ];
        for outcome in outcomes {
            let n = match outcome {
                DeleteRemoteOutcome::Deleted => 1,
                DeleteRemoteOutcome::SkippedSeeding => 2,
            };
            assert!(n == 1 || n == 2);
        }
        assert_eq!(outcomes.len(), 2);
    }
}
