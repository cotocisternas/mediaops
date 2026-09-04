//! In-process seedbox + home gateway for CLI unit tests. Loopback only.

use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mediaops_core::{Allowlist, GrabOps, Grabber, Probe, UnderlayMode, endpoint_fingerprint};
use mediaops_store::Store;
use mediaops_sync::ensure_layout;
use mediaops_transfer::{
    HomeGateway, Seedbox, connect_home, connect_tcp, mint, serve_home_unix, serve_tcp,
};
use tokio::net::{TcpListener, UnixListener};

static NET_TEST: Mutex<()> = Mutex::new(());

pub fn serial_net() -> std::sync::MutexGuard<'static, ()> {
    NET_TEST.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mediaops-cli-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

pub fn write_ds(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("desired-state.toml");
    std::fs::write(&path, body).expect("ds");
    path
}

pub const DS_UNLOCKED: &str = "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 1\nmax_nvenc = 1\nlock = false\n";
pub const DS_LOCKED: &str = "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 1\nmax_nvenc = 1\nlock = true\n";
pub const DS_MAX_COPY_ZERO: &str = "schema_version = 1\nmax_copy_gib = 0\nmin_free_gib = 0\nrange_len_mib = 1\nmax_nvenc = 1\nlock = false\n";

pub const MOVIE_REL: &str = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";

pub struct Loopback {
    pub sock: PathBuf,
    pub tls_dir: PathBuf,
    pub remote_root: PathBuf,
    pub fingerprint: String,
    pub tcp_addr: SocketAddr,
    seed_task: tokio::task::JoinHandle<()>,
    uds_task: tokio::task::JoinHandle<()>,
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.seed_task.abort();
        self.uds_task.abort();
        let _ = std::fs::remove_file(&self.sock);
        let _ = std::fs::remove_dir_all(&self.tls_dir);
        let _ = std::fs::remove_dir_all(&self.remote_root);
    }
}

fn write_file(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    let mut f = std::fs::File::create(path).expect("create");
    f.write_all(bytes).expect("write");
}

pub async fn start_pair(rel: Option<&str>, body: &[u8]) -> Loopback {
    start_pair_with(rel, body, Grabber::None, None).await
}

pub async fn start_pair_with_grab_ops(
    grabber: Grabber,
    grab_ops: Option<Arc<dyn GrabOps>>,
) -> Loopback {
    start_pair_with(None, b"", grabber, grab_ops).await
}

pub async fn start_pair_with(
    rel: Option<&str>,
    body: &[u8],
    grabber: Grabber,
    grab_ops: Option<Arc<dyn GrabOps>>,
) -> Loopback {
    let remote_root = scratch("remote");
    if let Some(rel) = rel {
        write_file(&remote_root.join(rel), body);
    }
    let id = mint().expect("mint");
    let tls_dir = scratch("tls");
    id.write_to_dir(&tls_dir).expect("write tls");
    let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind tcp");
    let addr = tcp.local_addr().expect("addr");
    let mut allowlist = Allowlist::new();
    allowlist
        .add_root("seedbox", remote_root.clone())
        .expect("root");
    let nginx = scratch("nginx");
    std::fs::write(
        nginx.join("sonarr.conf"),
        "location /sonarr {\n    proxy_pass http://127.0.0.1:8989/sonarr;\n    proxy_set_header Host $host;\n}\n",
    )
    .expect("nginx");
    let seed = Seedbox::new(allowlist, "0.1.0", grabber)
        .with_nginx_dir(nginx)
        .with_grab_ops(grab_ops);
    let server = id.server_config().expect("server");
    let seed_task = tokio::spawn(async move {
        let _ = serve_tcp(tcp, server, seed).await;
    });
    let client = id.client_config().expect("client");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match connect_tcp(addr, client.clone()).await {
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
    let gateway = HomeGateway::connect(addr, client, fingerprint.clone(), 1)
        .await
        .expect("gw");
    let sock = scratch("uds").join("mediaops.sock");
    let unix = UnixListener::bind(&sock).expect("bind uds");
    let uds_server = id.server_config().expect("server");
    let uds_task = tokio::spawn(async move {
        let _ = serve_home_unix(unix, uds_server, gateway).await;
    });
    let lb = Loopback {
        sock,
        tls_dir,
        remote_root,
        fingerprint,
        tcp_addr: addr,
        seed_task,
        uds_task,
    };
    wait_ready(&lb).await;
    lb
}

pub async fn wait_ready(lb: &Loopback) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match connect_home(&lb.sock, &lb.tls_dir).await {
            Ok(_) => return,
            Err(err) => {
                if tokio::time::Instant::now() >= deadline {
                    panic!("home gateway: {err}");
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }
    }
}

pub async fn open_store(dir: &Path) -> Store {
    Store::open(dir.join("state.db")).await.expect("store")
}

pub async fn seed_probe(store: &Store, fingerprint: &str) {
    store
        .put_probe(&Probe {
            endpoint_fingerprint: fingerprint.to_string(),
            range_concurrency: 1,
        })
        .await
        .expect("probe");
}

pub fn library_root(dir: &Path) -> PathBuf {
    let root = dir.join("library");
    ensure_layout(&root).expect("layout");
    root
}
