//! PullFile crate. Also the CLI's legal door into `net` (AD-2).

mod home;
mod prune;
mod pull;
mod schedule;
mod sidecar;

pub use home::{
    configure_pool, connect_home, grpc_source, list_entries, pool_status, probe_range, stat_entry,
    GrpcRangeSource,
};
pub use mediaops_net::{
    ChannelPool, DaemonRole, HomeGateway, IdentityBundle, NetError, connect_pool, connect_tcp,
    connect_unix, mint, probe_range_n, serve_home_unix, serve_tcp, serve_unix,
};
pub use prune::{dir_is_sacred, prune_empty_incoming};
pub use pull::{pull_file, PullOutcome, PullSpec, RangeSource};
pub use schedule::{plan_ranges, take_slots, PendingFile};
pub use sidecar::{Sidecar, SIDECAR_VERSION};

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
