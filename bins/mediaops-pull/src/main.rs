use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use clap::Parser;
use mediaops_core::{
    Actor, Blake3Hex, EventStatus, HomeObject, JobPhase, JobSpec, JobStatus, Kind,
    NODE_HEARTBEAT_SECS, PULL_DEADLINE_SECS, PULL_MAX_ATTEMPTS, RemoteRef, Spec, StatusBody,
    TitleFileStatus, TitleId, TitleStatus, VerifiedStagingHandle, WorkerKind, bind_priority,
    cleanup_install_temporary, free_bytes, install_fits, install_verified_before, parse_placement,
    pull_fits, pull_remaining_bytes, staging_path,
};
use mediaops_home_client::{
    ClientError, HomeApi, claim_process, default_api_socket, default_gateway_socket,
    default_tls_dir,
};
use mediaops_transfer::{
    HomeChannel, PullSpec, RangeSource, TransferError, cleanup_verified_staging, configure_pool,
    connect_home, grpc_source, pull_file_with_progress,
};

#[derive(Parser, Debug)]
#[command(name = "mediaops-pull", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    Serve {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        gateway_socket: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    match Cli::parse().command {
        Command::Serve {
            socket,
            gateway_socket,
            tls_dir,
        } => {
            run(
                &socket.unwrap_or_else(default_api_socket),
                &Gateway {
                    socket: gateway_socket.unwrap_or_else(default_gateway_socket),
                    tls: tls_dir.unwrap_or_else(default_tls_dir),
                },
            )
            .await
        }
    }
}

