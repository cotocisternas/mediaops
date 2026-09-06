use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use mediaops_core::{
    Actor, Blake3Hex, Bytes, ClusterSpec, HomeObject, JobPhase, Kind, PathRoot, SecretSpec, Spec,
    StatusBody, SyncDisposition,
};
use mediaops_home_client::HomeApi;

#[allow(dead_code)]
#[path = "../src/test_support.rs"]
mod test_support;

struct Supervisor(Child);
impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .status();
        let _ = self.0.wait();
    }
}

async fn connect(socket: &Path) -> HomeApi {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(api) = HomeApi::connect(socket, Actor::Cli).await {
                break api;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("API startup")
}

fn sync_cli(socket: &Path, dry: bool) -> HomeObject {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mediaops"));
    cmd.args(["sync", "--socket"])
        .arg(socket)
        .args(["-o", "json"]);
    if dry {
        cmd.arg("--dry-run");
    }
    let output = cmd.output().expect("sync CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = serde_json::from_slice(&output.stdout).expect("raw JSON");
    HomeObject::from_document(&value).expect("Sync response")
}

#[tokio::test(flavor = "multi_thread")]
async fn one_shot_cli_installs_without_wants_and_does_not_follow_new_files() {
    let _serial = test_support::serial_net();
    let payload = [7u8; 64];
    let lb = test_support::start_pair(Some(test_support::MOVIE_REL), &payload).await;
    let dir = test_support::scratch("sync-supervisor");
    let root = test_support::library_root(&dir);
    let socket = dir.join("api.sock");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_mediaops")).with_file_name("mediaops-home");
    assert!(binary.is_file(), "build workspace before sync_e2e");
    let child = Command::new(binary)
        .arg("--socket")
        .arg(&socket)
        .arg("--api-db")
        .arg(dir.join("api.db"))
        .arg("--gateway-socket")
        .arg(dir.join("gateway.sock"))
        .arg("--tls-dir")
        .arg(&lb.tls_dir)
        .spawn()
        .expect("supervisor");
    let supervisor = Supervisor(child);
    let api = connect(&socket).await;
    api.apply(HomeObject::new(
        Kind::Cluster,
        "home",
        Spec::Cluster(ClusterSpec {
            library_root: root.to_string_lossy().into_owned(),
            max_copy: Bytes::new(1024 * 1024),
            range_len: Bytes::new(64),
            roots: vec![PathRoot {
                id: "seedbox".into(),
                path: "/data".into(),
                kind: None,
            }],
            ..Default::default()
        }),
        StatusBody::empty(Kind::Cluster),
    ))
    .await
    .expect("cluster");
    api.apply(HomeObject::new(
        Kind::Secret,
        "seedbox",
        Spec::Secret(SecretSpec {
            seedbox_address: lb.tcp_addr.to_string(),
            ..Default::default()
        }),
        StatusBody::empty(Kind::Secret),
    ))
    .await
    .expect("secret");

    let preview = sync_cli(&socket, true);
    assert!(
        matches!(preview.status, StatusBody::Sync(st) if st.entries.iter().any(|e| e.disposition == SyncDisposition::WouldQueue))
    );
    assert!(
        api.list(Some(Kind::Job))
            .await
            .expect("no preview jobs")
            .is_empty()
    );
    assert!(
        api.list(Some(Kind::Sync))
            .await
            .expect("no preview request")
            .is_empty()
    );
    let request = sync_cli(&socket, false);
    let mut watch = api
        .watch_home(Some(Kind::Job), 0)
        .await
        .expect("watch Jobs");
    let job = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let event = watch.message().await.expect("event").expect("stream");
            if matches!(&event.object().status, StatusBody::Job(st) if st.phase == JobPhase::Installed) { break event.object().clone(); }
        }
    }).await.expect("installed job");
    let Spec::Job(spec) = &job.spec else {
        panic!("job spec");
    };
    assert_eq!(spec.sync_name, request.metadata.name);
    assert!(
        api.list(Some(Kind::Want))
            .await
            .expect("no persistent Wants")
            .is_empty()
    );
    assert_eq!(
        std::fs::read(root.join(&spec.dest_rel)).expect("installed bytes"),
        payload
    );
    let title = api
        .get(Kind::Title, &spec.title_id)
        .await
        .expect("Title proof");
    assert!(
        matches!(title.status, StatusBody::Title(st) if st.observed_files().iter().any(|file| file.install_b3 == Blake3Hex::of_bytes(&payload)))
    );

    let node = api.get(Kind::Node, "inventory").await.expect("inventory");
    let generation = match node.status {
        StatusBody::Node(st) => st.list_generation,
        _ => 0,
    };
    let added = lb.remote_root.join("movies/Alien.(1979)/Alien.(1979).mkv");
    std::fs::create_dir_all(added.parent().expect("parent")).expect("dirs");
    std::fs::write(added, payload).expect("new completed source");
    let mut nodes = api
        .watch_home(Some(Kind::Node), 0)
        .await
        .expect("watch inventory");
    tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            let event = nodes.message().await.expect("event").expect("stream");
            if event.object().metadata.name == "inventory" && matches!(&event.object().status, StatusBody::Node(st) if st.ready && st.list_generation > generation) { break; }
        }
    }).await.expect("new listing");
    assert_eq!(
        api.list(Some(Kind::Job)).await.expect("finite Jobs").len(),
        1
    );
    drop(supervisor);
    std::fs::remove_dir_all(dir).expect("cleanup");
}
