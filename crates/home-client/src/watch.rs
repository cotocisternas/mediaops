//! Typed Home watch stream. Decoding lives in proto.

use mediaops_proto::WatchEvent;
use mediaops_proto::home::WatchResponse;
use mediaops_proto::watch_event_from_wire;

use crate::ClientError;

/// Streaming watch with decoded Added/Modified/Deleted messages.
pub struct HomeWatch {
    inner: tonic::Streaming<WatchResponse>,
}

impl HomeWatch {
    pub(crate) fn new(inner: tonic::Streaming<WatchResponse>) -> Self {
        Self { inner }
    }

    /// Next decoded event. `Ok(None)` is a clean stream end.
    pub async fn message(&mut self) -> Result<Option<WatchEvent>, ClientError> {
        match self.inner.message().await {
            Ok(None) => Ok(None),
            Ok(Some(resp)) => Ok(Some(watch_event_from_wire(resp)?)),
            Err(status) => Err(ClientError::from_status(status)),
        }
    }
}