async fn run(socket: &Path, gateway: &Gateway) -> anyhow::Result<()> {
    // A restart may resume Pulling, but two processes must never resume it
    // together. This process lock is independent of the legacy library flock.
    let _owner = claim_process(socket, "pull")?;
    let api = wait_api(socket).await?;
    let beat = api.clone();
    tokio::spawn(async move {
        loop {
            if let Err(err) = beat.heartbeat(WorkerKind::Pull, true, None).await {
                tracing::warn!(error = %err, "pull heartbeat failed");
            }
            tokio::time::sleep(Duration::from_secs(NODE_HEARTBEAT_SECS)).await;
        }
    });
    loop {
        if let Err(err) = claim_and_run(&api, gateway).await {
            tracing::warn!(error = %err, "pull pass failed");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn wait_api(socket: &Path) -> anyhow::Result<HomeApi> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        match HomeApi::connect(socket, Actor::Pull).await {
            Ok(api) => return Ok(api),
            Err(err) if tokio::time::Instant::now() >= deadline => return Err(err.into()),
            Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
}

async fn claim_and_run(api: &HomeApi, gateway: &Gateway) -> anyhow::Result<()> {
    let mut jobs = api.list(Some(Kind::Job)).await?;
    jobs.sort_by_key(|job| match &job.spec {
        Spec::Job(spec) => (
            TitleId::parse(&spec.title_id)
                .map(|id| bind_priority(&id))
                .unwrap_or(u8::MAX),
            job.metadata.name.clone(),
        ),
        _ => (u8::MAX, job.metadata.name.clone()),
    });
    for job in jobs {
        if matches!((&job.spec, &job.status), (Spec::Job(spec), StatusBody::Job(status))
            if spec.node_name == WorkerKind::Pull.node_name() && !status.phase.is_terminal())
        {
            return run_job(api, gateway, job).await;
        }
    }
    Ok(())
}

/// Persistence and transport are separate ports so crash windows can be tested
/// with real temporary files and deliberately failing API writes.
trait JobApi {
    async fn title(&self, name: &str) -> Result<HomeObject, ClientError>;
    async fn patch_status(&self, object: HomeObject) -> Result<HomeObject, ClientError>;
    async fn event(&self, _job: &HomeObject) {}
}

impl JobApi for HomeApi {
    async fn title(&self, name: &str) -> Result<HomeObject, ClientError> {
        self.get(Kind::Title, name).await
    }

    async fn patch_status(&self, object: HomeObject) -> Result<HomeObject, ClientError> {
        self.patch(object, "status").await
    }

    async fn event(&self, job: &HomeObject) {
        let StatusBody::Job(status) = &job.status else {
            return;
        };
        let event = HomeObject::new(
            Kind::Event,
            format!("{}-{}", job.metadata.uid, job.metadata.resource_version),
            Spec::Event,
            StatusBody::Event(EventStatus {
                involved_kind: Kind::Job.as_str().into(),
                involved_name: job.metadata.name.clone(),
                reason: status.phase.as_str().into(),
                message: status.message.clone(),
                ts: unix_now(),
            }),
        );
        if let Err(err) = self.apply(event).await {
            tracing::warn!(error = %err, "record pull event failed");
        }
    }
}

trait Transfer {
    async fn pull(&self, spec: &PullSpec, done: Arc<AtomicU64>) -> anyhow::Result<()>;
}

struct Gateway {
    socket: PathBuf,
    tls: PathBuf,
}

impl Transfer for Gateway {
    async fn pull(&self, spec: &PullSpec, done: Arc<AtomicU64>) -> anyhow::Result<()> {
        let source = Arc::new(GatewayRanges {
            socket: self.socket.clone(),
            tls: self.tls.clone(),
            concurrency: spec.concurrency as u32,
            channel: tokio::sync::OnceCell::new(),
        });
        pull_file_with_progress(source, spec, |bytes, _| {
            done.store(bytes, Ordering::Relaxed)
        })
        .await?;
        Ok(())
    }
}

/// Connect only when a range is needed. Fully staged, verified sidecars can
/// recover while the gateway is unavailable, and parallel ranges share one pool.
struct GatewayRanges {
    socket: PathBuf,
    tls: PathBuf,
    concurrency: u32,
    channel: tokio::sync::OnceCell<HomeChannel>,
}

impl RangeSource for GatewayRanges {
    async fn get_range(
        &self,
        remote: &RemoteRef,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, TransferError> {
        let channel = self
            .channel
            .get_or_try_init(|| async {
                let channel = connect_home(&self.socket, &self.tls).await?;
                configure_pool(channel.clone(), self.concurrency).await?;
                Ok::<_, TransferError>(channel)
            })
            .await?;
        grpc_source(channel.clone())
            .get_range(remote, offset, len)
            .await
    }
}

async fn run_job(
    api: &impl JobApi,
    transfer: &impl Transfer,
    mut job: HomeObject,
) -> anyhow::Result<()> {
    let (Spec::Job(spec), StatusBody::Job(status)) = (&job.spec, &job.status) else {
        anyhow::bail!("Pull requires a Job spec and status");
    };
    let spec = spec.clone();
    if status.phase.is_terminal() {
        return Ok(());
    }
    if status.phase == JobPhase::Verifying {
        return finish_verifying(api, &mut job, &spec).await;
    }
    if remaining_time(status, unix_now()).is_zero()
        || (status.phase == JobPhase::Pending && status.attempts >= PULL_MAX_ATTEMPTS)
    {
        return set_failure(
            api,
            &mut job,
            JobPhase::Failed,
            "pull retry limit or deadline reached".into(),
        )
        .await;
    }
    if status.phase == JobPhase::Pending {
        let mut next = status.clone();
        next.phase = JobPhase::Pulling;
        next.attempts += 1;
        if next.started_unix == 0 {
            next.started_unix = unix_now();
        }
        next.message.clear();
        save_job(api, &mut job, next).await?;
    }
    // A process restart continues the persisted attempt. Only a failed attempt
    // returns to Pending; the original deadline survives either kind of retry.
    let budget = remaining_time(job_status(&job), unix_now());
    let deadline = Instant::now() + budget;
    let result = tokio::time::timeout(
        budget,
        copy_and_hash(api, transfer, &mut job, &spec, deadline),
    )
    .await;
    let digest = match result {
        Ok(Ok(digest)) => digest,
        result => {
            let err = match result {
                Ok(Err(err)) => err,
                Err(_) => anyhow::anyhow!("pull deadline reached"),
                _ => unreachable!(),
            };
            let phase = if err.downcast_ref::<Refusal>().is_some() {
                JobPhase::Refused
            } else if job_status(&job).attempts >= PULL_MAX_ATTEMPTS
                || remaining_time(job_status(&job), unix_now()).is_zero()
                || Instant::now() >= deadline
            {
                JobPhase::Failed
            } else {
                JobPhase::Pending
            };
            return set_failure(api, &mut job, phase, err.to_string()).await;
        }
    };
    let mut next = job_status(&job).clone();
    next.phase = JobPhase::Verifying;
    next.bytes_done = spec.file_len;
    next.verified_b3 = Some(digest);
    // The verification digest must be durable before the filesystem changes.
    save_job(api, &mut job, next).await?;
    finish_verifying(api, &mut job, &spec).await
}

async fn copy_and_hash(
    api: &impl JobApi,
    transfer: &impl Transfer,
    job: &mut HomeObject,
    spec: &JobSpec,
    deadline: Instant,
) -> anyhow::Result<Blake3Hex> {
    let pull = pull_spec(spec)?;
    let destination = Path::new(&spec.library_root).join(&spec.dest_rel);
    match std::fs::symlink_metadata(&destination) {
        Ok(_) => return Err(Refusal("destination already exists").into()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    cleanup_install_temporary(spec)?;
    let remaining = pull_remaining_bytes(spec)?;
    let free = free_bytes(Path::new(&spec.library_root))?;
    if (spec.max_copy > 0 && spec.file_len > spec.max_copy)
        || !pull_fits(free, spec.min_free, 0, 0, remaining)
        || !install_fits(spec)?
    {
        return Err(Refusal("watermark or copy budget").into());
    }
    let done = Arc::new(AtomicU64::new(job_status(job).bytes_done));
    let copying = transfer.pull(&pull, done.clone());
    tokio::pin!(copying);
    let mut progress = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            result = &mut copying => { result?; break; },
            _ = progress.tick() => {
                let bytes = done.load(Ordering::Relaxed).min(spec.file_len);
                if bytes != job_status(job).bytes_done {
                    let mut next = job_status(job).clone();
                    next.bytes_done = bytes;
                    save_job(api, job, next).await?;
                }
            }
        }
    }
    let staged = pull
        .library_root
        .join(staging_path(&pull.title_id, &pull.final_name)?);
    // Keep the operation owned by this task; cancelling an outer timeout must
    // never leave a detached task that later installs a supposedly failed Job.
    tokio::task::block_in_place(|| hash_file(&staged, spec.file_len, Some(deadline)))
}

fn pull_spec(spec: &JobSpec) -> anyhow::Result<PullSpec> {
    let title_id = TitleId::parse(&spec.title_id)?;
    let (_, placement) = parse_placement(Path::new(&spec.dest_rel))?;
    if mediaops_core::render(&title_id, &placement)? != Path::new(&spec.dest_rel) {
        anyhow::bail!("Job destination does not match its title and placement");
    }
    let final_name = Path::new(&spec.dest_rel)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("Job destination has no filename"))?
        .to_string();
    Ok(PullSpec {
        library_root: PathBuf::from(&spec.library_root),
        title_id,
        final_name,
        remote: RemoteRef::from_wire_parts(
            spec.remote_root.clone(),
            spec.remote_path.clone().into(),
        )?,
        file_len: spec.file_len,
        range_len: spec.range_len,
        concurrency: spec.range_concurrency as usize,
    })
}

async fn finish_verifying(
    api: &impl JobApi,
    job: &mut HomeObject,
    spec: &JobSpec,
) -> anyhow::Result<()> {
    let expected = job_status(job)
        .verified_b3
        .clone()
        .ok_or_else(|| anyhow::anyhow!("verifying Job has no digest"))?;
    let deadline = Instant::now() + remaining_time(job_status(job), unix_now());
    if let Err(err) = tokio::task::block_in_place(|| place_verified(spec, &expected, deadline)) {
        if err.downcast_ref::<Retryable>().is_some() {
            return Err(err);
        }
        let phase = if err.downcast_ref::<Refusal>().is_some() {
            JobPhase::Refused
        } else {
            JobPhase::Failed
        };
        return set_failure(api, job, phase, err.to_string()).await;
    }
    // API failures leave the Job Verifying. On retry, the saved digest proves
    // the existing destination even when the transfer deadline has elapsed.
    record_title(api, spec, &expected).await?;
    let mut next = job_status(job).clone();
    next.phase = JobPhase::Installed;
    next.bytes_done = spec.file_len;
    next.message.clear();
    save_job(api, job, next).await
}

fn place_verified(spec: &JobSpec, expected: &Blake3Hex, deadline: Instant) -> anyhow::Result<()> {
    if installed_matches(spec, expected)? {
        cleanup_owned_staging(spec)?;
        cleanup_install_temporary(spec).map_err(retryable)?;
        return Ok(());
    }
    // Recover a previous process's owned partial even when its durable Job
    // deadline has elapsed; expiration must not strand allocated copy space.
    cleanup_install_temporary(spec)?;
    if Instant::now() >= deadline {
        anyhow::bail!("pull deadline reached before installation");
    }
    if !install_fits(spec)? || free_bytes(Path::new(&spec.library_root))? < spec.min_free {
        return Err(Refusal("watermark before installation").into());
    }
    let pull = pull_spec(spec)?;
    let staged = pull
        .library_root
        .join(staging_path(&pull.title_id, &pull.final_name)?);
    let (_, placement) = parse_placement(Path::new(&spec.dest_rel))?;
    let handle =
        VerifiedStagingHandle::verify(&pull.library_root, &pull.title_id, staged, &placement)?;
    match install_verified_before(
        &pull.library_root,
        &pull.title_id,
        &handle,
        expected,
        deadline,
    ) {
        Ok(_) => cleanup_owned_staging(spec),
        // Publication can succeed before source cleanup/fsync reports an error.
        // Its on-disk bytes and the saved digest decide whether it completed.
        Err(err) => match installed_matches(spec, expected) {
            Ok(true) => {
                tracing::warn!(error = %err, "installed file recovered after cleanup failure");
                cleanup_owned_staging(spec)
            }
            Ok(false) => {
                // A returned failure is terminal here. Process death is recovered
                // on startup, but a live failure must release its owned copy space
                // now because terminal Jobs are no longer selected by the worker.
                cleanup_install_temporary(spec)?;
                Err(err.into())
            }
            Err(proof) => Err(proof),
        },
    }
}

fn cleanup_owned_staging(spec: &JobSpec) -> anyhow::Result<()> {
    cleanup_verified_staging(&pull_spec(spec)?).map_err(retryable)
}

fn installed_matches(spec: &JobSpec, expected: &Blake3Hex) -> anyhow::Result<bool> {
    let destination = Path::new(&spec.library_root).join(&spec.dest_rel);
    match std::fs::symlink_metadata(&destination) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(retryable(err)),
        Ok(meta) => {
            if !meta.file_type().is_file() || meta.len() != spec.file_len {
                return Err(
                    Refusal("existing destination does not match the verified file").into(),
                );
            }
            match hash_file(&destination, spec.file_len, None) {
                Ok(digest) if digest == *expected => Ok(true),
                Ok(_) => {
                    Err(Refusal("existing destination does not match the verified file").into())
                }
                Err(err) => Err(retryable(err)),
            }
        }
    }
}

fn hash_file(
    path: &Path,
    expected_len: u64,
    deadline: Option<Instant>,
) -> anyhow::Result<Blake3Hex> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() != expected_len {
        anyhow::bail!("file has unexpected type or length: {}", path.display());
    }
    Ok(Blake3Hex::of_reader(DeadlineReader { file, deadline })?)
}

struct DeadlineReader {
    file: std::fs::File,
    deadline: Option<Instant>,
}

impl Read for DeadlineReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "pull deadline reached while verifying",
            ));
        }
        self.file.read(bytes)
    }
}

