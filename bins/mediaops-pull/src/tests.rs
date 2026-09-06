use super::*;
use mediaops_core::{HomeError, HomeJobKind, Placement, TitleSpec};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize},
};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mediaops-pull-worker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir(&path).expect("scratch");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn staging_files(spec: &JobSpec) -> (PathBuf, PathBuf, PathBuf) {
    let pull = pull_spec(spec).expect("pull");
    let staged = pull
        .library_root
        .join(staging_path(&pull.title_id, &pull.final_name).expect("stage"));
    let mut sidecar = staged.clone();
    sidecar.as_mut_os_string().push(".partial.b3");
    let lock = staged.with_extension("pull.lock");
    (staged, sidecar, lock)
}

async fn stage_verifying_job(root: &Path) -> (HomeObject, JobSpec) {
    let mut job = job(root);
    let Spec::Job(spec) = job.spec.clone() else {
        panic!("Job");
    };
    Memory::new()
        .pull(
            &pull_spec(&spec).expect("pull"),
            Arc::new(AtomicU64::new(0)),
        )
        .await
        .expect("stage");
    job.status = StatusBody::Job(JobStatus {
        phase: JobPhase::Verifying,
        attempts: 1,
        started_unix: unix_now(),
        bytes_done: spec.file_len,
        verified_b3: Some(Blake3Hex::of_bytes(&[7; 64])),
        message: String::new(),
    });
    (job, spec)
}

fn job(root: &Path) -> HomeObject {
    let title = TitleId::parse("movie:key:thematrix.1999").expect("id");
    let destination =
        mediaops_core::render(&title, &Placement::movie("The.Matrix", 1999, "mkv")).expect("path");
    let mut job = HomeObject::new(
        Kind::Job,
        "pull-fixture",
        Spec::Job(JobSpec {
            library_root: root.display().to_string(),
            kind: HomeJobKind::Pull,
            title_id: title.render(),
            remote_root: "movies".into(),
            remote_path: "The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
            dest_rel: destination.display().to_string(),
            file_len: 64,
            range_len: 16,
            range_concurrency: 2,
            max_copy: 1024,
            min_free: 0,
            node_name: "pull".into(),
            worker_kind: "pull".into(),
            ..JobSpec::default()
        }),
        StatusBody::Job(JobStatus::default()),
    );
    job.metadata.resource_version = 1;
    job.metadata.uid = "job-fixture".into();
    job
}

struct State {
    job: HomeObject,
    title: HomeObject,
    history: Vec<JobStatus>,
}

struct Api {
    state: Mutex<State>,
    fail_title_once: AtomicBool,
    fail_installed_once: AtomicBool,
}

impl Api {
    fn new(job: HomeObject) -> Self {
        let Spec::Job(spec) = &job.spec else {
            panic!("Job");
        };
        let mut title = HomeObject::new(
            Kind::Title,
            &spec.title_id,
            Spec::Title(TitleSpec {
                title_id: spec.title_id.clone(),
                desired_present: true,
            }),
            StatusBody::Title(TitleStatus::default()),
        );
        title.metadata.resource_version = 1;
        Self {
            state: Mutex::new(State {
                job,
                title,
                history: Vec::new(),
            }),
            fail_title_once: AtomicBool::new(false),
            fail_installed_once: AtomicBool::new(false),
        }
    }

    fn job(&self) -> HomeObject {
        self.state.lock().expect("state").job.clone()
    }
    fn title(&self) -> TitleStatus {
        let StatusBody::Title(status) = &self.state.lock().expect("state").title.status else {
            panic!("Title");
        };
        status.clone()
    }
}

impl JobApi for Api {
    async fn title(&self, name: &str) -> Result<HomeObject, ClientError> {
        let state = self.state.lock().expect("state");
        assert_eq!(state.title.metadata.name, name);
        Ok(state.title.clone())
    }

