use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mediaops_apiserver::{ApiConfig, serve_api};
use mediaops_core::Actor;
use mediaops_home_client::HomeApi;

static TEST_API_SEQ: AtomicU64 = AtomicU64::new(0);

pub struct TestApi {
    pub dir: PathBuf,
    pub socket: PathBuf,
    pub api: HomeApi,
    server: tokio::task::JoinHandle<Result<(), mediaops_apiserver::ApiError>>,
}

impl TestApi {
    pub async fn start(tag: &str) -> Self {
        let seq = TEST_API_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("mediaops-tui-{tag}-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let socket = dir.join("api.sock");
        let server = tokio::spawn(serve_api(ApiConfig {
            socket: socket.clone(),
            api_db: dir.join("api.db"),
        }));
        let api = wait_ready(&socket).await;
        Self {
            dir,
            socket,
            api,
            server,
        }
    }
}

async fn wait_ready(socket: &std::path::Path) -> HomeApi {
    let start = std::time::Instant::now();
    let deadline = Duration::from_secs(5);
    loop {
        match HomeApi::connect(socket, Actor::Cli).await {
            Ok(api) => return api,
            Err(err) if start.elapsed() < deadline => {
                drop(err);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(err) => panic!("connect after {:?}: {err}", start.elapsed()),
        }
    }
}

impl Drop for TestApi {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[allow(dead_code)]
pub async fn pump_until_current(session: &mut mediaops_tui::Session) {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        while let Some(ev) = session.try_recv() {
            session.apply_event(ev);
        }
        if session.sync == mediaops_tui::SyncState::Current {
            return;
        }
        if let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(50), session.recv()).await
        {
            session.apply_event(ev);
        }
    }
    panic!("session did not become Current ({:?})", session.sync);
}