async fn record_title(api: &impl JobApi, spec: &JobSpec, digest: &Blake3Hex) -> anyhow::Result<()> {
    for attempt in 0..3 {
        let mut title = api.title(&spec.title_id).await?;
        let StatusBody::Title(status) = &title.status else {
            anyhow::bail!("Title status missing");
        };
        let next = with_installed_file(status, &spec.dest_rel, digest)?;
        if next == *status {
            return Ok(());
        }
        title.status = StatusBody::Title(next);
        match api.patch_status(title).await {
            Ok(_) => return Ok(()),
            Err(err) if err.is_conflict() && attempt < 2 => {}
            Err(err) => return Err(err.into()),
        }
    }
    unreachable!()
}

fn with_installed_file(
    status: &TitleStatus,
    path: &str,
    digest: &Blake3Hex,
) -> anyhow::Result<TitleStatus> {
    let mut files = status.observed_files();
    if let Some(file) = files.iter().find(|file| file.path == path) {
        if file.install_b3 != *digest || file.current_b3 != *digest || file.drifted {
            anyhow::bail!("existing Title proof differs from the installed file");
        }
    } else {
        files.push(TitleFileStatus {
            path: path.into(),
            install_b3: digest.clone(),
            current_b3: digest.clone(),
            drifted: false,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let first = &files[0];
    Ok(TitleStatus {
        path: first.path.clone(),
        install_b3: Some(first.install_b3.clone()),
        current_b3: Some(first.current_b3.clone()),
        drifted: files.iter().any(|file| file.drifted),
        files,
    })
}

fn remaining_time(status: &JobStatus, now: i64) -> Duration {
    if status.started_unix == 0 {
        return Duration::from_secs(PULL_DEADLINE_SECS);
    }
    let elapsed = now.saturating_sub(status.started_unix).max(0) as u64;
    Duration::from_secs(PULL_DEADLINE_SECS.saturating_sub(elapsed))
}

fn job_status(job: &HomeObject) -> &JobStatus {
    match &job.status {
        StatusBody::Job(status) => status,
        _ => unreachable!("validated Job"),
    }
}

async fn set_failure(
    api: &impl JobApi,
    job: &mut HomeObject,
    phase: JobPhase,
    message: String,
) -> anyhow::Result<()> {
    let mut status = job_status(job).clone();
    status.phase = phase;
    status.message = message;
    save_job(api, job, status).await
}

async fn save_job(
    api: &impl JobApi,
    job: &mut HomeObject,
    status: JobStatus,
) -> anyhow::Result<()> {
    let changed_phase = job_status(job).phase != status.phase;
    let mut next = job.clone();
    next.status = StatusBody::Job(status);
    *job = api.patch_status(next).await?;
    if changed_phase {
        api.event(job).await;
    }
    Ok(())
}

#[derive(Debug)]
struct Refusal(&'static str);
impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Refusal {}

#[derive(Debug)]
struct Retryable(anyhow::Error);
impl std::fmt::Display for Retryable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Retryable {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

fn retryable(err: impl Into<anyhow::Error>) -> anyhow::Error {
    Retryable(err.into()).into()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn init_tracing() {
    let subscriber = tracing_subscriber::fmt().with_writer(io::stderr);
    if io::stderr().is_terminal() {
        subscriber.init();
    } else {
        subscriber.json().init();
    }
}

#[cfg(test)]
mod tests;
