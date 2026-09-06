//! Process-level harness: `mediaops-home` execs the five roles, one Pull installs.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use mediaops_core::{Blake3Hex, TitleId, TitleKind, parse_remote, staging_path};

#[path = "../src/test_support.rs"]
mod test_support;

fn workspace_bin(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mediaops")).with_file_name(name)
}

fn require_bins() {
    for name in [
        "mediaops-home",
        "mediaops-api",
        "mediaops-scheduler",
        "mediaops-gateway",
        "mediaops-inventory",
        "mediaops-pull",
    ] {
        let path = workspace_bin(name);
        assert!(
            path.is_file(),
            "missing {}; make test / CI builds the workspace first",
            path.display()
        );
    }
}

fn wait_socket(path: &Path, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !path.exists() {
        if Instant::now() >= deadline {
            panic!("socket {} did not appear", path.display());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mediaops"))
}

fn assert_installed_file_proof_and_staging_gone(api_sock: &Path, job: &serde_json::Value) {
    let library_root = Path::new(job["spec"]["libraryRoot"].as_str().expect("libraryRoot"));
    let dest_rel = job["spec"]["destRel"].as_str().expect("destRel");
    let title_id = job["spec"]["titleId"].as_str().expect("titleId");
    let verified = job["status"]["verifiedB3"].as_str().expect("verifiedB3");

    let dest = library_root.join(dest_rel);
    let digest =
        Blake3Hex::of_reader(std::fs::File::open(&dest).expect("open dest")).expect("hash dest");
    assert_eq!(
        digest.as_str(),
        verified,
        "dest digest must match Job.verifiedB3 for {dest_rel}"
    );

    let parsed = TitleId::parse(title_id).expect("parse titleId");
    let final_name = Path::new(dest_rel)
        .file_name()
        .expect("final_name")
        .to_str()
        .expect("utf8 name");
    let staged = library_root.join(staging_path(&parsed, final_name).expect("staging_path"));
    let mut sidecar = staged.clone();
    sidecar.as_mut_os_string().push(".partial.b3");
    assert!(
        std::fs::symlink_metadata(&staged).is_err(),
        "owned staged source must be gone: {}",
        staged.display()
    );
    assert!(
        std::fs::symlink_metadata(&sidecar).is_err(),
        "owned .partial.b3 must be gone: {}",
        sidecar.display()
    );

    let out = cli()
        .args(["get", "Title", title_id, "-o", "json", "--socket"])
        .arg(api_sock)
        .output()
        .expect("get Title");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let title: serde_json::Value = serde_json::from_slice(&out.stdout).expect("title json");
    assert!(
        title["status"]["files"].as_array().is_some_and(|files| {
            files
                .iter()
                .any(|file| file["path"] == dest_rel && file["installB3"] == verified)
        }),
        "installed Job must have Title observed file proof: destRel={dest_rel} verifiedB3={verified} title={title}"
    );
}

struct Supervisor(Child);

impl Drop for Supervisor {
    fn drop(&mut self) {
        let pid = self.0.id().to_string();
        let _ = Command::new("kill").args(["-TERM", &pid]).status();
        let _ = self.0.wait();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn supervisor_installs_each_file_and_restarts_with_expired_secret_history() {
    require_bins();
    let _serial = test_support::serial_net();
    let lb = test_support::start_pair(Some(test_support::MOVIE_REL), &[7u8; 64]).await;
    let extra = [
        "music/Yes/Relayer.(1974)/Relayer.(1974).01.The.Gates.Of.Delirium.flac",
        "music/Yes/Relayer.(1974)/Relayer.(1974).02.Sound.Chaser.flac",
        "series/The.Wire.(2002)/Season.01/The.Wire.(2002).S01E01.The.Target.mkv",
        "series/The.Wire.(2002)/Season.01/The.Wire.(2002).S01E02.The.Detail.mkv",
    ];
    for rel in extra {
        let path = lb.remote_root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, [7u8; 64]).expect("remote file");
    }
    let dir = test_support::scratch("home-supervisor");
    let library = test_support::library_root(&dir);
    let api_sock = dir.join("api.sock");
    let gw_sock = dir.join("gw.sock");
    let api_db = dir.join("api.db");

    let child = Command::new(workspace_bin("mediaops-home"))
        .arg("--socket")
        .arg(&api_sock)
        .arg("--api-db")
        .arg(&api_db)
        .arg("--gateway-socket")
        .arg(&gw_sock)
        .arg("--tls-dir")
        .arg(&lb.tls_dir)
        .spawn()
        .expect("mediaops-home");
    let supervisor = Supervisor(child);
    wait_socket(&api_sock, 15);

    let title_id = parse_remote(Some(TitleKind::Movie), Path::new(test_support::MOVIE_REL))
        .expect("classify")
        .0
        .render();

    let cluster = dir.join("cluster.toml");
    std::fs::write(
        &cluster,
        format!(
            "kind = \"Cluster\"\n[metadata]\nname = \"home\"\n[spec]\nmax_copy = 1073741824\nmin_free = 0\nrange_len = 64\ngrabber = \"none\"\nlibrary_root = \"{}\"\n[[spec.roots]]\nid = \"seedbox\"\npath = \"/data\"\n",
            library.display()
        ),
    )
    .expect("cluster");
    let secret = dir.join("secret.toml");
    std::fs::write(
        &secret,
        format!(
            "kind = \"Secret\"\n[metadata]\nname = \"seedbox\"\n[spec]\nseedbox_address = \"{}\"\n",
            lb.tcp_addr
        ),
    )
    .expect("secret");
    let want = dir.join("want.toml");
    std::fs::write(
        &want,
        format!(
            "kind = \"Want\"\n[metadata]\nname = \"{title_id}\"\n[spec]\ntitle_id = \"{title_id}\"\n"
        ),
    )
    .expect("want");

    for file in [&cluster, &secret, &want] {
        let out = cli()
            .args(["apply", "-f"])
            .arg(file)
            .arg("--socket")
            .arg(&api_sock)
            .output()
            .expect("apply");
        assert_eq!(
            out.status.code(),
            Some(0),
            "apply {} stderr={}",
            file.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    for id in ["album:key:yes.relayer", "series:key:thewire.2002"] {
        let out = cli()
            .args(["watch", id, "--socket"])
            .arg(&api_sock)
            .output()
            .expect("watch");
        assert!(
            out.status.success(),
            "watch failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let deadline = Instant::now() + Duration::from_secs(40);
    let mut installed = false;
    let mut installed_jobs = Vec::new();
    while Instant::now() < deadline {
        let out = cli()
            .args(["get", "Job", "-o", "json", "--socket"])
            .arg(&api_sock)
            .output()
            .expect("get Job");
        if out.status.code() == Some(0) {
            let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("jobs json");
            let jobs = value["items"].as_array().expect("items");
            if jobs.len() == 5 && jobs.iter().all(|job| job["status"]["phase"] == "installed") {
                installed = true;
                installed_jobs = jobs.clone();
                let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
                let obj = if value.get("kind").is_some() {
                    value
                } else {
                    value["items"][0].clone()
                };
                assert_eq!(obj["apiVersion"], "mediaops.home.v1");
                assert!(obj.get("ok").is_none(), "raw object: {obj}");
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(installed, "all five same-size files must reach installed");
    for rel in std::iter::once(test_support::MOVIE_REL).chain(extra) {
        assert_eq!(
            std::fs::read(library.join(rel)).expect("installed bytes"),
            [7u8; 64]
        );
    }

    let title = cli()
        .args(["get", "Title", &title_id, "-o", "json", "--socket"])
        .arg(&api_sock)
        .output()
        .expect("get Title");
    assert_eq!(
        title.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&title.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&title.stdout).expect("title json");
    assert_eq!(value["kind"], "Title");
    assert!(
        value["status"]["files"]
            .as_array()
            .is_some_and(|files| files.len() == 1),
        "Title observed path: {value}"
    );
    for job in &installed_jobs {
        assert_installed_file_proof_and_staging_gone(&api_sock, job);
    }

    // A show remains desired after its initial listing is satisfied. Restart
    // the complete control plane before a new episode arrives.
    drop(supervisor);
    // An unchanged Secret may predate retained history (also true immediately
    // after migrating an older api.db). A new gateway needs a fresh snapshot,
    // not a replay cursor derived from that object's last update.
    let store = mediaops_store::ApiStore::open(&api_db)
        .await
        .expect("offline store");
    let secret = store
        .get(mediaops_core::Kind::Secret, mediaops_core::SECRET_NAME)
        .await
        .expect("get Secret")
        .expect("Secret exists");
    let mut cluster = store
        .get(mediaops_core::Kind::Cluster, mediaops_core::CLUSTER_NAME)
        .await
        .expect("get Cluster")
        .expect("Cluster exists");
    for _ in 0..4097 {
        cluster = store.apply(cluster).await.expect("advance history").0;
    }
    assert!(
        matches!(
            store.events_after(secret.metadata.resource_version).await,
            Err(mediaops_store::StoreError::Home(
                mediaops_core::HomeError::Expired { .. }
            ))
        ),
        "the unchanged Secret must predate retained history"
    );
    drop(store);
    let later = "series/The.Wire.(2002)/Season.01/The.Wire.(2002).S01E03.The.Buys.mkv";
    std::fs::write(lb.remote_root.join(later), [9u8; 64]).expect("later episode");
    let child = Command::new(workspace_bin("mediaops-home"))
        .arg("--socket")
        .arg(&api_sock)
        .arg("--api-db")
        .arg(&api_db)
        .arg("--gateway-socket")
        .arg(&gw_sock)
        .arg("--tls-dir")
        .arg(&lb.tls_dir)
        .spawn()
        .expect("restart supervisor");
    let supervisor = Supervisor(child);
    let deadline = Instant::now() + Duration::from_secs(35);
    while !library.join(later).is_file() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        std::fs::read(library.join(later)).expect("new episode after restart"),
        [9u8; 64]
    );
    for (id, count) in [("album:key:yes.relayer", 2), ("series:key:thewire.2002", 3)] {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let out = cli()
                .args(["get", "Title", id, "-o", "json", "--socket"])
                .arg(&api_sock)
                .output()
                .expect("get proof");
            let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("proof json");
            if value["status"]["files"]
                .as_array()
                .is_some_and(|files| files.len() == count)
            {
                break;
            }
            assert!(Instant::now() < deadline, "per-file proof missing: {value}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let later_jobs = loop {
        let out = cli()
            .args(["get", "Job", "-o", "json", "--socket"])
            .arg(&api_sock)
            .output()
            .expect("get Job");
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("jobs json");
        let jobs = value["items"].as_array().expect("items").clone();
        if jobs.len() == 6 && jobs.iter().all(|job| job["status"]["phase"] == "installed") {
            break jobs;
        }
        assert!(
            Instant::now() < deadline,
            "later episode Job not installed: {value}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    for job in &later_jobs {
        assert_installed_file_proof_and_staging_gone(&api_sock, job);
    }
    drop(supervisor);
    let _ = std::fs::remove_dir_all(dir);
}
