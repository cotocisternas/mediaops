//! Control port. A trait, not I/O: no tokio, tonic, or filesystem.

use std::future::Future;
use std::pin::Pin;

use crate::ExitCode;
use crate::bytes::Bytes;
use crate::desired_state::DesiredState;
use crate::title_id::TitleId;
use crate::walker::RemoteRef;

/// Boxed future so [`ControlPort`] / [`GrabOps`] are dyn-compatible.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Remote Control operations. Async signatures only; the canonical impl lives in `proto`.
pub trait ControlPort: Send + Sync {
    fn df(&self) -> BoxFuture<'_, Result<DfSnapshot, ControlError>>;
    fn unmonitor<'a>(&'a self, title_id: &'a TitleId) -> BoxFuture<'a, Result<(), ControlError>>;
    fn delete_remote<'a>(
        &'a self,
        remote: &'a RemoteRef,
    ) -> BoxFuture<'a, Result<DeleteRemoteOutcome, ControlError>>;
    fn grab_apply<'a>(
        &'a self,
        desired_state_toml: &'a [u8],
    ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>>;
    fn edge_check(&self) -> BoxFuture<'_, Result<EdgeApiReport, ControlError>>;
    fn edge_apply<'a>(
        &'a self,
        desired_state_toml: &'a [u8],
    ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>>;
    fn key_discovery(&self) -> BoxFuture<'_, Result<KeyPresence, ControlError>>;
    fn guard_preview(&self) -> BoxFuture<'_, Result<(), ControlError>>;
}

/// Seedbox-local grabber HTTP. Injected into `net::Seedbox`; `net` does not name HTTP.
pub trait GrabOps: Send + Sync {
    fn grab_apply<'a>(
        &'a self,
        desired: &'a DesiredState,
    ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>>;
    fn key_discovery(&self) -> BoxFuture<'_, Result<KeyPresence, ControlError>>;
    fn edge_api_check(&self) -> BoxFuture<'_, Result<EdgeApiReport, ControlError>>;
    fn edge_apply<'a>(
        &'a self,
        desired: &'a DesiredState,
    ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>>;
}

/// `df` payload for handshake + free space. Wire still splits the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DfSnapshot {
    pub free: Bytes,
    pub semver: String,
    pub proto_package: String,
}

/// Grabber set-diff result. `noop` is a second apply with no desired-state change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrabApplyReport {
    pub noop: bool,
    pub diff: String,
}

/// API-key presence only. Never carries key material (CAP-2).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct KeyPresence {
    pub sonarr_key_present: bool,
    pub radarr_key_present: bool,
    pub lidarr_key_present: bool,
    pub prowlarr_key_present: bool,
    pub sab_key_present: bool,
    pub qbit_key_present: bool,
}

/// EdgeInvariant API-half plus panel fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeApiReport {
    pub fingerprint: String,
    pub invariant_ok: bool,
    pub drift: String,
}

/// Domain error carried across Control. Wire packing lives in `proto`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}", code = exit_code.error_code())]
pub struct ControlError {
    pub exit_code: ExitCode,
    pub message: String,
}

impl ControlError {
    pub fn runtime(message: impl Into<String>) -> Self {
        Self {
            exit_code: ExitCode::Runtime,
            message: message.into(),
        }
    }

    pub fn policy(message: impl Into<String>) -> Self {
        Self {
            exit_code: ExitCode::PolicyRefusal,
            message: message.into(),
        }
    }

    pub fn drift(message: impl Into<String>) -> Self {
        Self {
            exit_code: ExitCode::DriftVerify,
            message: message.into(),
        }
    }
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

    #[test]
    fn control_error_helpers_map_exit_codes() {
        assert_eq!(ControlError::runtime("x").exit_code, ExitCode::Runtime);
        assert_eq!(ControlError::policy("x").exit_code, ExitCode::PolicyRefusal);
        assert_eq!(ControlError::drift("x").exit_code, ExitCode::DriftVerify);
    }

    #[test]
    fn grab_apply_report_and_key_presence_are_data() {
        let report = GrabApplyReport {
            noop: true,
            diff: String::new(),
        };
        assert!(report.noop);
        let keys = KeyPresence {
            sonarr_key_present: true,
            ..KeyPresence::default()
        };
        assert!(keys.sonarr_key_present);
        assert!(!keys.sab_key_present);
        let edge = EdgeApiReport {
            fingerprint: "abc".into(),
            invariant_ok: false,
            drift: "bind-to-star".into(),
        };
        assert!(!edge.invariant_ok);
        let df = DfSnapshot {
            free: crate::Bytes::new(1),
            semver: "0.1.0".into(),
            proto_package: "mediaops.v1".into(),
        };
        assert_eq!(df.free.get(), 1);
    }
}
