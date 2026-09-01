//! CLI door into the home unix-socket gateway.

use std::path::Path;
use std::sync::Arc;

use mediaops_core::{RemoteEntry, RemoteRef};
use mediaops_net::{IdentityBundle, connect_unix};
use mediaops_proto::gateway_client::GatewayClient;
use mediaops_proto::transfer_client::TransferClient;
use mediaops_proto::{
    ConfigurePoolRequest, GetRangeRequest, ListRequest, PoolStatusRequest, ProbeRangeRequest,
    RemoteRef as WireRef, StatRequest,
};
use tonic::transport::Channel;

use crate::TransferError;
use crate::pull::RangeSource;

pub type HomeChannel = Channel;

pub async fn connect_home(socket: &Path, tls_dir: &Path) -> Result<Channel, TransferError> {
    let id =
        IdentityBundle::from_dir(tls_dir).map_err(|err| TransferError::Net(err.to_string()))?;
    let client = id
        .client_config()
        .map_err(|err| TransferError::Net(err.to_string()))?;
    connect_unix(socket, client)
        .await
        .map_err(|err| TransferError::Net(err.to_string()))
}

pub async fn list_entries(channel: Channel) -> Result<Vec<RemoteEntry>, TransferError> {
    let mut client = TransferClient::new(channel);
    let list = client
        .list(ListRequest {})
        .await
        .map_err(TransferError::from_status)?
        .into_inner();
    list.entries
        .into_iter()
        .map(RemoteEntry::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| TransferError::Wire(err.to_string()))
}

pub async fn stat_entry(
    channel: Channel,
    remote: &RemoteRef,
) -> Result<RemoteEntry, TransferError> {
    let mut client = TransferClient::new(channel);
    let wire = WireRef::try_from(remote).map_err(|err| TransferError::Wire(err.to_string()))?;
    let st = client
        .stat(StatRequest { r#ref: Some(wire) })
        .await
        .map_err(TransferError::from_status)?
        .into_inner();
    let entry = st
        .entry
        .ok_or_else(|| TransferError::Wire("StatResponse.entry missing".into()))?;
    RemoteEntry::try_from(entry).map_err(|err| TransferError::Wire(err.to_string()))
}

pub async fn configure_pool(channel: Channel, n: u32) -> Result<u32, TransferError> {
    let mut client = GatewayClient::new(channel);
    let resp = client
        .configure_pool(ConfigurePoolRequest { n })
        .await
        .map_err(TransferError::from_status)?
        .into_inner();
    Ok(resp.n)
}

pub async fn pool_status(channel: Channel) -> Result<(String, u32), TransferError> {
    let mut client = GatewayClient::new(channel);
    let resp = client
        .pool_status(PoolStatusRequest {})
        .await
        .map_err(TransferError::from_status)?
        .into_inner();
    Ok((resp.endpoint_fingerprint, resp.n))
}

pub async fn probe_range(channel: Channel, max_n: u32) -> Result<u32, TransferError> {
    let mut client = GatewayClient::new(channel);
    let resp = client
        .probe_range(ProbeRangeRequest { max_n })
        .await
        .map_err(TransferError::from_status)?
        .into_inner();
    Ok(resp.n)
}

#[derive(Clone)]
pub struct GrpcRangeSource {
    channel: Channel,
}

impl GrpcRangeSource {
    pub fn new(channel: Channel) -> Self {
        Self { channel }
    }
}

impl RangeSource for GrpcRangeSource {
    async fn get_range(
        &self,
        remote: &RemoteRef,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, TransferError> {
        let mut client = TransferClient::new(self.channel.clone());
        let wire = WireRef::try_from(remote).map_err(|err| TransferError::Wire(err.to_string()))?;
        let mut stream = client
            .get_range(GetRangeRequest {
                r#ref: Some(wire),
                offset,
                len,
            })
            .await
            .map_err(TransferError::from_status)?
            .into_inner();
        let mut buf = Vec::new();
        while let Some(chunk) = stream.message().await.map_err(TransferError::from_status)? {
            if buf.len() as u64 + chunk.data.len() as u64 > len {
                return Err(TransferError::ShortRange {
                    offset,
                    want: len,
                    got: buf.len() as u64 + chunk.data.len() as u64,
                });
            }
            buf.extend_from_slice(&chunk.data);
        }
        if buf.len() as u64 != len {
            return Err(TransferError::ShortRange {
                offset,
                want: len,
                got: buf.len() as u64,
            });
        }
        Ok(buf)
    }
}

/// Keep `Arc` construction at the composition root honest.
pub fn grpc_source(channel: Channel) -> Arc<GrpcRangeSource> {
    Arc::new(GrpcRangeSource::new(channel))
}
