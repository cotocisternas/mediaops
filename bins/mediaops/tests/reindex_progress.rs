//! Human reindex feedback, raw output, and proof preservation through the Home API.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use mediaops_apiserver::{ApiConfig, serve_api};
use mediaops_core::{Actor, CLUSTER_NAME, ClusterSpec, HomeObject, Kind, Spec, StatusBody};
use mediaops_home_client::HomeApi;

struct Fixture {
    dir: PathBuf,
    api: HomeApi,
    server: tokio::task::JoinHandle<Result<(), mediaops_apiserver::ApiError>>,
}

impl Fixture {
    async fn start() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-progress-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let socket = dir.join("runtime/mediaops-api.sock");
        let library = dir.join("library");
        std::fs::create_dir_all(&library).unwrap();
        let server = tokio::spawn(serve_api(ApiConfig {
            socket: socket.clone(),
            api_db: dir.join("state/mediaops/api.db"),
        }));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let api = loop {
            if let Ok(api) = HomeApi::connect(&socket, Actor::Import).await {
                break api;
            }
            assert!(tokio::time::Instant::now() < deadline, "API startup");
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        api.apply(HomeObject::new(
            Kind::Cluster,
            CLUSTER_NAME,
            Spec::Cluster(ClusterSpec {
                library_root: library.display().to_string(),
                ..ClusterSpec::default()
            }),
            StatusBody::empty(Kind::Cluster),
        ))
        .await
        .unwrap();
        Self { dir, api, server }
    }

    fn cli(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mediaops"));
        command
            .env("HOME", &self.dir)
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("XDG_RUNTIME_DIR", self.dir.join("runtime"))
            .env("TERM", "dumb")
            .args(["library", "reindex"]);
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reindex_reports_human_result_raw_json_and_never_success_on_drift() {
    let fixture = Fixture::start().await;
    let file = fixture
        .dir
        .join("library/movies/The.Matrix.(1999)/The.Matrix.(1999).mkv");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, vec![3_u8; 200_000]).unwrap();
    let output = fixture.cli().output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("indexed   1 file\nlibrary   "),
        "{stdout}"
    );
    assert!(stdout.contains("\nelapsed   "), "{stdout}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("reindex  starting"), "{stderr}");
    assert!(stderr.contains("reindex  done"), "{stderr}");
    assert!(!stderr.contains(['\r', '\u{1b}']), "{stderr}");
    let output = fixture.cli().args(["-o", "json"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!({"indexed": 1})
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("reindex  "));
    std::fs::write(&file, b"changed").unwrap();
    let output = fixture.cli().output().unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("reindex  stopped"), "{stderr}");
    assert!(!stderr.contains("reindex  done"), "{stderr}");
    assert!(stderr.contains("Cluster remains locked"), "{stderr}");
    let cluster = fixture.api.get(Kind::Cluster, CLUSTER_NAME).await.unwrap();
    assert!(matches!(cluster.spec, Spec::Cluster(spec) if spec.lock));
}

#[tokio::test(flavor = "multi_thread")]
async fn progress_is_visible_while_home_is_unresponsive() {
    use std::io::{BufRead, BufReader};
    let fixture = Fixture::start().await;
    fixture.server.abort();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let socket = fixture.dir.join("runtime/mediaops-api.sock");
    let _ = std::fs::remove_file(&socket);
    // Accept no HTTP/2 traffic: the CLI has work pending, not a successful result.
    let _listener = std::os::unix::net::UnixListener::bind(socket).unwrap();
    let mut child = fixture
        .cli()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            if line.contains("reindex  connecting to Home API") {
                let _ = tx.send(line);
                break;
            }
        }
    });
    let first = rx.recv_timeout(Duration::from_secs(10));
    let still_running = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    reader.join().unwrap();
    let first = first.unwrap();
    assert!(
        first.contains("reindex  connecting to Home API"),
        "periodic stderr line: {first:?}"
    );
    assert!(
        first.contains("elapsed ") && !first.contains("elapsed 0s"),
        "{first}"
    );
    assert!(
        still_running,
        "feedback must arrive before the command finishes"
    );
}
