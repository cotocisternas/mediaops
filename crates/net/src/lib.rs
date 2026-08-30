//! rustls identity, channel pool, UDS/TCP serve (AD-12, AD-14).

mod listen;
mod mint;
mod pool;
mod seedbox;

pub use listen::{connect_tcp, connect_unix, serve_tcp, serve_unix};
pub use mint::{IdentityBundle, SERVER_NAME, mint};
pub use pool::{ChannelPool, SlotGuard};
pub use seedbox::Seedbox;

use std::net::SocketAddr;
use std::sync::Arc;

use rustls::ClientConfig;
use tonic::transport::Channel;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NetError {
    #[error("mint failed: {0}")]
    Mint(String),
    #[error("tls: {0}")]
    Tls(String),
    #[error("io: {0}")]
    Io(String),
    #[error("serve: {0}")]
    Serve(String),
    #[error("connect: {0}")]
    Connect(String),
    #[error("pool: {0}")]
    Pool(String),
    #[error("channel pool exhausted")]
    Exhausted,
    #[error("role `{0}` is a designed-unused mode of this binary")]
    UnusedRole(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonRole {
    Seedbox,
    Home,
    ReverseConnect,
}

impl DaemonRole {
    pub fn parse(name: &str) -> Result<Self, NetError> {
        match name {
            "seedbox" => Ok(Self::Seedbox),
            "home" => Ok(Self::Home),
            "reverse-connect" | "reverse_connect" => Ok(Self::ReverseConnect),
            other => Err(NetError::Pool(format!("unknown role `{other}`"))),
        }
    }

    pub fn ensure_seedbox(self) -> Result<(), NetError> {
        match self {
            Self::Seedbox => Ok(()),
            Self::Home => Err(NetError::UnusedRole("home".into())),
            Self::ReverseConnect => Err(NetError::UnusedRole("reverse-connect".into())),
        }
    }
}

pub async fn connect_pool(
    addr: SocketAddr,
    client: Arc<ClientConfig>,
    n: usize,
) -> Result<ChannelPool, NetError> {
    let mut channels = Vec::with_capacity(n);
    for _ in 0..n {
        channels.push(connect_tcp(addr, client.clone()).await?);
    }
    ChannelPool::new(channels)
}

pub async fn probe_range_n(
    addr: SocketAddr,
    client: Arc<ClientConfig>,
    max_n: u32,
) -> Result<u32, NetError> {
    use mediaops_core::plateau_n;
    use mediaops_proto::transfer_client::TransferClient;
    use mediaops_proto::{GetRangeRequest, ListRequest};
    use tokio::time::Instant;

    let mut samples = Vec::new();
    for n in 1..=max_n {
        let pool = connect_pool(addr, client.clone(), n as usize).await?;
        let mut clients: Vec<TransferClient<Channel>> = Vec::new();
        for _ in 0..n {
            let slot = pool.try_checkout()?;
            clients.push(TransferClient::new(slot.channel().clone()));
        }
        let mut listing = clients[0]
            .list(ListRequest {})
            .await
            .map_err(|err| NetError::Connect(err.to_string()))?
            .into_inner();
        let Some(entry) = listing.entries.pop() else {
            return Err(NetError::Connect("probe listing was empty".into()));
        };
        let started = Instant::now();
        let mut total = 0_u64;
        let mut joins = Vec::new();
        for mut client in clients {
            let r#ref = entry.r#ref.clone();
            joins.push(tokio::spawn(async move {
                let mut stream = client
                    .get_range(GetRangeRequest {
                        r#ref,
                        offset: 0,
                        len: 1024 * 1024,
                    })
                    .await
                    .map_err(|err| err.to_string())?
                    .into_inner();
                let mut n = 0_u64;
                while let Some(chunk) = stream.message().await.map_err(|err| err.to_string())? {
                    n += chunk.data.len() as u64;
                }
                Ok::<u64, String>(n)
            }));
        }
        for join in joins {
            total += join
                .await
                .map_err(|err| NetError::Connect(err.to_string()))?
                .map_err(NetError::Connect)?;
        }
        let elapsed_ms = started.elapsed().as_millis().max(1) as u64;
        samples.push((n, total.saturating_mul(1000) / elapsed_ms));
    }
    plateau_n(&samples).map_err(|err| NetError::Pool(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{Allowlist, ControlPort, Grabber};
    use mediaops_proto::control_client::ControlClient;
    use mediaops_proto::transfer_client::TransferClient;
    use mediaops_proto::{GetRangeRequest, ListRequest, PROTO_PACKAGE};
    use std::io::Write;
    use tokio::net::{TcpListener, UnixListener};

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-net-{tag}-{}-{}",
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

    #[test]
    fn mint_is_ecdsa_p256_and_fingerprints_are_sha256() {
        let id = mint().expect("mint");
        assert_eq!(id.ca_sha256.len(), 64);
        assert!(id.ca_sha256.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
        assert!(id.ca_pem.contains("BEGIN CERTIFICATE"));
        assert!(id.server_key_pem.contains("PRIVATE KEY"));
        let server = id.server_config().expect("server");
        let client = id.client_config().expect("client");
        assert!(!server.alpn_protocols.is_empty());
        assert!(!client.alpn_protocols.is_empty());
    }

    #[test]
    fn unused_roles_are_not_seedbox() {
        assert!(DaemonRole::Home.ensure_seedbox().is_err());
        assert!(DaemonRole::ReverseConnect.ensure_seedbox().is_err());
        assert!(DaemonRole::Seedbox.ensure_seedbox().is_ok());
    }

    #[tokio::test]
    async fn tcp_and_uds_share_rustls_config() {
        let root = scratch("tree");
        write_file(&root.join("a.bin"), b"abcdefghij");
        let id = mint().expect("mint");
        let server = id.server_config().expect("server cfg");
        let client = id.client_config().expect("client cfg");

        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind tcp");
        let addr = tcp.local_addr().expect("addr");
        let tcp_seed = seedbox_for(root.clone());
        let tcp_server = server.clone();
        let tcp_task = tokio::spawn(async move { serve_tcp(tcp, tcp_server, tcp_seed).await });

        let sock = scratch("uds").join("mediaops.sock");
        let unix = UnixListener::bind(&sock).expect("bind uds");
        let uds_seed = seedbox_for(root.clone());
        let uds_task = tokio::spawn(async move { serve_unix(unix, server, uds_seed).await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let tcp_ch = connect_tcp(addr, client.clone()).await.expect("tcp connect");
        let mut tcp_control = ControlClient::new(tcp_ch.clone());
        let df = tcp_control
            .df(mediaops_proto::DfRequest {})
            .await
            .expect("df")
            .into_inner();
        assert_eq!(df.proto_package, PROTO_PACKAGE);
        assert_eq!(df.semver, "0.1.0");
        assert!(df.free_bytes > 0);

        let mut tcp_transfer = TransferClient::new(tcp_ch);
        let list = tcp_transfer
            .list(ListRequest {})
            .await
            .expect("list")
            .into_inner();
        assert_eq!(list.entries.len(), 1);
        let mut stream = tcp_transfer
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

        let uds_ch = connect_unix(&sock, client).await.expect("uds connect");
        let uds_control = mediaops_proto::ControlPortClient::new(ControlClient::new(uds_ch));
        let bytes = uds_control.df().await.expect("uds df");
        assert!(bytes.get() > 0);

        tcp_task.abort();
        uds_task.abort();
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(&sock);
    }

    #[tokio::test]
    async fn pool_is_n_independent_channels() {
        let root = scratch("pool");
        write_file(&root.join("a.bin"), b"x");
        let id = mint().expect("mint");
        let server = id.server_config().expect("server");
        let seed = seedbox_for(root.clone());
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("rebind");
        let addr = tcp.local_addr().expect("addr");
        let task = tokio::spawn(async move { serve_tcp(tcp, server, seed).await });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let client = id.client_config().expect("client");
        let pool = connect_pool(addr, client, 3).await.expect("pool");
        assert_eq!(pool.len(), 3);
        let a = pool.try_checkout().expect("a");
        let b = pool.try_checkout().expect("b");
        let c = pool.try_checkout().expect("c");
        assert!(pool.try_checkout().is_err());
        drop((a, b, c));
        assert!(pool.try_checkout().is_ok());
        task.abort();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn grabber_none_is_a_noop_apply() {
        let root = scratch("grab");
        write_file(&root.join("a.bin"), b"x");
        let id = mint().expect("mint");
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = tcp.local_addr().expect("addr");
        let task = tokio::spawn(serve_tcp(
            tcp,
            id.server_config().expect("server"),
            seedbox_for(root.clone()),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let ch = connect_tcp(addr, id.client_config().expect("client"))
            .await
            .expect("connect");
        let mut control = ControlClient::new(ch);
        control
            .grab_apply(mediaops_proto::GrabApplyRequest {})
            .await
            .expect("noop");
        task.abort();
        let _ = std::fs::remove_dir_all(root);
    }
}
