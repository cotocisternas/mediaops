//! Home overlay gateway: UDS Control+Transfer proxy plus WAN channel pool (AD-4, AD-12).

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use mediaops_core::{ControlError, ExitCode};
use mediaops_proto::control_client::ControlClient;
use mediaops_proto::control_server::Control;
use mediaops_proto::gateway_server::Gateway;
use mediaops_proto::transfer_client::TransferClient;
use mediaops_proto::transfer_server::Transfer;
use mediaops_proto::{
    ConfigurePoolRequest, ConfigurePoolResponse, DeleteRemoteRequest, DeleteRemoteResponse,
    DfRequest, DfResponse, EdgeCheckRequest, EdgeCheckResponse, ErrorDetail, GetRangeRequest,
    GetRangeResponse, GrabApplyRequest, GrabApplyResponse, GuardPreviewRequest,
    GuardPreviewResponse, KeyDiscoveryRequest, KeyDiscoveryResponse, ListRequest, ListResponse,
    PoolStatusRequest, PoolStatusResponse, ProbeRangeRequest, ProbeRangeResponse, StatRequest,
    StatResponse, UnmonitorRequest, UnmonitorResponse, resource_exhausted_detail,
    status_from_error_detail,
};
use rustls::ClientConfig;
use tokio::sync::Mutex;
use tokio_stream::Stream;
use tonic::transport::Channel;
use tonic::{Request, Response, Status};

use crate::pool::{ChannelPool, SlotGuard};
use crate::{NetError, connect_pool, connect_tcp, probe_range_n};

#[derive(Clone)]
pub struct HomeGateway {
    addr: SocketAddr,
    client: Arc<ClientConfig>,
    fingerprint: String,
    control: Channel,
    pool: Arc<Mutex<ChannelPool>>,
}

impl HomeGateway {
    pub async fn connect(
        addr: SocketAddr,
        client: Arc<ClientConfig>,
        fingerprint: String,
        n: usize,
    ) -> Result<Self, NetError> {
        let control = connect_tcp(addr, client.clone()).await?;
        let pool = connect_pool(addr, client.clone(), n).await?;
        Ok(Self {
            addr,
            client,
            fingerprint,
            control,
            pool: Arc::new(Mutex::new(pool)),
        })
    }

    pub fn endpoint_fingerprint(&self) -> &str {
        &self.fingerprint
    }

    async fn configure(&self, n: usize) -> Result<u32, NetError> {
        crate::check_pool_n(n)?;
        {
            let pool = self.pool.lock().await;
            if pool.len() == n {
                return Ok(n as u32);
            }
        }
        let pool = connect_pool(self.addr, self.client.clone(), n).await?;
        *self.pool.lock().await = pool;
        Ok(n as u32)
    }

    #[cfg(test)]
    async fn checkout_slot(&self) -> Result<SlotGuard, NetError> {
        self.pool.lock().await.try_checkout()
    }

    async fn pool_n(&self) -> u32 {
        self.pool.lock().await.len() as u32
    }

    fn exhausted() -> Status {
        status_from_error_detail(&resource_exhausted_detail("channel pool exhausted"))
    }

    fn net_status(err: NetError) -> Status {
        if matches!(err, NetError::Exhausted) {
            return Self::exhausted();
        }
        status_from_error_detail(&ErrorDetail::from(ControlError {
            exit_code: ExitCode::Runtime,
            message: err.to_string(),
        }))
    }
}

struct GuardedRange {
    inner: tonic::Streaming<GetRangeResponse>,
    _slot: SlotGuard,
}

impl Stream for GuardedRange {
    type Item = Result<GetRangeResponse, Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        Pin::new(&mut this.inner).poll_next(cx)
    }
}

#[tonic::async_trait]
impl Control for HomeGateway {
    async fn df(&self, request: Request<DfRequest>) -> Result<Response<DfResponse>, Status> {
        ControlClient::new(self.control.clone()).df(request).await
    }

    async fn unmonitor(
        &self,
        request: Request<UnmonitorRequest>,
    ) -> Result<Response<UnmonitorResponse>, Status> {
        ControlClient::new(self.control.clone())
            .unmonitor(request)
            .await
    }

    async fn delete_remote(
        &self,
        request: Request<DeleteRemoteRequest>,
    ) -> Result<Response<DeleteRemoteResponse>, Status> {
        ControlClient::new(self.control.clone())
            .delete_remote(request)
            .await
    }

    async fn grab_apply(
        &self,
        request: Request<GrabApplyRequest>,
    ) -> Result<Response<GrabApplyResponse>, Status> {
        ControlClient::new(self.control.clone())
            .grab_apply(request)
            .await
    }

