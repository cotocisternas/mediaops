use std::path::Path;

use super::beat::wait_connect;
use super::errors::FixtureError;
use super::seed_media::MovieFile;
use mediaops_core::{
    Actor, ClusterSpec, HomeObject, JobPhase, JobSpec, JobStatus, Kind, Spec, StatusBody,
    WorkerKind,
};

pub(super) struct JobSeed<'a> {
    pub name: &'a str,
    pub file: &'a MovieFile,
    pub file_len: u64,
    pub hold_name: &'a str,
    pub phase: JobPhase,
    pub bytes_done: u64,
    pub message: String,
    pub now: i64,
}

pub(super) async fn seed_job(
    socket: &Path,
    library: &Path,
    seed: JobSeed<'_>,
) -> Result<(), FixtureError> {
    let library_root = library
        .to_str()
        .ok_or_else(|| FixtureError::Invalid("library path is not utf-8".into()))?
        .to_owned();
    let controller = wait_connect(socket, Actor::Controller).await?;
    let unbound = HomeObject::new(
        Kind::Job,
        seed.name,
        Spec::Job(JobSpec {
            hold_name: seed.hold_name.into(),
            title_id: seed.file.title_id.into(),
            remote_root: seed.file.remote_root.into(),
            remote_path: seed.file.remote_path.clone(),
            dest_rel: seed.file.dest_rel.clone(),
            file_len: seed.file_len,
            range_len: ClusterSpec::default().range_len.get(),
            range_concurrency: 1,
            library_root,
            worker_kind: WorkerKind::Pull.as_str().to_string(),
            ..JobSpec::default()
        }),
        StatusBody::Job(JobStatus::default()),
    );
    let stored = match controller.apply(unbound).await {
        Ok(job) => job,
        Err(err) if err.is_conflict() => controller.get(Kind::Job, seed.name).await?,
        Err(err) => return Err(err.into()),
    };
    let bound = bind_if_needed(socket, stored).await?;
    patch_progress(socket, bound, &seed).await
}

async fn bind_if_needed(socket: &Path, mut job: HomeObject) -> Result<HomeObject, FixtureError> {
    let Spec::Job(spec) = &job.spec else {
        return Err(FixtureError::Invalid("Job spec missing".into()));
    };
    if !spec.node_name.is_empty() {
        return Ok(job);
    }
    if let Spec::Job(spec) = &mut job.spec {
        spec.node_name = WorkerKind::Pull.node_name().into();
        spec.worker_kind = WorkerKind::Pull.as_str().to_string();
    }
    let scheduler = wait_connect(socket, Actor::Scheduler).await?;
    Ok(scheduler.patch(job, "bind").await?)
}

async fn patch_progress(
    socket: &Path,
    mut job: HomeObject,
    seed: &JobSeed<'_>,
) -> Result<(), FixtureError> {
    let StatusBody::Job(before) = &job.status else {
        return Err(FixtureError::Invalid("Job status missing".into()));
    };
    if before.phase == seed.phase {
        return Ok(());
    }
    job.status = StatusBody::Job(JobStatus {
        phase: seed.phase,
        bytes_done: seed.bytes_done,
        attempts: 1,
        started_unix: match seed.phase {
            JobPhase::Pulling | JobPhase::Verifying | JobPhase::Installed => seed.now,
            JobPhase::Pending | JobPhase::Refused | JobPhase::Failed => 0,
        },
        message: seed.message.clone(),
        ..JobStatus::default()
    });
    let pull = wait_connect(socket, Actor::Pull).await?;
    pull.patch(job, "status").await?;
    Ok(())
}