    async fn patch_status(&self, mut object: HomeObject) -> Result<HomeObject, ClientError> {
        let mut state = self.state.lock().expect("state");
        let current = if object.kind == Kind::Job {
            &state.job
        } else {
            &state.title
        };
        if current.metadata.resource_version != object.metadata.resource_version {
            return Err(HomeError::Conflict {
                kind: object.kind,
                name: object.metadata.name,
            }
            .into());
        }
        if object.kind == Kind::Title && self.fail_title_once.swap(false, Ordering::Relaxed) {
            return Err(ClientError::Connect(
                "injected API failure after installation".into(),
            ));
        }
        if let StatusBody::Job(status) = &object.status {
            if status.phase == JobPhase::Installed {
                if self.fail_installed_once.swap(false, Ordering::Relaxed) {
                    return Err(ClientError::Connect(
                        "injected Job completion failure".into(),
                    ));
                }
                let Spec::Job(spec) = &object.spec else {
                    panic!("Job");
                };
                let StatusBody::Title(title) = &state.title.status else {
                    panic!("Title");
                };
                assert!(
                    title.files.iter().any(|file| file.path == spec.dest_rel
                        && Some(&file.install_b3) == status.verified_b3.as_ref()),
                    "Job completed before Title proof"
                );
                assert_eq!(status.bytes_done, spec.file_len);
            }
            if status.phase == JobPhase::Verifying {
                assert!(status.verified_b3.is_some());
            }
            state.history.push(status.clone());
        }
        object.metadata.resource_version += 1;
        if object.kind == Kind::Job {
            state.job = object.clone();
        } else {
            state.title = object.clone();
        }
        Ok(object)
    }
}

struct Memory {
    calls: AtomicUsize,
}