    async fn edge_check(
        &self,
        request: Request<EdgeCheckRequest>,
    ) -> Result<Response<EdgeCheckResponse>, Status> {
        ControlClient::new(self.control.clone())
            .edge_check(request)
            .await
    }

    async fn key_discovery(
        &self,
        request: Request<KeyDiscoveryRequest>,
    ) -> Result<Response<KeyDiscoveryResponse>, Status> {
        ControlClient::new(self.control.clone())
            .key_discovery(request)
            .await
    }

    async fn guard_preview(
        &self,
        request: Request<GuardPreviewRequest>,
    ) -> Result<Response<GuardPreviewResponse>, Status> {
        ControlClient::new(self.control.clone())
            .guard_preview(request)
            .await
    }
}

#[tonic::async_trait]
impl Transfer for HomeGateway {
    async fn list(&self, request: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        TransferClient::new(self.control.clone())
            .list(request)
            .await
    }

    async fn stat(&self, request: Request<StatRequest>) -> Result<Response<StatResponse>, Status> {
        TransferClient::new(self.control.clone())
            .stat(request)
            .await
    }

    type GetRangeStream =
        Pin<Box<dyn Stream<Item = Result<GetRangeResponse, Status>> + Send + 'static>>;

    async fn get_range(
        &self,
        request: Request<GetRangeRequest>,
    ) -> Result<Response<Self::GetRangeStream>, Status> {
        let slot = {
            let pool = self.pool.lock().await;
            pool.try_checkout().map_err(Self::net_status)?
        };
        let mut client = TransferClient::new(slot.channel().clone());
        let stream = match client.get_range(request).await {
            Ok(response) => response.into_inner(),
            Err(status) => return Err(status),
        };
        Ok(Response::new(Box::pin(GuardedRange {
            inner: stream,
            _slot: slot,
        })))
    }
}

#[tonic::async_trait]
impl Gateway for HomeGateway {
    async fn configure_pool(
        &self,
        request: Request<ConfigurePoolRequest>,
    ) -> Result<Response<ConfigurePoolResponse>, Status> {
        let n = request.into_inner().n;
        let n = self.configure(n as usize).await.map_err(Self::net_status)?;
        Ok(Response::new(ConfigurePoolResponse { n }))
    }

    async fn pool_status(
        &self,
        _request: Request<PoolStatusRequest>,
    ) -> Result<Response<PoolStatusResponse>, Status> {
        Ok(Response::new(PoolStatusResponse {
            endpoint_fingerprint: self.fingerprint.clone(),
            n: self.pool_n().await,
        }))
    }

