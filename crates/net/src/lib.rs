//! rustls identity, channel pool, UDS/TCP serve (AD-12, AD-14).

mod gateway;
mod listen;
mod mint;
mod pool;
mod seedbox;

pub use gateway::HomeGateway;
pub use listen::{
    connect_tcp, connect_unix, serve_home_unix, serve_tcp, serve_unix, tcp_connect_count,
};
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
    #[error("role `{0}` is not the seedbox role")]
    NotSeedbox(String),
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
            other => Err(NetError::NotSeedbox(other.to_string())),
        }
    }

    pub fn ensure_seedbox(self) -> Result<(), NetError> {
        match self {
            Self::Seedbox => Ok(()),
            Self::Home => Err(NetError::NotSeedbox("home".into())),
            Self::ReverseConnect => Err(NetError::UnusedRole("reverse-connect".into())),
        }
    }
}

#[cfg(test)]
pub(crate) fn serial_net() -> std::sync::MutexGuard<'static, ()> {
    static NET_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());
    NET_TEST.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn check_pool_n(n: usize) -> Result<(), NetError> {
    if n == 0 || n > 64 {
        Err(NetError::Pool(format!(
            "channel pool N must be 1..=64, got {n}"
        )))
    } else {
        Ok(())
    }
}

pub async fn connect_pool(
    addr: SocketAddr,
    client: Arc<ClientConfig>,
    n: usize,
) -> Result<ChannelPool, NetError> {
    check_pool_n(n)?;
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

    check_pool_n(max_n as usize)?;

    let mut samples = Vec::new();
    for n in 1..=max_n {
        let pool = connect_pool(addr, client.clone(), n as usize).await?;
        let mut guards = Vec::with_capacity(n as usize);
        for _ in 0..n {
            guards.push(pool.try_checkout()?);
        }
        let mut clients: Vec<TransferClient<Channel>> = guards
            .iter()
            .map(|guard| TransferClient::new(guard.channel().clone()))
            .collect();
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
        drop(guards);
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
    use mediaops_proto::{DfRequest, GetRangeRequest, ListRequest, PROTO_PACKAGE, StatRequest};
    use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256};
    use rustls::pki_types::{CertificateDer, pem::PemObject};
    use rustls::{ClientConfig, RootCertStore};
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use std::net::SocketAddr;
    use std::path::Path;
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

    fn sha256_hex(der: &[u8]) -> String {
        Sha256::digest(der)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn cert_der(pem: &str) -> CertificateDer<'static> {
        CertificateDer::from_pem_slice(pem.as_bytes()).expect("cert pem")
    }

    fn client_trusts_ca_no_auth(ca_pem: &str) -> Arc<ClientConfig> {
        crate::mint::ensure_crypto_provider();
        let mut roots = RootCertStore::empty();
        roots.add(cert_der(ca_pem)).expect("root");
        let mut config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec()];
        Arc::new(config)
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

    async fn rpc_must_fail(addr: SocketAddr, client: Arc<ClientConfig>) {
        match connect_tcp(addr, client).await {
            Err(_) => {}
            Ok(channel) => {
                let mut control = ControlClient::new(channel);
                let result = control.df(DfRequest {}).await;
                assert!(result.is_err(), "Control/Transfer RPC must fail");
            }
        }
    }

    #[test]
    fn mint_is_ecdsa_p256_and_fingerprints_are_sha256() {
        let id = mint().expect("mint");
        let ca_der = cert_der(&id.ca_pem);
        let server_der = cert_der(&id.server_cert_pem);
        let client_der = cert_der(&id.client_cert_pem);
        assert_eq!(sha256_hex(ca_der.as_ref()), id.ca_sha256);
        assert_eq!(sha256_hex(server_der.as_ref()), id.server_sha256);
        assert_eq!(sha256_hex(client_der.as_ref()), id.client_sha256);
        assert_eq!(id.ca_sha256.len(), 64);
        assert!(
            id.ca_sha256
                .chars()
                .all(|c| matches!(c, '0'..='9' | 'a'..='f'))
        );
        assert!(id.ca_pem.contains("BEGIN CERTIFICATE"));
        assert!(id.server_key_pem.contains("PRIVATE KEY"));
        let server_key = KeyPair::from_pem(&id.server_key_pem).expect("server key");
        let client_key = KeyPair::from_pem(&id.client_key_pem).expect("client key");
        assert!(server_key.is_compatible(&PKCS_ECDSA_P256_SHA256));
        assert!(client_key.is_compatible(&PKCS_ECDSA_P256_SHA256));
        let server = id.server_config().expect("server");
        let client = id.client_config().expect("client");
        assert!(!server.alpn_protocols.is_empty());
        assert!(!client.alpn_protocols.is_empty());
    }

    #[test]
    fn unused_roles_are_not_seedbox() {
        assert!(matches!(
            DaemonRole::Home.ensure_seedbox(),
            Err(NetError::NotSeedbox(role)) if role == "home"
        ));
        assert!(!matches!(
            DaemonRole::Home.ensure_seedbox(),
            Err(NetError::UnusedRole(_))
        ));
        assert!(matches!(
            DaemonRole::ReverseConnect.ensure_seedbox(),
            Err(NetError::UnusedRole(role)) if role == "reverse-connect"
        ));
        assert!(DaemonRole::Seedbox.ensure_seedbox().is_ok());
        assert!(matches!(
            DaemonRole::parse("laptop"),
            Err(NetError::NotSeedbox(_))
        ));
        assert!(!matches!(
            DaemonRole::parse("laptop"),
            Err(NetError::Pool(_))
        ));
    }

    #[tokio::test]
    async fn tcp_and_uds_share_rustls_config() {
        let _serial = serial_net();
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

        let tcp_ch = wait_tcp(addr, client.clone()).await;
        let mut tcp_control = ControlClient::new(tcp_ch.clone());
        let df = tcp_control.df(DfRequest {}).await.expect("df").into_inner();
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

        let uds_ch = wait_unix(&sock, client).await;
        let uds_control = mediaops_proto::ControlPortClient::new(ControlClient::new(uds_ch));
        let snapshot = uds_control.df().await.expect("uds df");
        assert!(snapshot.free.get() > 0);

        tcp_task.abort();
        uds_task.abort();
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(&sock);
    }

    #[tokio::test]
    async fn pool_is_n_independent_channels() {
        let _serial = serial_net();
        let root = scratch("pool");
        write_file(&root.join("a.bin"), b"abcdefghij");
        let id = mint().expect("mint");
        let server = id.server_config().expect("server");
        let seed = seedbox_for(root.clone());
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("rebind");
        let addr = tcp.local_addr().expect("addr");
        let task = tokio::spawn(async move { serve_tcp(tcp, server, seed).await });
        let client = id.client_config().expect("client");
        let _ = wait_tcp(addr, client.clone()).await;
        let pool = connect_pool(addr, client, 3).await.expect("pool");
        assert_eq!(pool.len(), 3);
        let a = pool.try_checkout().expect("a");
        let b = pool.try_checkout().expect("b");
        let c = pool.try_checkout().expect("c");
        assert!(matches!(pool.try_checkout(), Err(NetError::Exhausted)));
        let mut listing = TransferClient::new(a.channel().clone())
            .list(ListRequest {})
            .await
            .expect("list")
            .into_inner();
        let r#ref = listing.entries.pop().expect("entry").r#ref;
        let mut joins = Vec::new();
        for guard in [a, b, c] {
            let r#ref = r#ref.clone();
            joins.push(tokio::spawn(async move {
                let mut client = TransferClient::new(guard.channel().clone());
                let mut stream = client
                    .get_range(GetRangeRequest {
                        r#ref,
                        offset: 0,
                        len: 4,
                    })
                    .await
                    .expect("range")
                    .into_inner();
                let mut body = Vec::new();
                while let Some(chunk) = stream.message().await.expect("msg") {
                    body.extend_from_slice(&chunk.data);
                }
                drop(guard);
                body
            }));
        }
        for join in joins {
            let body = join.await.expect("join");
            assert_eq!(body, b"abcd");
        }
        assert!(pool.try_checkout().is_ok());
        task.abort();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn grabber_none_is_a_noop_apply() {
        let _serial = serial_net();
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
        let ch = wait_tcp(addr, id.client_config().expect("client")).await;
        let mut control = ControlClient::new(ch);
        control
            .grab_apply(mediaops_proto::GrabApplyRequest::default())
            .await
            .expect("noop");
        task.abort();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn negative_mtls_rejects_unauthenticated_and_foreign_clients() {
        let _serial = serial_net();
        let root = scratch("mtls");
        write_file(&root.join("a.bin"), b"x");
        let id = mint().expect("mint");
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = tcp.local_addr().expect("addr");
        let task = tokio::spawn(serve_tcp(
            tcp,
            id.server_config().expect("server"),
            seedbox_for(root.clone()),
        ));
        let matching = id.client_config().expect("matching");
        let ch = wait_tcp(addr, matching.clone()).await;
        ControlClient::new(ch)
            .df(DfRequest {})
            .await
            .expect("matching client");
        rpc_must_fail(addr, client_trusts_ca_no_auth(&id.ca_pem)).await;
        let other = mint().expect("other mint");
        rpc_must_fail(addr, other.client_config().expect("foreign client")).await;
        task.abort();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn write_to_dir_from_dir_and_server_only_dir_serve() {
        let _serial = serial_net();
        let root = scratch("tls-rt");
        write_file(&root.join("a.bin"), b"x");
        let id = mint().expect("mint");
        let tls_dir = scratch("tls-full");
        id.write_to_dir(&tls_dir).expect("write");
        for name in [
            "ca.pem",
            "server.pem",
            "server.key",
            "client.pem",
            "client.key",
        ] {
            assert!(tls_dir.join(name).is_file(), "{name}");
        }
        let loaded = IdentityBundle::from_dir(&tls_dir).expect("from_dir");
        let server = loaded.server_config().expect("server");
        let client = loaded.client_config().expect("client");
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = tcp.local_addr().expect("addr");
        let task = tokio::spawn(serve_tcp(tcp, server, seedbox_for(root.clone())));
        let ch = wait_tcp(addr, client).await;
        ControlClient::new(ch)
            .df(DfRequest {})
            .await
            .expect("loaded handshake");
        task.abort();

        let server_only = scratch("tls-server");
        for name in ["ca.pem", "server.pem", "server.key"] {
            std::fs::copy(tls_dir.join(name), server_only.join(name)).expect("copy");
        }
        assert!(!server_only.join("client.pem").exists());
        assert!(!server_only.join("client.key").exists());
        let loaded_server = IdentityBundle::from_dir(&server_only).expect("from_dir server-only");
        assert!(loaded_server.client_config().is_err());
        let server = loaded_server.server_config().expect("server-only config");
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind2");
        let addr = tcp.local_addr().expect("addr2");
        let task = tokio::spawn(serve_tcp(tcp, server, seedbox_for(root.clone())));
        let ch = wait_tcp(addr, id.client_config().expect("minted client")).await;
        ControlClient::new(ch)
            .df(DfRequest {})
            .await
            .expect("server-only serve");
        task.abort();
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(tls_dir);
        let _ = std::fs::remove_dir_all(server_only);
    }

    #[tokio::test]
    async fn probe_range_n_against_in_process_server() {
        let _serial = serial_net();
        let root = scratch("probe");
        write_file(&root.join("a.bin"), b"abcdefghij");
        let id = mint().expect("mint");
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = tcp.local_addr().expect("addr");
        let task = tokio::spawn(serve_tcp(
            tcp,
            id.server_config().expect("server"),
            seedbox_for(root.clone()),
        ));
        let client = id.client_config().expect("client");
        let _ = wait_tcp(addr, client.clone()).await;
        let n = probe_range_n(addr, client, 2).await.expect("probe");
        assert!(n >= 1);
        task.abort();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn stat_round_trip_matches_list() {
        let _serial = serial_net();
        let root = scratch("stat");
        write_file(&root.join("a.bin"), b"abcdefghij");
        let id = mint().expect("mint");
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = tcp.local_addr().expect("addr");
        let task = tokio::spawn(serve_tcp(
            tcp,
            id.server_config().expect("server"),
            seedbox_for(root.clone()),
        ));
        let ch = wait_tcp(addr, id.client_config().expect("client")).await;
        let mut transfer = TransferClient::new(ch);
        let list = transfer
            .list(ListRequest {})
            .await
            .expect("list")
            .into_inner();
        assert_eq!(list.entries.len(), 1);
        let listed = &list.entries[0];
        let st = transfer
            .stat(StatRequest {
                r#ref: listed.r#ref.clone(),
            })
            .await
            .expect("stat")
            .into_inner();
        let entry = st.entry.expect("entry");
        assert_eq!(entry.r#ref, listed.r#ref);
        assert_eq!(entry.len, listed.len);
        task.abort();
        let _ = std::fs::remove_dir_all(root);
    }
}
