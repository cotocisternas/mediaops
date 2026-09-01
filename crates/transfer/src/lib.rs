//! PullFile crate. Also the CLI's legal door into `net` (AD-2).

mod home;
mod prune;
mod pull;
mod schedule;
mod sidecar;

pub use home::{
    GrpcRangeSource, HomeChannel, configure_pool, connect_home, grpc_source, list_entries,
    pool_status, probe_range, stat_entry,
};
pub use mediaops_net::{
    ChannelPool, DaemonRole, HomeGateway, IdentityBundle, NetError, Seedbox, connect_pool,
    connect_tcp, connect_unix, mint, probe_range_n, serve_home_unix, serve_tcp, serve_unix,
};
pub use prune::{dir_is_sacred, prune_empty_incoming};
pub use pull::{PullOutcome, PullSpec, RangeSource, pull_file};
pub use schedule::{PendingFile, plan_ranges, take_slots};
pub use sidecar::{SIDECAR_VERSION, Sidecar};

use std::io;
use std::path::Path;

use mediaops_proto::REASON_RESOURCE_EXHAUSTED;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransferError {
    #[error("io at `{path}`: {message}")]
    Io { path: String, message: String },
    #[error("sidecar: {0}")]
    Sidecar(String),
    #[error("path: {0}")]
    Path(String),
    #[error("net: {0}")]
    Net(String),
    #[error("wire: {0}")]
    Wire(String),
    #[error("join: {0}")]
    Join(String),
    #[error("short range at {offset}: want {want} got {got}")]
    ShortRange { offset: u64, want: u64, got: u64 },
    #[error("channel pool exhausted")]
    Exhausted,
    #[error("{0}")]
    Rpc(String),
}

impl TransferError {
    pub fn io(path: &Path, err: io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            message: err.to_string(),
        }
    }

    pub fn from_status(status: tonic::Status) -> Self {
        if status.code() == tonic::Code::ResourceExhausted
            || mediaops_proto::error_detail_from_status(&status)
                .ok()
                .is_some_and(|d| d.reason == REASON_RESOURCE_EXHAUSTED)
        {
            return Self::Exhausted;
        }
        Self::Rpc(status.message().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{ControlError, ExitCode};
    use mediaops_proto::{ErrorDetail, resource_exhausted_detail, status_from_error_detail};
    use std::io::{Error as IoError, ErrorKind};
    use std::path::Path;

    #[test]
    fn io_preserves_path_and_message() {
        let err = TransferError::io(
            Path::new("/tmp/partial"),
            IoError::from(ErrorKind::NotFound),
        );
        match err {
            TransferError::Io { path, message } => {
                assert_eq!(path, "/tmp/partial");
                assert!(!message.is_empty());
            }
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn from_status_resource_exhausted_reason_is_exhausted() {
        let status = status_from_error_detail(&resource_exhausted_detail("n+1"));
        assert_eq!(status.code(), tonic::Code::ResourceExhausted);
        assert!(matches!(
            TransferError::from_status(status),
            TransferError::Exhausted
        ));
    }

    #[test]
    fn from_status_other_codes_are_rpc() {
        let status = status_from_error_detail(&ErrorDetail::from(ControlError {
            exit_code: ExitCode::Runtime,
            message: "walker boom".into(),
        }));
        match TransferError::from_status(status) {
            TransferError::Rpc(message) => assert_eq!(message, "walker boom"),
            other => panic!("expected Rpc, got {other:?}"),
        }
    }
}