    async fn probe_range(
        &self,
        request: Request<ProbeRangeRequest>,
    ) -> Result<Response<ProbeRangeResponse>, Status> {
        let max_n = request.into_inner().max_n.max(1);
        let n = probe_range_n(self.addr, self.client.clone(), max_n)
            .await
            .map_err(Self::net_status)?;
        Ok(Response::new(ProbeRangeResponse { n }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        IdentityBundle, Seedbox, connect_unix, mint, serve_home_unix, serve_tcp, tcp_connect_count,
    };
    use mediaops_core::{Allowlist, Grabber, UnderlayMode, endpoint_fingerprint};
    use mediaops_proto::gateway_client::GatewayClient;
    use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256};
    use std::io::Write;
    use std::path::Path;
    use std::sync::Mutex;
    use tokio::net::{TcpListener, UnixListener};

    static NET_TEST: Mutex<()> = Mutex::new(());

    fn serial_net() -> std::sync::MutexGuard<'static, ()> {
        NET_TEST.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-gw-{tag}-{}-{}",
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

    async fn wait_tcp(addr: SocketAddr, client: Arc<ClientConfig>) -> Channel {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match connect_tcp(addr, client.clone()).await {
                Ok(channel) => return channel,
                Err(err) => {
                    if tokio::time::Instant::now() >= deadline {
                        panic!("tcp connect: {err}");
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
        }
    }

    async fn wait_unix(path: &Path, client: Arc<ClientConfig>) -> Channel {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match connect_unix(path, client.clone()).await {
                Ok(channel) => return channel,
                Err(err) => {
                    if tokio::time::Instant::now() >= deadline {
                        panic!("unix connect: {err}");
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
        }
    }

    async fn start_pair() -> (
        SocketAddr,
        std::path::PathBuf,
        IdentityBundle,
        tokio::task::JoinHandle<()>,
        tokio::task::JoinHandle<()>,
        std::path::PathBuf,
    ) {
        let root = scratch("tree");
        write_file(&root.join("a.bin"), b"abcdefghij");
        let id = mint().expect("mint");
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind tcp");
        let addr = tcp.local_addr().expect("addr");
        let seed = seedbox_for(root.clone());
        let server = id.server_config().expect("server");
        let seed_task = tokio::spawn(async move {
            let _ = serve_tcp(tcp, server, seed).await;
        });
        let client = id.client_config().expect("client");
        let _ = wait_tcp(addr, client.clone()).await;
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
        (addr, sock, id, seed_task, uds_task, root)
    }

    #[test]
    fn mint_key_is_still_p256() {
        let id = mint().expect("mint");
        let server_key = KeyPair::from_pem(&id.server_key_pem).expect("server key");
        assert!(server_key.is_compatible(&PKCS_ECDSA_P256_SHA256));
    }

    #[tokio::test]
    async fn home_uds_proxies_control_and_range() {
        let _serial = serial_net();
        let (_addr, sock, id, seed_task, uds_task, root) = start_pair().await;
        let client = id.client_config().expect("client");
        let ch = wait_unix(&sock, client).await;
        let mut control = ControlClient::new(ch.clone());
        let df = control.df(DfRequest {}).await.expect("df").into_inner();
        assert!(df.free_bytes > 0);

        let mut transfer = TransferClient::new(ch.clone());
        let list = transfer
            .list(ListRequest {})
            .await
            .expect("list")
            .into_inner();
        assert_eq!(list.entries.len(), 1);
        let mut stream = transfer
            .get_range(GetRangeRequest {
                r#ref: list.entries[0].r#ref.clone(),
                offset: 1,
                len: 4,
            })
            .await
            .expect("range")
            .into_inner();
        let mut body = Vec::new();
        while let Some(chunk) = stream.message().await.expect("msg") {
            body.extend_from_slice(&chunk.data);
        }
        assert_eq!(body, b"bcde");

        let mut gateway = GatewayClient::new(ch);
        let status = gateway
            .pool_status(PoolStatusRequest {})
            .await
            .expect("status")
            .into_inner();
        assert!(!status.endpoint_fingerprint.is_empty());
        assert_eq!(status.n, 1);

        seed_task.abort();
        uds_task.abort();
        let _ = std::fs::remove_file(&sock);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn configure_pool_opens_n_wan_channels_and_refuses_n_plus_one() {
        let _serial = serial_net();
        let root = scratch("pool-n");
        let blob = vec![b'x'; 256 * 1024];
        write_file(&root.join("a.bin"), &blob);
        let id = mint().expect("mint");
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind tcp");
        let addr = tcp.local_addr().expect("addr");
        let seed = seedbox_for(root.clone());
        let server = id.server_config().expect("server");
        let seed_task = tokio::spawn(async move {
            let _ = serve_tcp(tcp, server, seed).await;
        });
        let client = id.client_config().expect("client");
        let _ = wait_tcp(addr, client.clone()).await;
        let fingerprint = endpoint_fingerprint(&addr.to_string(), UnderlayMode::Direct);
        let gateway = HomeGateway::connect(addr, client.clone(), fingerprint, 1)
            .await
            .expect("gw");
        let holder = gateway.clone();
        let sock = scratch("uds-n").join("mediaops.sock");
        let unix = UnixListener::bind(&sock).expect("bind uds");
        let uds_server = id.server_config().expect("server");
        let uds_task = tokio::spawn(async move {
            let _ = serve_home_unix(unix, uds_server, gateway).await;
        });
        let ch = wait_unix(&sock, client).await;
        let mut gateway = GatewayClient::new(ch.clone());
        let before = tcp_connect_count();
        let configured = gateway
            .configure_pool(ConfigurePoolRequest { n: 3 })
            .await
            .expect("configure")
            .into_inner();
        assert_eq!(configured.n, 3);
        let opened = tcp_connect_count() - before;
        assert_eq!(
            opened, 3,
            "ConfigurePool(3) must open 3 WAN TCP channels, opened {opened}"
        );

        let before_again = tcp_connect_count();
        let again = gateway
            .configure_pool(ConfigurePoolRequest { n: 3 })
            .await
            .expect("configure again")
            .into_inner();
        assert_eq!(again.n, 3);
        assert_eq!(
            tcp_connect_count() - before_again,
            0,
            "ConfigurePool with the same N must not open new WAN channels"
        );

        let mut transfer = TransferClient::new(ch.clone());
        let list = transfer
            .list(ListRequest {})
            .await
            .expect("list")
            .into_inner();
        let r#ref = list.entries[0].r#ref.clone();
        let _ = blob;

        let mut held = Vec::new();
        for _ in 0..3 {
            held.push(holder.checkout_slot().await.expect("slot"));
        }
        let fourth = TransferClient::new(ch.clone())
            .get_range(GetRangeRequest {
                r#ref: r#ref.clone(),
                offset: 0,
                len: 1,
            })
            .await;
        let err = fourth.expect_err("n+1 must be refused");
        assert_eq!(err.code(), tonic::Code::ResourceExhausted);
        drop(held);

        seed_task.abort();
        uds_task.abort();
        let _ = std::fs::remove_file(&sock);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn proxy_forwards_upstream_status_details() {
        let _serial = serial_net();
        let (_addr, sock, id, seed_task, uds_task, root) = start_pair().await;
        let client = id.client_config().expect("client");
        let ch = wait_unix(&sock, client).await;
        let mut transfer = TransferClient::new(ch);
        let err = transfer
            .stat(StatRequest {
                r#ref: Some(mediaops_proto::RemoteRef {
                    root_id: "nope".into(),
                    rel_path: "missing.bin".into(),
                }),
            })
            .await
            .expect_err("unknown root");
        assert!(
            !err.details().is_empty(),
            "gateway must not strip Status details"
        );
        seed_task.abort();
        uds_task.abort();
        let _ = std::fs::remove_file(&sock);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn two_overlapping_uds_get_ranges_complete_when_pool_is_n() {
        let _serial = serial_net();
        let root = scratch("mux");
        write_file(&root.join("a.bin"), &vec![b'x'; 64 * 1024]);
        let id = mint().expect("mint");
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind tcp");
        let addr = tcp.local_addr().expect("addr");
        let seed = seedbox_for(root.clone());
        let server = id.server_config().expect("server");
        let seed_task = tokio::spawn(async move {
            let _ = serve_tcp(tcp, server, seed).await;
        });
        let client = id.client_config().expect("client");
        let _ = wait_tcp(addr, client.clone()).await;
        let fingerprint = endpoint_fingerprint(&addr.to_string(), UnderlayMode::Direct);
        let gateway = HomeGateway::connect(addr, client.clone(), fingerprint, 1)
            .await
            .expect("gw");
        let sock = scratch("uds-mux").join("mediaops.sock");
        let unix = UnixListener::bind(&sock).expect("bind uds");
        let uds_server = id.server_config().expect("server");
        let uds_task = tokio::spawn(async move {
            let _ = serve_home_unix(unix, uds_server, gateway).await;
        });
        let ch = wait_unix(&sock, client).await;
        let mut gateway = GatewayClient::new(ch.clone());
        gateway
            .configure_pool(ConfigurePoolRequest { n: 2 })
            .await
            .expect("configure");

        let mut transfer = TransferClient::new(ch.clone());
        let list = transfer
            .list(ListRequest {})
            .await
            .expect("list")
            .into_inner();
        let r#ref = list.entries[0].r#ref.clone();
        let first = TransferClient::new(ch.clone())
            .get_range(GetRangeRequest {
                r#ref: r#ref.clone(),
                offset: 0,
                len: 8,
            })
            .await
            .expect("first range");
        let second = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            TransferClient::new(ch.clone()).get_range(GetRangeRequest {
                r#ref,
                offset: 8,
                len: 8,
            }),
        )
        .await
        .expect("UDS must multiplex a second GetRange while the first is open")
        .expect("second range");
        drop(first);
        drop(second);
        seed_task.abort();
        uds_task.abort();
        let _ = std::fs::remove_file(&sock);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn gateway_probe_range_returns_n() {
        let _serial = serial_net();
        let root = scratch("probe");
        write_file(&root.join("a.bin"), &vec![b'y'; 256 * 1024]);
        let id = mint().expect("mint");
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind tcp");
        let addr = tcp.local_addr().expect("addr");
        let seed = seedbox_for(root.clone());
        let server = id.server_config().expect("server");
        let seed_task = tokio::spawn(async move {
            let _ = serve_tcp(tcp, server, seed).await;
        });
        let client = id.client_config().expect("client");
        let _ = wait_tcp(addr, client.clone()).await;
        let fingerprint = endpoint_fingerprint(&addr.to_string(), UnderlayMode::Direct);
        let gateway = HomeGateway::connect(addr, client.clone(), fingerprint, 1)
            .await
            .expect("gw");
        let sock = scratch("uds-probe").join("mediaops.sock");
        let unix = UnixListener::bind(&sock).expect("bind uds");
        let uds_server = id.server_config().expect("server");
        let uds_task = tokio::spawn(async move {
            let _ = serve_home_unix(unix, uds_server, gateway).await;
        });
        let ch = wait_unix(&sock, client).await;
        let mut gateway = GatewayClient::new(ch);
        let probed = gateway
            .probe_range(ProbeRangeRequest { max_n: 2 })
            .await
            .expect("probe")
            .into_inner();
        assert!(probed.n >= 1, "ProbeRange n={}", probed.n);
        seed_task.abort();
        uds_task.abort();
        let _ = std::fs::remove_file(&sock);
        let _ = std::fs::remove_dir_all(root);
    }
}
