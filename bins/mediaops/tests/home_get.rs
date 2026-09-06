//! CLI `get -o json` prints the raw Home object (no `{ok,data,error}` envelope).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use mediaops_apiserver::{ApiConfig, serve_api};
use mediaops_core::{
    Actor, Blake3Hex, CLUSTER_NAME, ClusterSpec, ClusterStatus, HomeJobKind, HomeObject, JobSpec,
    JobStatus, Kind, Spec, StatusBody, TitleFileStatus, TitleId, TitleSpec, TitleStatus, WantSpec,
    WantStatus,
};
use mediaops_home_client::HomeApi;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mediaops-home-get-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

struct TestHome {
    dir: PathBuf,
    api: HomeApi,
    server: tokio::task::JoinHandle<Result<(), mediaops_apiserver::ApiError>>,
}

impl TestHome {
    async fn start(tag: &str) -> Self {
        let dir = scratch(tag);
        let socket = dir.join("runtime/mediaops-api.sock");
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
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        Self { dir, api, server }
    }

    fn cli(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mediaops"));
        cmd.env("HOME", &self.dir)
            .env("MEDIAOPS_CONFIG_DIR", self.dir.join("config"))
            .env("XDG_CONFIG_HOME", self.dir.join("config-home"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("XDG_RUNTIME_DIR", self.dir.join("runtime"));
        cmd
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const MATRIX_ID: &str = "movie:key:thematrix.1999";
const MATRIX_PATH: &str = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
const RELAYER_TRACK_1: &str =
    "music/Yes/Relayer.(1974)/Relayer.(1974).01.The.Gates.Of.Delirium.flac";
const RELAYER_TRACK_2: &str = "music/Yes/Relayer.(1974)/Relayer.(1974).02.Sound.Chaser.flac";

fn write_export_files(home: &TestHome) {
    std::fs::create_dir_all(home.dir.join("config/tls")).expect("config");
    std::fs::write(
        home.dir.join("config/config.toml"),
        "schema_version=1\nmax_copy_gib=1\nmin_free_gib=0\nrange_len_mib=1\nmax_nvenc=1\nlock=false\n",
    )
    .expect("config");
    std::fs::write(home.dir.join("config/tls/client.key"), b"fixture-key").expect("key");
}

async fn apply_cluster(home: &TestHome, library: &Path, lock: bool) {
    std::fs::create_dir_all(library).expect("library");
    home.api
        .apply(HomeObject::new(
            Kind::Cluster,
            CLUSTER_NAME,
            Spec::Cluster(ClusterSpec {
                library_root: library.display().to_string(),
                lock,
                ..ClusterSpec::default()
            }),
            StatusBody::Cluster(ClusterStatus::default()),
        ))
        .await
        .expect("cluster");
}

fn proof(path: &str, install: &[u8], current: &[u8]) -> TitleFileStatus {
    TitleFileStatus {
        path: path.into(),
        install_b3: Blake3Hex::of_bytes(install),
        current_b3: Blake3Hex::of_bytes(current),
        drifted: true,
    }
}

fn title_status(files: Vec<TitleFileStatus>) -> TitleStatus {
    TitleStatus {
        drifted: files.iter().any(|file| file.drifted),
        files,
        ..TitleStatus::default()
    }
}

async fn apply_title(home: &TestHome, id: &str, status: TitleStatus) {
    home.api
        .apply(HomeObject::new(
            Kind::Title,
            id,
            Spec::Title(TitleSpec {
                title_id: id.into(),
                desired_present: true,
            }),
            StatusBody::Title(status),
        ))
        .await
        .expect("title");
}

fn export_bundle(home: &TestHome) -> PathBuf {
    let bundle = home.dir.join("bundle");
    let out = home
        .cli()
        .args(["new-machine", "export", "--out"])
        .arg(&bundle)
        .output()
        .expect("export");
    assert!(
        out.status.success(),
        "export: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    bundle
}

fn import_bundle(home: &TestHome, bundle: &Path, library: &Path) -> Output {
    home.cli()
        .args(["new-machine", "import", "--from"])
        .arg(bundle)
        .arg("--library-root")
        .arg(library)
        .output()
        .expect("import")
}

fn title_files(object: &HomeObject) -> &[TitleFileStatus] {
    let StatusBody::Title(status) = &object.status else {
        panic!("title status");
    };
    &status.files
}

async fn movie_bundle(tag: &str) -> (TestHome, PathBuf) {
    let source = TestHome::start(tag).await;
    write_export_files(&source);
    apply_cluster(&source, &source.dir.join("library"), false).await;
    apply_title(
        &source,
        MATRIX_ID,
        title_status(vec![proof(MATRIX_PATH, b"original", b"encoded")]),
    )
    .await;
    let bundle = export_bundle(&source);
    (source, bundle)
}

#[tokio::test(flavor = "multi_thread")]
async fn numbered_hold_decision_ignores_archived_releases() {
    use mediaops_core::{HoldDecisionSpec, HoldSpec, HoldStatus, WorkerKind};
    let home = TestHome::start("hold-generation").await;
    let inventory = HomeApi::connect(home.dir.join("runtime/mediaops-api.sock"), Actor::Inventory)
        .await
        .expect("inventory");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    inventory
        .heartbeat(WorkerKind::Inventory, true, Some((2, now)))
        .await
        .expect("completed listing");
    for (name, generation) in [("a-old", 1), ("z-current", 2)] {
        inventory
            .apply(HomeObject::new(
                Kind::Hold,
                format!("movie:tmdb:603-{name}"),
                Spec::Hold(HoldSpec {
                    title_id: "movie:tmdb:603".into(),
                    release_id: name.into(),
                    decision: HoldDecisionSpec::Empty,
                }),
                StatusBody::Hold(HoldStatus {
                    list_generation: generation,
                    reason: "manual import".into(),
                    ..HoldStatus::default()
                }),
            ))
            .await
            .expect("hold");
    }
    let listing = home
        .cli()
        .args(["hold", "list", "-o", "json"])
        .output()
        .expect("hold list");
    assert!(
        listing.status.success(),
        "{}",
        String::from_utf8_lossy(&listing.stderr)
    );
    let listed: serde_json::Value = serde_json::from_slice(&listing.stdout).expect("holds JSON");
    assert_eq!(listed["items"].as_array().expect("items").len(), 1);
    assert_eq!(
        listed["items"][0]["metadata"]["name"],
        "movie:tmdb:603-z-current"
    );
    let decision = home
        .cli()
        .args(["hold", "reject", "1", "-o", "json"])
        .output()
        .expect("reject");
    assert!(
        decision.status.success(),
        "{}",
        String::from_utf8_lossy(&decision.stderr)
    );
    for (name, expected) in [
        ("a-old", HoldDecisionSpec::Empty),
        ("z-current", HoldDecisionSpec::Rejected),
    ] {
        let Spec::Hold(spec) = home
            .api
            .get(Kind::Hold, &format!("movie:tmdb:603-{name}"))
            .await
            .expect("decision")
            .spec
        else {
            panic!("hold spec")
        };
        assert_eq!(spec.decision, expected);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unsupported_api_version_is_refused_without_creating_an_object() {
    let home = TestHome::start("unsupported-version").await;
    let document = home.dir.join("want.json");
    std::fs::write(
        &document,
        serde_json::to_vec(&serde_json::json!({
            "apiVersion": "mediaops.home.v999", "kind": "Want",
            "metadata": {"name": "movie:tmdb:603"}, "spec": {"titleId": "movie:tmdb:603"}
        }))
        .expect("JSON"),
    )
    .expect("document");
    let output = home
        .cli()
        .args(["apply", "-f"])
        .arg(document)
        .output()
        .expect("apply");
    assert!(!output.status.success());
    assert!(
        home.api
            .list(Some(Kind::Want))
            .await
            .expect("wants")
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn home_bundle_roundtrip_preserves_runtime_settings_and_missing_file_proofs() {
    use mediaops_core::{Blake3Hex, Bytes, SecretSpec, TitleFileStatus, TitleSpec, TitleStatus};
    let source = TestHome::start("bundle-source").await;
    let destination = TestHome::start("bundle-destination").await;
    let old_root = destination.dir.join("old-library");
    std::fs::create_dir_all(&old_root).expect("destination's previous root");
    destination
        .api
        .apply(HomeObject::new(
            Kind::Cluster,
            CLUSTER_NAME,
            Spec::Cluster(ClusterSpec {
                library_root: old_root.display().to_string(),
                ..ClusterSpec::default()
            }),
            StatusBody::Cluster(ClusterStatus::default()),
        ))
        .await
        .expect("destination cluster");
    let library = source.dir.join("library");
    let rel = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
    std::fs::create_dir_all(library.join(rel).parent().expect("parent")).expect("library");
    std::fs::write(library.join(rel), b"encoded").expect("media");
    std::fs::create_dir_all(source.dir.join("config/tls")).expect("config");
    std::fs::write(source.dir.join("config/config.toml"),
        "schema_version=1\nmax_copy_gib=1\nmin_free_gib=0\nrange_len_mib=1\nmax_nvenc=1\nlock=false\n").expect("stale config");
    // These are inert fixture bytes, never loaded by a TLS service.
    std::fs::write(source.dir.join("config/tls/client.key"), b"fixture-key").expect("key");
    let mut cluster_spec = ClusterSpec {
        library_root: library.display().to_string(),
        max_copy: Bytes::new(3 << 30),
        encode_pause: true,
        range_concurrency: Some(3),
        ..ClusterSpec::default()
    };
    let mut cluster = source
        .api
        .apply(HomeObject::new(
            Kind::Cluster,
            CLUSTER_NAME,
            Spec::Cluster(cluster_spec.clone()),
            StatusBody::Cluster(ClusterStatus::default()),
        ))
        .await
        .expect("runtime cluster");
    cluster.status = StatusBody::Cluster(ClusterStatus {
        accepted_generation: 1,
        ..ClusterStatus::default()
    });
    let controller = HomeApi::connect(
        source.dir.join("runtime/mediaops-api.sock"),
        Actor::Controller,
    )
    .await
    .expect("controller");
    controller
        .patch(cluster, "status")
        .await
        .expect("observed cluster status");
    let secret = SecretSpec {
        seedbox_address: "127.0.0.1:54321".into(),
        ca_sha256: "a".repeat(64),
        server_sha256: "b".repeat(64),
        client_sha256: "c".repeat(64),
    };
    source
        .api
        .apply(HomeObject::new(
            Kind::Secret,
            mediaops_core::SECRET_NAME,
            Spec::Secret(secret.clone()),
            StatusBody::Secret,
        ))
        .await
        .expect("secret");
    let id = "movie:key:thematrix.1999";
    source
        .api
        .apply(HomeObject::new(
            Kind::Title,
            id,
            Spec::Title(TitleSpec {
                title_id: id.into(),
                desired_present: true,
            }),
            StatusBody::Title(TitleStatus {
                files: vec![TitleFileStatus {
                    path: rel.into(),
                    install_b3: Blake3Hex::of_bytes(b"original"),
                    current_b3: Blake3Hex::of_bytes(b"encoded"),
                    drifted: false,
                }],
                ..TitleStatus::default()
            }),
        ))
        .await
        .expect("source proof");
    let bundle = source.dir.join("bundle");
    let out = source
        .cli()
        .args(["new-machine", "export", "--out"])
        .arg(&bundle)
        .output()
        .expect("export");
    assert!(
        out.status.success(),
        "export: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(bundle.join("cluster.json").is_file());
    assert!(bundle.join("secret.json").is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in ["cluster.json", "secret.json", "tls/client.key"] {
            assert_eq!(
                std::fs::metadata(bundle.join(path))
                    .expect("bundle mode")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    let new_root = destination.dir.join("library");
    let out = destination
        .cli()
        .args(["new-machine", "import", "--from"])
        .arg(&bundle)
        .arg("--library-root")
        .arg(&new_root)
        .output()
        .expect("import");
    assert!(
        out.status.success(),
        "import: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    cluster_spec.library_root = new_root.display().to_string();
    let expected_cluster = Spec::Cluster(cluster_spec);
    assert_eq!(
        destination
            .api
            .get(Kind::Cluster, CLUSTER_NAME)
            .await
            .expect("restored cluster")
            .spec,
        expected_cluster
    );
    assert_eq!(
        destination
            .api
            .get(Kind::Secret, mediaops_core::SECRET_NAME)
            .await
            .expect("restored secret")
            .spec,
        Spec::Secret(secret)
    );
    let title = destination
        .api
        .get(Kind::Title, id)
        .await
        .expect("restored title");
    let StatusBody::Title(status) = title.status else {
        panic!("title status")
    };
    assert_eq!(status.files.len(), 1);
    assert!(
        status.files[0].drifted,
        "missing media never becomes verified local proof"
    );
    assert_eq!(status.files[0].install_b3, Blake3Hex::of_bytes(b"original"));
    assert_eq!(status.files[0].current_b3, Blake3Hex::of_bytes(b"encoded"));
    let repeat = destination
        .cli()
        .args(["new-machine", "import", "--from"])
        .arg(&bundle)
        .arg("--library-root")
        .arg(&new_root)
        .output()
        .expect("identical retry");
    assert!(
        repeat.status.success(),
        "identical same-bundle retry must resume: {}",
        String::from_utf8_lossy(&repeat.stderr)
    );
    let retried = destination
        .api
        .get(Kind::Title, id)
        .await
        .expect("title after identical retry");
    let StatusBody::Title(retried_status) = retried.status else {
        panic!("title status")
    };
    assert_eq!(retried_status.files, status.files);
    assert_eq!(
        destination
            .api
            .get(Kind::Cluster, CLUSTER_NAME)
            .await
            .expect("cluster after identical retry")
            .spec,
        expected_cluster
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn same_bundle_import_resumes_two_row_partial() {
    let source = TestHome::start("resume-two-row-src").await;
    write_export_files(&source);
    apply_cluster(&source, &source.dir.join("library"), false).await;
    let album = TitleId::album_key("Yes", "Relayer")
        .expect("album")
        .render();
    apply_title(
        &source,
        &album,
        title_status(vec![
            proof(RELAYER_TRACK_1, b"install-1", b"current-1"),
            proof(RELAYER_TRACK_2, b"install-2", b"current-2"),
        ]),
    )
    .await;
    let bundle = export_bundle(&source);

    let dest = TestHome::start("resume-two-row-dst").await;
    let library = dest.dir.join("library");
    apply_cluster(&dest, &library, true).await;
    apply_title(
        &dest,
        &album,
        title_status(vec![proof(RELAYER_TRACK_1, b"install-1", b"current-1")]),
    )
    .await;
    let out = import_bundle(&dest, &bundle, &library);
    assert!(
        out.status.success(),
        "two-row partial: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let title = dest
        .api
        .get(Kind::Title, &album)
        .await
        .expect("resumed album");
    let files = title_files(&title);
    assert_eq!(files.len(), 2);
    assert_eq!(files[0], proof(RELAYER_TRACK_1, b"install-1", b"current-1"));
    assert_eq!(files[1].path, RELAYER_TRACK_2);
    assert_eq!(files[1].install_b3, Blake3Hex::of_bytes(b"install-2"));
    let Spec::Cluster(spec) = dest
        .api
        .get(Kind::Cluster, CLUSTER_NAME)
        .await
        .expect("cluster")
        .spec
    else {
        panic!("cluster");
    };
    assert!(
        !spec.lock,
        "successful import restores the bundle scheduling lock"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn same_bundle_import_resumes_spec_without_status_placeholder() {
    let (_source, bundle) = movie_bundle("resume-placeholder-src").await;
    let dest = TestHome::start("resume-placeholder-dst").await;
    let library = dest.dir.join("library");
    apply_cluster(&dest, &library, false).await;
    apply_title(&dest, MATRIX_ID, TitleStatus::default()).await;
    let out = import_bundle(&dest, &bundle, &library);
    assert!(
        out.status.success(),
        "placeholder: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let title = dest
        .api
        .get(Kind::Title, MATRIX_ID)
        .await
        .expect("placeholder title");
    let files = title_files(&title);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, MATRIX_PATH);
    assert_eq!(files[0].install_b3, Blake3Hex::of_bytes(b"original"));
}

#[tokio::test(flavor = "multi_thread")]
async fn same_bundle_import_preserves_newer_current_b3() {
    let (_source, bundle) = movie_bundle("resume-current-src").await;
    let dest = TestHome::start("resume-current-dst").await;
    let library = dest.dir.join("library");
    apply_cluster(&dest, &library, false).await;
    let mut newer = proof(MATRIX_PATH, b"original", b"newer");
    newer.drifted = true;
    apply_title(&dest, MATRIX_ID, title_status(vec![newer.clone()])).await;
    let out = import_bundle(&dest, &bundle, &library);
    assert!(
        out.status.success(),
        "preserve current: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let title = dest
        .api
        .get(Kind::Title, MATRIX_ID)
        .await
        .expect("preserved title");
    assert_eq!(title_files(&title), std::slice::from_ref(&newer));
}

#[tokio::test(flavor = "multi_thread")]
async fn same_bundle_import_refuses_foreign_extra_and_changed_install_b3() {
    let source = TestHome::start("resume-refuse-src").await;
    write_export_files(&source);
    apply_cluster(&source, &source.dir.join("library"), false).await;
    let album = TitleId::album_key("Yes", "Relayer")
        .expect("album")
        .render();
    apply_title(
        &source,
        MATRIX_ID,
        title_status(vec![proof(MATRIX_PATH, b"original", b"encoded")]),
    )
    .await;
    apply_title(
        &source,
        &album,
        title_status(vec![proof(RELAYER_TRACK_1, b"install-1", b"current-1")]),
    )
    .await;
    let bundle = export_bundle(&source);
    let other = TitleId::movie_key("Other", 1999).expect("other").render();
    let other_path = "movies/Other.(1999)/Other.(1999).mkv";

    let dest = TestHome::start("resume-foreign-dst").await;
    let library = dest.dir.join("library");
    apply_cluster(&dest, &library, false).await;
    apply_title(
        &dest,
        &other,
        title_status(vec![proof(other_path, b"x", b"x")]),
    )
    .await;
    assert!(
        !import_bundle(&dest, &bundle, &library).status.success(),
        "foreign Title must refuse"
    );
    let foreign = dest
        .api
        .get(Kind::Title, &other)
        .await
        .expect("foreign kept");
    assert_eq!(title_files(&foreign).len(), 1);
    assert!(
        dest.api.get(Kind::Title, MATRIX_ID).await.is_err(),
        "refused import must not publish bundle Titles"
    );

    let dest = TestHome::start("resume-extra-dst").await;
    let library = dest.dir.join("library");
    apply_cluster(&dest, &library, false).await;
    apply_title(
        &dest,
        &album,
        title_status(vec![
            proof(RELAYER_TRACK_1, b"install-1", b"current-1"),
            proof(RELAYER_TRACK_2, b"extra", b"extra"),
        ]),
    )
    .await;
    assert!(
        !import_bundle(&dest, &bundle, &library).status.success(),
        "extra path must refuse"
    );
    let extra = dest.api.get(Kind::Title, &album).await.expect("extra kept");
    assert_eq!(title_files(&extra).len(), 2);

    let dest = TestHome::start("resume-install-dst").await;
    let library = dest.dir.join("library");
    apply_cluster(&dest, &library, false).await;
    apply_title(
        &dest,
        MATRIX_ID,
        title_status(vec![proof(MATRIX_PATH, b"changed", b"encoded")]),
    )
    .await;
    assert!(
        !import_bundle(&dest, &bundle, &library).status.success(),
        "changed install_b3 must refuse"
    );
    let changed = dest
        .api
        .get(Kind::Title, MATRIX_ID)
        .await
        .expect("changed kept");
    assert_eq!(
        title_files(&changed)[0].install_b3,
        Blake3Hex::of_bytes(b"changed")
    );

    let dest = TestHome::start("resume-root-dst").await;
    let library = dest.dir.join("library");
    let other_root = dest.dir.join("other-library");
    apply_cluster(&dest, &library, false).await;
    apply_title(
        &dest,
        MATRIX_ID,
        title_status(vec![proof(MATRIX_PATH, b"original", b"encoded")]),
    )
    .await;
    std::fs::create_dir_all(&other_root).expect("other root");
    assert!(
        !import_bundle(&dest, &bundle, &other_root).status.success(),
        "nonempty index must resolve to the requested library root"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn same_bundle_import_refuses_existing_job() {
    let (_source, bundle) = movie_bundle("resume-job-src").await;
    let dest = TestHome::start("resume-job-dst").await;
    let library = dest.dir.join("library");
    apply_cluster(&dest, &library, false).await;
    let controller = HomeApi::connect(
        dest.dir.join("runtime/mediaops-api.sock"),
        Actor::Controller,
    )
    .await
    .expect("controller");
    controller
        .apply(HomeObject::new(
            Kind::Job,
            "pull-resume-gate",
            Spec::Job(JobSpec {
                library_root: library.display().to_string(),
                kind: HomeJobKind::Pull,
                title_id: MATRIX_ID.into(),
                remote_root: "movies".into(),
                remote_path: "The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
                dest_rel: MATRIX_PATH.into(),
                file_len: 64,
                range_len: 16,
                range_concurrency: 1,
                worker_kind: "pull".into(),
                ..JobSpec::default()
            }),
            StatusBody::Job(JobStatus::default()),
        ))
        .await
        .expect("job");
    let out = import_bundle(&dest, &bundle, &library);
    assert!(
        !out.status.success(),
        "terminal Job must refuse: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        dest.api.get(Kind::Title, MATRIX_ID).await.is_err(),
        "Job refusal must not publish Titles"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn same_bundle_import_refusal_restores_previous_lock() {
    let (_source, bundle) = movie_bundle("resume-lock-src").await;
    let other = TitleId::movie_key("Other", 1999).expect("other").render();
    for lock in [false, true] {
        let dest = TestHome::start(&format!("resume-lock-{lock}")).await;
        let library = dest.dir.join("library");
        apply_cluster(&dest, &library, lock).await;
        apply_title(
            &dest,
            &other,
            title_status(vec![proof(
                "movies/Other.(1999)/Other.(1999).mkv",
                b"x",
                b"x",
            )]),
        )
        .await;
        assert!(
            !import_bundle(&dest, &bundle, &library).status.success(),
            "foreign Title must refuse"
        );
        let Spec::Cluster(spec) = dest
            .api
            .get(Kind::Cluster, CLUSTER_NAME)
            .await
            .expect("cluster")
            .spec
        else {
            panic!("cluster");
        };
        assert_eq!(spec.lock, lock, "refusal must restore lock={lock}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn get_json_is_raw_object() {
    let dir = scratch("api");
    let socket = dir.join("api.sock");
    let api_task = tokio::spawn(serve_api(ApiConfig {
        socket: socket.clone(),
        api_db: dir.join("api.db"),
    }));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let api = loop {
        match HomeApi::connect(&socket, Actor::Cli).await {
            Ok(api) => break api,
            Err(err) => {
                if tokio::time::Instant::now() >= deadline {
                    panic!("api: {err}");
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    };
    api.apply(HomeObject::new(
        Kind::Cluster,
        CLUSTER_NAME,
        Spec::Cluster(ClusterSpec::default()),
        StatusBody::Cluster(ClusterStatus::default()),
    ))
    .await
    .expect("cluster");
    api.apply(HomeObject::new(
        Kind::Want,
        "movie:tmdb:603",
        Spec::Want(WantSpec {
            title_id: "movie:tmdb:603".into(),
        }),
        StatusBody::Want(WantStatus::default()),
    ))
    .await
    .expect("want");

    let output = Command::new(env!("CARGO_BIN_EXE_mediaops"))
        .args(["get", "Want", "movie:tmdb:603", "-o", "json", "--socket"])
        .arg(&socket)
        .output()
        .expect("cli");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["kind"], "Want");
    assert_eq!(value["apiVersion"], "mediaops.home.v1");
    assert_eq!(value["metadata"]["name"], "movie:tmdb:603");
    assert!(
        value.get("ok").is_none(),
        "-o json must be the raw object: {value}"
    );

    api_task.abort();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_creates_want_and_json_is_raw_object() {
    let dir = scratch("watch");
    let socket = dir.join("api.sock");
    let api_task = tokio::spawn(serve_api(ApiConfig {
        socket: socket.clone(),
        api_db: dir.join("api.db"),
    }));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if HomeApi::connect(&socket, Actor::Cli).await.is_ok() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("api did not come up");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let first = Command::new(env!("CARGO_BIN_EXE_mediaops"))
        .args(["watch", "movie:tmdb:603", "-o", "json", "--socket"])
        .arg(&socket)
        .output()
        .expect("watch");
    assert_eq!(
        first.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&first.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&first.stdout).expect("json");
    assert_eq!(value["kind"], "Want");
    assert_eq!(value["apiVersion"], "mediaops.home.v1");
    assert_eq!(value["metadata"]["name"], "movie:tmdb:603");
    assert!(value.get("ok").is_none(), "raw object: {value}");

    let second = Command::new(env!("CARGO_BIN_EXE_mediaops"))
        .args(["watch", "movie:tmdb:603", "-o", "json", "--socket"])
        .arg(&socket)
        .output()
        .expect("watch2");
    assert_eq!(second.status.code(), Some(0));
    let again: serde_json::Value = serde_json::from_slice(&second.stdout).expect("json");
    assert_eq!(again["kind"], "Want");
    assert_eq!(again["metadata"]["name"], "movie:tmdb:603");

    let legacy = Command::new(env!("CARGO_BIN_EXE_mediaops"))
        .args(["watch", "movie:tmdb:603", "--json", "--socket"])
        .arg(&socket)
        .output()
        .expect("legacy json");
    assert!(legacy.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&legacy.stdout).expect("envelope");
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["data"]["kind"], "Want");
    assert!(envelope["error"].is_null());

    api_task.abort();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unavailable_api_never_records_a_want_in_legacy_state() {
    let dir = scratch("offline");
    let out = Command::new(env!("CARGO_BIN_EXE_mediaops"))
        .env("HOME", &dir)
        .env("XDG_STATE_HOME", &dir)
        .env("XDG_RUNTIME_DIR", &dir)
        .args(["watch", "movie:tmdb:603", "--json"])
        .output()
        .expect("watch");
    assert!(!out.status.success(), "API outage must fail");
    assert!(
        !dir.join("mediaops/state.db").exists(),
        "no cold-state write"
    );
    let envelope: serde_json::Value = serde_json::from_slice(&out.stdout).expect("error envelope");
    assert_eq!(envelope["ok"], false);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn named_watch_flushes_first_event_and_continues_past_32_changes() {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, BufReader};

    let dir = scratch("stream");
    let socket = dir.join("api.sock");
    let server = tokio::spawn(serve_api(ApiConfig {
        socket: socket.clone(),
        api_db: dir.join("api.db"),
    }));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let api = loop {
        if let Ok(api) = HomeApi::connect(&socket, Actor::Cli).await {
            break api;
        }
        assert!(tokio::time::Instant::now() < deadline, "API did not start");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let mut wanted = api
        .apply(HomeObject::new(
            Kind::Want,
            "movie:tmdb:603",
            Spec::Want(WantSpec {
                title_id: "movie:tmdb:603".into(),
            }),
            StatusBody::Want(WantStatus::default()),
        ))
        .await
        .expect("want");
    api.apply(HomeObject::new(
        Kind::Want,
        "movie:tmdb:604",
        Spec::Want(WantSpec {
            title_id: "movie:tmdb:604".into(),
        }),
        StatusBody::Want(WantStatus::default()),
    ))
    .await
    .expect("other want");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mediaops"))
        .args([
            "get",
            "Want",
            "movie:tmdb:603",
            "--watch",
            "-o",
            "json",
            "--socket",
        ])
        .arg(&socket)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("watch process");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    let first = tokio::time::timeout(Duration::from_secs(3), lines.next_line())
        .await
        .expect("initial event must be flushed immediately")
        .expect("read")
        .expect("event");
    let value: serde_json::Value = serde_json::from_str(&first).expect("event json");
    assert_eq!(value["metadata"]["name"], "movie:tmdb:603");
    let controller = HomeApi::connect(&socket, Actor::Controller)
        .await
        .expect("controller");
    for i in 0..35 {
        wanted.status = StatusBody::Want(WantStatus {
            phase: if i % 2 == 0 {
                mediaops_core::WantPhase::Satisfied
            } else {
                mediaops_core::WantPhase::Open
            },
        });
        wanted = controller
            .patch(wanted, "status")
            .await
            .expect("update want");
        let line = tokio::time::timeout(Duration::from_secs(3), lines.next_line())
            .await
            .expect("event delivery")
            .expect("read")
            .expect("watch must not terminate at 32");
        let event: serde_json::Value = serde_json::from_str(&line).expect("event json");
        assert_eq!(
            event["metadata"]["name"], "movie:tmdb:603",
            "named watch filters other objects"
        );
        assert_eq!(
            event["metadata"]["resourceVersion"],
            wanted.metadata.resource_version
        );
    }
    child.kill().await.expect("stop watch");
    child.wait().await.expect("reap watch");
    server.abort();
    let _ = server.await;
    let _ = std::fs::remove_dir_all(dir);
}

fn assert_one_stdout_line(out: &Output) {
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.ends_with('\n'), "one trailing newline: {stdout:?}");
    assert_eq!(
        stdout.matches('\n').count(),
        1,
        "exactly one stdout line: {stdout:?}"
    );
}

fn assert_cli_ok(out: &Output) {
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_one_stdout_line(out);
}

async fn cluster_generation(home: &TestHome) -> i64 {
    let obj = home
        .api
        .get(Kind::Cluster, CLUSTER_NAME)
        .await
        .expect("cluster");
    match obj.status {
        StatusBody::Cluster(status) => status.reconcile_generation,
        _ => panic!("cluster status"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reconcile_table_and_wide_keep_tsv() {
    let home = TestHome::start("reconcile-tsv").await;
    apply_cluster(&home, &home.dir.join("library"), false).await;
    for extra in [None, Some(&["-o", "table"][..]), Some(&["-o", "wide"][..])] {
        let mut cmd = home.cli();
        cmd.arg("reconcile");
        if let Some(flags) = extra {
            cmd.args(flags);
        }
        let out = cmd.output().expect("cli");
        assert_cli_ok(&out);
        let generation = cluster_generation(&home).await;
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim_end(),
            format!("reconcileGeneration\t{generation}")
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reconcile_json_is_raw_object() {
    let home = TestHome::start("reconcile-json").await;
    apply_cluster(&home, &home.dir.join("library"), false).await;
    let out = home
        .cli()
        .args(["reconcile", "-o", "json"])
        .output()
        .expect("cli");
    assert_cli_ok(&out);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(
        value["reconcileGeneration"],
        cluster_generation(&home).await
    );
    assert!(
        value.get("ok").is_none(),
        "-o json must be the raw object: {value}"
    );
    assert!(!String::from_utf8_lossy(&out.stderr).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn reconcile_legacy_json_is_envelope() {
    let home = TestHome::start("reconcile-legacy").await;
    apply_cluster(&home, &home.dir.join("library"), false).await;
    let out = home
        .cli()
        .args(["reconcile", "--json"])
        .output()
        .expect("cli");
    assert_cli_ok(&out);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(value["ok"], true);
    assert_eq!(
        value["data"]["reconcileGeneration"],
        cluster_generation(&home).await
    );
    assert!(value["error"].is_null());
}

#[tokio::test(flavor = "multi_thread")]
async fn reconcile_invalid_output_does_not_mutate() {
    let home = TestHome::start("reconcile-bad-o").await;
    apply_cluster(&home, &home.dir.join("library"), false).await;
    let before = cluster_generation(&home).await;
    let out = home
        .cli()
        .args(["reconcile", "-o", "yaml"])
        .output()
        .expect("cli");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).trim().is_empty());
    assert_eq!(cluster_generation(&home).await, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn reconcile_conflicting_flags_do_not_mutate() {
    let home = TestHome::start("reconcile-conflict").await;
    apply_cluster(&home, &home.dir.join("library"), false).await;
    let before = cluster_generation(&home).await;
    let out = home
        .cli()
        .args(["reconcile", "--json", "-o", "json"])
        .output()
        .expect("cli");
    assert_eq!(out.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("envelope");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "usage");
    assert_eq!(cluster_generation(&home).await, before);
}

fn import_legacy_cmd(home: &TestHome, extra: &[&str]) -> Command {
    let mut cmd = home.cli();
    cmd.arg("import-legacy").args(extra);
    cmd
}

#[tokio::test(flavor = "multi_thread")]
async fn import_legacy_table_and_wide_keep_tsv() {
    let home = TestHome::start("import-tsv").await;
    write_export_files(&home);
    let first = import_legacy_cmd(&home, &[]).output().expect("cli");
    assert_cli_ok(&first);
    assert_eq!(
        String::from_utf8_lossy(&first.stdout).trim_end(),
        "imported\t1"
    );
    for flags in [&["-o", "table"][..], &["-o", "wide"][..]] {
        let out = import_legacy_cmd(&home, flags).output().expect("cli");
        assert_cli_ok(&out);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim_end(),
            "imported\t0"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn import_legacy_json_is_raw_object() {
    let home = TestHome::start("import-json").await;
    write_export_files(&home);
    let out = import_legacy_cmd(&home, &["-o", "json"])
        .output()
        .expect("cli");
    assert_cli_ok(&out);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(value["imported"], 1);
    assert!(
        value.get("ok").is_none(),
        "-o json must be the raw object: {value}"
    );
    assert!(!String::from_utf8_lossy(&out.stderr).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn import_legacy_legacy_json_is_envelope() {
    let home = TestHome::start("import-legacy-json").await;
    write_export_files(&home);
    let out = import_legacy_cmd(&home, &["--json"]).output().expect("cli");
    assert_cli_ok(&out);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["imported"], 1);
    assert!(value["error"].is_null());
}

#[tokio::test(flavor = "multi_thread")]
async fn import_legacy_invalid_output_does_not_mutate() {
    let home = TestHome::start("import-bad-o").await;
    write_export_files(&home);
    let out = import_legacy_cmd(&home, &["-o", "yaml"])
        .output()
        .expect("cli");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).trim().is_empty());
    assert!(
        home.api.get(Kind::Cluster, CLUSTER_NAME).await.is_err(),
        "invalid -o must not import"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn import_legacy_conflicting_flags_do_not_mutate() {
    let home = TestHome::start("import-conflict").await;
    write_export_files(&home);
    let out = import_legacy_cmd(&home, &["--json", "-o", "json"])
        .output()
        .expect("cli");
    assert_eq!(out.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("envelope");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "usage");
    assert!(
        home.api.get(Kind::Cluster, CLUSTER_NAME).await.is_err(),
        "conflicting flags must not import"
    );
}