struct Ranges;
impl RangeSource for Ranges {
    async fn get_range(
        &self,
        _remote: &RemoteRef,
        _offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, TransferError> {
        Ok(vec![7; len as usize])
    }
}

impl Memory {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

impl Transfer for Memory {
    async fn pull(&self, spec: &PullSpec, done: Arc<AtomicU64>) -> anyhow::Result<()> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        pull_file_with_progress(Arc::new(Ranges), spec, |bytes, _| {
            done.store(bytes, Ordering::Relaxed)
        })
        .await?;
        Ok(())
    }
}

struct NeverTransfer;
impl Transfer for NeverTransfer {
    async fn pull(&self, _spec: &PullSpec, _done: Arc<AtomicU64>) -> anyhow::Result<()> {
        panic!("recovery must not start another transfer");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_failures_stop_after_three_attempts_and_preserve_start_time() {
    let dir = Scratch::new();
    let api = Api::new(job(&dir.0));
    let gateway = Gateway {
        socket: dir.0.join("missing.sock"),
        tls: dir.0.join("missing-tls"),
    };
    let mut start = 0;
    for attempt in 1..=PULL_MAX_ATTEMPTS {
        run_job(&api, &gateway, api.job())
            .await
            .expect("failure recorded");
        let job = api.job();
        let status = job_status(&job);
        if start == 0 {
            start = status.started_unix;
        }
        assert!(start > 0);
        assert_eq!(status.started_unix, start);
        assert_eq!(status.attempts, attempt);
        assert_eq!(
            status.phase,
            if attempt == PULL_MAX_ATTEMPTS {
                JobPhase::Failed
            } else {
                JobPhase::Pending
            }
        );
        assert!(!status.message.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn installation_recovers_after_title_or_job_write_failure_even_after_deadline() {
    for fail_title in [true, false] {
        let dir = Scratch::new();
        let api = Api::new(job(&dir.0));
        api.fail_title_once.store(fail_title, Ordering::Relaxed);
        api.fail_installed_once
            .store(!fail_title, Ordering::Relaxed);
        let transfer = Memory::new();
        assert!(run_job(&api, &transfer, api.job()).await.is_err());
        let saved = api.job();
        assert_eq!(job_status(&saved).phase, JobPhase::Verifying);
        let Spec::Job(spec) = &saved.spec else {
            panic!("Job");
        };
        assert_eq!(
            std::fs::read(dir.0.join(&spec.dest_rel)).expect("installed"),
            vec![7; 64]
        );
        let (staged, sidecar, lock) = staging_files(spec);
        assert!(std::fs::symlink_metadata(&staged).is_err());
        assert!(std::fs::symlink_metadata(&sidecar).is_err());
        assert!(lock.is_file());
        // Emulate loading this durable state after an extended API outage.
        if let StatusBody::Job(status) = &mut api.state.lock().expect("state").job.status {
            status.started_unix = unix_now() - PULL_DEADLINE_SECS as i64 - 1;
        }
        run_job(&api, &NeverTransfer, api.job())
            .await
            .expect("resume completion");
        assert_eq!(job_status(&api.job()).phase, JobPhase::Installed);
        assert_eq!(api.title().files.len(), 1);
        assert_eq!(transfer.calls.load(Ordering::Relaxed), 1);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn verifying_restart_installs_staged_bytes_but_refuses_an_unrelated_destination() {
    for conflicting in [false, true] {
        let dir = Scratch::new();
        let mut job = job(&dir.0);
        let Spec::Job(spec) = job.spec.clone() else {
            panic!("Job");
        };
        Memory::new()
            .pull(
                &pull_spec(&spec).expect("pull"),
                Arc::new(AtomicU64::new(0)),
            )
            .await
            .expect("stage");
        job.status = StatusBody::Job(JobStatus {
            phase: JobPhase::Verifying,
            attempts: 1,
            started_unix: unix_now(),
            bytes_done: spec.file_len,
            verified_b3: Some(Blake3Hex::of_bytes(&[7; 64])),
            message: String::new(),
        });
        let destination = dir.0.join(&spec.dest_rel);
        if conflicting {
            std::fs::create_dir_all(destination.parent().expect("parent")).expect("mkdir");
            std::fs::write(&destination, [9; 64]).expect("existing library file");
        }
        let api = Api::new(job);
        run_job(&api, &NeverTransfer, api.job())
            .await
            .expect("verification outcome");
        if conflicting {
            assert_eq!(job_status(&api.job()).phase, JobPhase::Refused);
            assert_eq!(std::fs::read(destination).expect("preserved"), vec![9; 64]);
            assert!(api.title().files.is_empty());
        } else {
            assert_eq!(job_status(&api.job()).phase, JobPhase::Installed);
            assert_eq!(std::fs::read(destination).expect("installed"), vec![7; 64]);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn matching_destination_cleans_staged_hardlink() {
    let dir = Scratch::new();
    let (job, spec) = stage_verifying_job(&dir.0).await;
    let destination = dir.0.join(&spec.dest_rel);
    let (staged, sidecar, lock) = staging_files(&spec);
    std::fs::create_dir_all(destination.parent().expect("parent")).expect("mkdir");
    std::fs::hard_link(&staged, &destination).expect("hardlink dest");
    let api = Api::new(job);
    run_job(&api, &NeverTransfer, api.job())
        .await
        .expect("installed");
    assert_eq!(job_status(&api.job()).phase, JobPhase::Installed);
    assert_eq!(std::fs::read(&destination).expect("dest"), vec![7; 64]);
    assert!(std::fs::symlink_metadata(&staged).is_err());
    assert!(std::fs::symlink_metadata(&sidecar).is_err());
    assert!(lock.is_file());
    assert_eq!(api.title().files.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn matching_destination_cleans_cross_device_source() {
    let Some(other) = std::env::var_os("MEDIAOPS_TEST_INSTALL_FS") else {
        eprintln!(
            "skipping matching_destination_cleans_cross_device_source: set MEDIAOPS_TEST_INSTALL_FS to a directory on another filesystem (no root required)"
        );
        return;
    };
    let dir = Scratch::new();
    let (job, spec) = stage_verifying_job(&dir.0).await;
    let destination = dir.0.join(&spec.dest_rel);
    let parent = destination.parent().expect("parent");
    let other_parent = PathBuf::from(&other).join(format!(
        "mediaops-pull-xd-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&other_parent).expect("other dest parent");
    std::fs::create_dir_all(parent.parent().expect("kind dir")).expect("movies");
    std::os::unix::fs::symlink(&other_parent, parent).expect("cross-device dest parent");
    std::fs::write(&destination, [7; 64]).expect("dest copy");
    let dest_dev = std::fs::symlink_metadata(&destination)
        .expect("dest meta")
        .dev();
    let (staged, sidecar, lock) = staging_files(&spec);
    let staged_dev = std::fs::symlink_metadata(&staged)
        .expect("staged meta")
        .dev();
    if dest_dev == staged_dev {
        let _ = std::fs::remove_dir_all(&other_parent);
        eprintln!(
            "skipping matching_destination_cleans_cross_device_source: MEDIAOPS_TEST_INSTALL_FS is the same filesystem as the test temp dir"
        );
        return;
    }
    let api = Api::new(job);
    run_job(&api, &NeverTransfer, api.job())
        .await
        .expect("installed");
    assert_eq!(job_status(&api.job()).phase, JobPhase::Installed);
    assert_eq!(std::fs::read(&destination).expect("dest"), vec![7; 64]);
    assert!(std::fs::symlink_metadata(&staged).is_err());
    assert!(std::fs::symlink_metadata(&sidecar).is_err());
    assert!(lock.is_file());
    let _ = std::fs::remove_dir_all(&other_parent);
}

#[tokio::test(flavor = "multi_thread")]
async fn cleanup_error_keeps_verifying() {
    let dir = Scratch::new();
    let (job, spec) = stage_verifying_job(&dir.0).await;
    let destination = dir.0.join(&spec.dest_rel);
    let (staged, sidecar, lock_path) = staging_files(&spec);
    std::fs::create_dir_all(destination.parent().expect("parent")).expect("mkdir");
    std::fs::hard_link(&staged, &destination).expect("hardlink dest");
    let lock = std::fs::File::open(&lock_path).expect("lock");
    lock.try_lock().expect("hold");
    let api = Api::new(job);
    assert!(run_job(&api, &NeverTransfer, api.job()).await.is_err());
    assert_eq!(job_status(&api.job()).phase, JobPhase::Verifying);
    assert_eq!(std::fs::read(&destination).expect("dest"), vec![7; 64]);
    assert_eq!(std::fs::read(&staged).expect("staging retained"), [7; 64]);
    assert!(sidecar.is_file());
    assert!(api.title().files.is_empty());
    drop(lock);
}

#[tokio::test(flavor = "multi_thread")]
async fn destination_hash_io_error_keeps_verifying() {
    let dir = Scratch::new();
    let (job, spec) = stage_verifying_job(&dir.0).await;
    let destination = dir.0.join(&spec.dest_rel);
    let (staged, sidecar, _) = staging_files(&spec);
    std::fs::create_dir_all(destination.parent().expect("parent")).expect("mkdir");
    std::fs::write(&destination, [7; 64]).expect("dest");
    std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    struct Restore(PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o600));
        }
    }
    let _restore = Restore(destination.clone());
    if std::fs::File::open(&destination).is_ok() {
        eprintln!("skipping destination_hash_io_error_keeps_verifying: process can read mode 000");
        return;
    }
    let api = Api::new(job);
    assert!(run_job(&api, &NeverTransfer, api.job()).await.is_err());
    assert_eq!(job_status(&api.job()).phase, JobPhase::Verifying);
    assert_eq!(std::fs::read(&staged).expect("staging retained"), [7; 64]);
    assert!(sidecar.is_file());
    assert!(api.title().files.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn mismatching_destination_preserves_staging() {
    let dir = Scratch::new();
    let (job, spec) = stage_verifying_job(&dir.0).await;
    let destination = dir.0.join(&spec.dest_rel);
    let (staged, sidecar, lock) = staging_files(&spec);
    std::fs::create_dir_all(destination.parent().expect("parent")).expect("mkdir");
    std::fs::write(&destination, [9; 64]).expect("other dest");
    let api = Api::new(job);
    run_job(&api, &NeverTransfer, api.job())
        .await
        .expect("refusal recorded");
    assert_eq!(job_status(&api.job()).phase, JobPhase::Refused);
    assert_eq!(std::fs::read(&destination).expect("dest"), vec![9; 64]);
    assert_eq!(std::fs::read(&staged).expect("staging retained"), [7; 64]);
    assert!(sidecar.is_file());
    assert!(lock.is_file());
    assert!(api.title().files.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn expired_before_publication_preserves_staging() {
    let dir = Scratch::new();
    let (mut job, spec) = stage_verifying_job(&dir.0).await;
    if let StatusBody::Job(status) = &mut job.status {
        status.started_unix = unix_now() - PULL_DEADLINE_SECS as i64 - 1;
    }
    let (staged, sidecar, lock) = staging_files(&spec);
    let api = Api::new(job);
    run_job(&api, &NeverTransfer, api.job())
        .await
        .expect("deadline recorded");
    assert_eq!(job_status(&api.job()).phase, JobPhase::Failed);
    assert!(job_status(&api.job()).message.contains("deadline"));
    assert!(!dir.0.join(&spec.dest_rel).exists());
    assert!(api.title().files.is_empty());
    assert_eq!(std::fs::read(&staged).expect("staging retained"), [7; 64]);
    assert!(sidecar.is_file());
    assert!(lock.is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn fully_staged_pull_recovers_without_gateway_or_tls_files() {
    let dir = Scratch::new();
    let mut job = job(&dir.0);
    let Spec::Job(spec) = &job.spec else {
        panic!("Job");
    };
    Memory::new()
        .pull(&pull_spec(spec).expect("pull"), Arc::new(AtomicU64::new(0)))
        .await
        .expect("stage");
    job.status = StatusBody::Job(JobStatus {
        phase: JobPhase::Pulling,
        attempts: 1,
        started_unix: unix_now(),
        ..JobStatus::default()
    });
    let api = Api::new(job);
    let gateway = Gateway {
        socket: dir.0.join("missing.sock"),
        tls: dir.0.join("missing-tls"),
    };
    run_job(&api, &gateway, api.job())
        .await
        .expect("resume staged");
    assert_eq!(job_status(&api.job()).phase, JobPhase::Installed);
    assert_eq!(job_status(&api.job()).attempts, 1);
}

struct Stalled(Arc<AtomicBool>);
struct Cancelled(Arc<AtomicBool>);
impl Drop for Cancelled {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
impl Transfer for Stalled {
    async fn pull(&self, _spec: &PullSpec, _done: Arc<AtomicU64>) -> anyhow::Result<()> {
        let cancelled = Cancelled(self.0.clone());
        std::future::pending::<()>().await;
        drop(cancelled);
        unreachable!()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stalled_transfer_is_cancelled_at_persisted_deadline() {
    let dir = Scratch::new();
    let mut job = job(&dir.0);
    job.status = StatusBody::Job(JobStatus {
        phase: JobPhase::Pulling,
        attempts: 1,
        started_unix: unix_now() - PULL_DEADLINE_SECS as i64 + 2,
        ..JobStatus::default()
    });
    let api = Api::new(job);
    let cancelled = Arc::new(AtomicBool::new(false));
    run_job(&api, &Stalled(cancelled.clone()), api.job())
        .await
        .expect("timeout recorded");
    assert!(cancelled.load(Ordering::Relaxed));
    assert_eq!(job_status(&api.job()).phase, JobPhase::Failed);
    assert!(api.title().files.is_empty());
}

#[test]
fn adding_an_episode_preserves_existing_proof_and_refuses_digest_rewrites() {
    let id = TitleId::parse("series:key:mrrobot.2015").expect("id");
    let first = mediaops_core::render(&id, &Placement::episode("Mr.Robot", 2015, 1, 1, "mkv"))
        .expect("first");
    let second = mediaops_core::render(&id, &Placement::episode("Mr.Robot", 2015, 1, 2, "mkv"))
        .expect("second");
    let original = Blake3Hex::of_bytes(b"episode one");
    let new = Blake3Hex::of_bytes(b"episode two");
    let status = with_installed_file(
        &TitleStatus::default(),
        first.to_str().expect("path"),
        &original,
    )
    .expect("first proof");
    let updated =
        with_installed_file(&status, second.to_str().expect("path"), &new).expect("second proof");
    assert_eq!(updated.files.len(), 2);
    assert_eq!(updated.files[0], status.files[0]);
    assert!(with_installed_file(&updated, first.to_str().expect("path"), &new).is_err());
}
