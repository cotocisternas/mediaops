//! CLI door into the home unix-socket gateway.

use std::path::Path;
use std::sync::Arc;

use mediaops_core::{RemoteEntry, RemoteRef};
use mediaops_net::{IdentityBundle, connect_unix};
use mediaops_proto::gateway_service_client::GatewayServiceClient;
use mediaops_proto::transfer_service_client::TransferServiceClient;
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
    let mut client = TransferServiceClient::new(channel);
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
    let mut client = TransferServiceClient::new(channel);
    let wire = WireRef::try_from(remote).map_err(|err| TransferError::Wire(err.to_string()))?;
    let st = client
        .stat(StatRequest {
            remote_ref: Some(wire),
        })
        .await
        .map_err(TransferError::from_status)?
        .into_inner();
    let entry = st
        .entry
        .ok_or_else(|| TransferError::Wire("StatResponse.entry missing".into()))?;
    RemoteEntry::try_from(entry).map_err(|err| TransferError::Wire(err.to_string()))
}

pub async fn configure_pool(channel: Channel, n: u32) -> Result<u32, TransferError> {
    let mut client = GatewayServiceClient::new(channel);
    let resp = client
        .configure_pool(ConfigurePoolRequest { concurrency: n })
        .await
        .map_err(TransferError::from_status)?
        .into_inner();
    Ok(resp.concurrency)
}

pub async fn pool_status(channel: Channel) -> Result<(String, u32), TransferError> {
    let mut client = GatewayServiceClient::new(channel);
    let resp = client
        .pool_status(PoolStatusRequest {})
        .await
        .map_err(TransferError::from_status)?
        .into_inner();
    Ok((resp.endpoint_fingerprint, resp.concurrency))
}

pub async fn probe_range(channel: Channel, max_n: u32) -> Result<u32, TransferError> {
    let mut client = GatewayServiceClient::new(channel);
    let resp = client
        .probe_range(ProbeRangeRequest {
            max_concurrency: max_n,
        })
        .await
        .map_err(TransferError::from_status)?
        .into_inner();
    Ok(resp.concurrency)
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
        let mut client = TransferServiceClient::new(self.channel.clone());
        let wire = WireRef::try_from(remote).map_err(|err| TransferError::Wire(err.to_string()))?;
        let mut stream = client
            .get_range(GetRangeRequest {
                remote_ref: Some(wire),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HomeGateway, TransferError, mint, serve_home_unix, serve_tcp};
    use mediaops_core::{Allowlist, Grabber, UnderlayMode, endpoint_fingerprint};
    use mediaops_net::Seedbox;
    use std::io::Write;
    use std::sync::Mutex;
    use tokio::net::{TcpListener, UnixListener};

    static NET_TEST: Mutex<()> = Mutex::new(());

    fn serial_net() -> std::sync::MutexGuard<'static, ()> {
        NET_TEST.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-xfer-home-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn write_file(path: &std::path::Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        let mut f = std::fs::File::create(path).expect("create");
        f.write_all(bytes).expect("write");
    }

    fn seedbox_for(root: std::path::PathBuf) -> Seedbox {
        let mut allowlist = Allowlist::new();
        allowlist.add_root("seedbox", root).expect("root");
        Seedbox::new(allowlist, "0.1.0", Grabber::None)
    }

    async fn wait_home(socket: &Path, tls_dir: &Path) -> Channel {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match connect_home(socket, tls_dir).await {
                Ok(channel) => return channel,
                Err(err) => {
                    if tokio::time::Instant::now() >= deadline {
                        panic!("connect_home: {err}");
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
        }
    }

    async fn start_pair() -> (
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        tokio::task::JoinHandle<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let root = scratch("tree");
        write_file(&root.join("a.bin"), b"abcdefghij");
        let id = mint().expect("mint");
        let tls_dir = scratch("tls");
        id.write_to_dir(&tls_dir).expect("write tls");
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind tcp");
        let addr = tcp.local_addr().expect("addr");
        let seed = seedbox_for(root.clone());
        let server = id.server_config().expect("server");
        let seed_task = tokio::spawn(async move {
            let _ = serve_tcp(tcp, server, seed).await;
        });
        let client = id.client_config().expect("client");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match mediaops_net::connect_tcp(addr, client.clone()).await {
                Ok(_) => break,
                Err(err) => {
                    if tokio::time::Instant::now() >= deadline {
                        panic!("tcp connect: {err}");
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
        }
        let fingerprint = endpoint_fingerprint(&addr.to_string(), UnderlayMode::Direct);
        let gateway = HomeGateway::connect(addr, client, fingerprint, 1)
            .await
            .expect("gw");
        let sock = scratch("uds").join("mediaops.sock");
        let unix = UnixListener::bind(&sock).expect("bind uds");
        let uds_server = id.server_config().expect("server");
        let uds_task = tokio::spawn(async move {
            let _ = serve_home_unix(unix, uds_server, gateway).await;
        });
        (sock, tls_dir, root, seed_task, uds_task)
    }

    #[tokio::test]
    async fn connect_home_lists_stats_and_fetches_range() {
        let _serial = serial_net();
        let (sock, tls_dir, root, seed_task, uds_task) = start_pair().await;
        let channel = wait_home(&sock, &tls_dir).await;
        let entries = list_entries(channel.clone()).await.expect("list");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].r#ref().root_id(), "seedbox");
        assert_eq!(entries[0].r#ref().rel_path(), Path::new("a.bin"));
        assert_eq!(entries[0].len(), 10);

        let st = stat_entry(channel.clone(), entries[0].r#ref())
            .await
            .expect("stat");
        assert_eq!(st.len(), 10);

        let src = grpc_source(channel.clone());
        let bytes = src
            .get_range(entries[0].r#ref(), 1, 4)
            .await
            .expect("range");
        assert_eq!(bytes, b"bcde");

        let short = src
            .get_range(entries[0].r#ref(), 0, 100)
            .await
            .expect_err("short");
        match short {
            TransferError::ShortRange { offset, want, got } => {
                assert_eq!(offset, 0);
                assert_eq!(want, 100);
                assert_eq!(got, 10);
            }
            other => panic!("expected ShortRange, got {other:?}"),
        }

        seed_task.abort();
        uds_task.abort();
        let _ = std::fs::remove_file(&sock);
        let _ = std::fs::remove_dir_all(&tls_dir);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn pool_status_configure_and_probe_round_trip() {
        let _serial = serial_net();
        let (sock, tls_dir, root, seed_task, uds_task) = start_pair().await;
        let channel = wait_home(&sock, &tls_dir).await;
        let (fingerprint, n) = pool_status(channel.clone()).await.expect("status");
        assert!(!fingerprint.is_empty());
        assert_eq!(n, 1);
        let configured = configure_pool(channel.clone(), 2).await.expect("configure");
        assert_eq!(configured, 2);
        let probed = probe_range(channel, 1).await.expect("probe");
        assert_eq!(probed, 1);
        seed_task.abort();
        uds_task.abort();
        let _ = std::fs::remove_file(&sock);
        let _ = std::fs::remove_dir_all(&tls_dir);
        let _ = std::fs::remove_dir_all(root);
    }
}
