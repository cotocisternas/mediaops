//! Apply a Plan artifact in the locked CLI process.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mediaops_core::{
    Action, ControlError, ControlPort, InstallError, Job, JobEvent, JobId, JobKind, JobState,
    JobsRepo, PathSchemaError, Placement, Plan, PlanError, PullEvent, PullState, RemoteRef,
    SKIP_UPGRADE_NEVER, TitleId, TitleIndexRepo, VerifiedStagingHandle, WantEvent, WantState,
    install, render, staging_path,
};
use mediaops_transfer::{PullSpec, RangeSource, pull_file};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledCopy {
    pub title_id: TitleId,
    pub path: PathBuf,
    pub pull_job_id: JobId,
    pub placement: Placement,
}

/// An [`Action::Unmonitor`] the grabber refused. Reported, never fatal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmonitorFailure {
    pub title_id: TitleId,
    pub error: String,
}

/// One [`Action::Copy`] that did not install this run. The loop goes on to the
/// next action: a bad remote, a dropped WAN mid-file, or a stuck job must not
/// starve every file queued behind it. The `.partial` stays for next run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyFailure {
    pub title_id: TitleId,
    pub remote: RemoteRef,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApplyReport {
    pub copies: usize,
    pub skips: usize,
    pub reviews: usize,
    pub installed: Vec<InstalledCopy>,
    pub copy_failed: Vec<CopyFailure>,
    pub unmonitor_failed: Vec<UnmonitorFailure>,
    pub deleted: usize,
    pub skipped_seeding: usize,
    pub qbit_unavailable: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    #[error("plan snapshot does not match active desired-state bytes")]
    SnapshotMismatch,
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    PathSchema(#[from] PathSchemaError),
    #[error(transparent)]
    Install(#[from] InstallError),
    #[error("jobs: {0}")]
    Jobs(String),
    #[error("title-index: {0}")]
    Titles(String),
    #[error("transfer: {0}")]
    Transfer(String),
    #[error("control: {0}")]
    Control(#[from] ControlError),
}

impl ApplyError {
    pub fn is_snapshot_mismatch(&self) -> bool {
        matches!(self, Self::SnapshotMismatch)
    }
}

pub struct ApplyCtx<'a, J, T, S> {
    pub jobs: &'a J,
    pub titles: &'a T,
    pub source: Arc<S>,
    pub library_root: &'a Path,
    pub concurrency: usize,
    pub control: Option<&'a dyn ControlPort>,
}

pub async fn apply<J, T, S>(
    plan: &Plan,
    active_toml: &[u8],
    ctx: ApplyCtx<'_, J, T, S>,
) -> Result<ApplyReport, ApplyError>
where
    J: JobsRepo,
    T: TitleIndexRepo,
    S: RangeSource + 'static,
    J::Error: std::fmt::Display,
    T::Error: std::fmt::Display,
{
    if !plan.matches_snapshot(active_toml) {
        return Err(ApplyError::SnapshotMismatch);
    }
    let ds = plan.desired_state().map_err(ApplyError::from_desired)?;
    let mut report = ApplyReport::default();
    for action in plan.actions() {
        match action {
            Action::Copy {
                title_id,
                remote,
                file_len,
                placement,
            } => {
                if ds.lock() {
                    report.skips += 1;
                    continue;
                }
                match apply_copy(
                    ctx.jobs,
                    ctx.titles,
                    ctx.source.clone(),
                    ctx.library_root,
                    ctx.concurrency,
                    ds.range_len().get(),
                    title_id,
                    remote,
                    *file_len,
                    placement,
                )
                .await
                {
                    Ok(installed) => {
                        report.copies += 1;
                        report.installed.push(installed);
                    }
                    // Snapshot drift is the one thing that invalidates the
                    // whole artifact; everything else is per-file data.
                    Err(err @ ApplyError::SnapshotMismatch) => return Err(err),
                    Err(err) => {
                        tracing::warn!(
                            title = %title_id,
                            remote = %remote.rel_path().display(),
                            error = %err,
                            "copy failed; continuing with the next action"
                        );
                        report.copy_failed.push(CopyFailure {
                            title_id: title_id.clone(),
                            remote: remote.clone(),
                            error: err.to_string(),
                        });
                    }
                }
            }
            Action::Skip {
                title_id: Some(id),
                reason,
            } if reason == SKIP_UPGRADE_NEVER => {
                satisfy_open_want(ctx.jobs, id).await?;
                report.skips += 1;
            }
            Action::Skip { .. } => report.skips += 1,
            Action::Review { .. } => report.reviews += 1,
            Action::GrabApply => {
                if let Some(control) = ctx.control {
                    let _report = control.grab_apply(plan.desired_state_toml()).await?;
                }
            }
            Action::EdgeApply => {
                if let Some(control) = ctx.control {
                    let _report = control.edge_apply(plan.desired_state_toml()).await?;
                }
            }
            Action::Unmonitor { title_id } => {
                if let Some(control) = ctx.control {
                    // Never fatal. Unmonitor is ordered after Copy, so aborting
                    // here would strand copies that are already installed and
                    // still owe their post-install encode -- and they would not
                    // get a second chance, since the next plan skips them
                    // upgrade-never. The action itself is idempotent and the
                    // next run re-emits it, so a failure only costs a cycle.
                    if let Err(err) = control.unmonitor(title_id).await {
                        report.unmonitor_failed.push(UnmonitorFailure {
                            title_id: title_id.clone(),
                            error: err.message,
                        });
                    }
                }
            }
            Action::DeleteRemote { remote } => {
                if let Some(control) = ctx.control {
                    match control.delete_remote(remote).await? {
                        mediaops_core::DeleteRemoteOutcome::Deleted => report.deleted += 1,
                        mediaops_core::DeleteRemoteOutcome::SkippedSeeding => {
                            report.skipped_seeding += 1
                        }
                        mediaops_core::DeleteRemoteOutcome::QbitUnavailable => {
                            report.qbit_unavailable += 1
                        }
                    }
                }
            }
            Action::Encode { .. } | Action::Reclaim => {}
        }
    }
    Ok(report)
}

impl ApplyError {
    fn from_desired(err: mediaops_core::DesiredStateError) -> Self {
        Self::Plan(PlanError::DesiredState(err))
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_copy<J, T, S>(
    jobs: &J,
    titles: &T,
    source: Arc<S>,
    library_root: &Path,
    concurrency: usize,
    range_len: u64,
    title_id: &TitleId,
    remote: &mediaops_core::RemoteRef,
    file_len: u64,
    placement: &Placement,
) -> Result<InstalledCopy, ApplyError>
where
    J: JobsRepo,
    T: TitleIndexRepo,
    S: RangeSource + 'static,
    J::Error: std::fmt::Display,
    T::Error: std::fmt::Display,
{
    let dest_rel = render(title_id, placement)?;
    let final_name = dest_rel
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| ApplyError::Jobs("destination file name is not utf-8".into()))?
        .to_string();

    let existing = jobs
        .list_by_title(title_id)
        .await
        .map_err(|err| ApplyError::Jobs(err.to_string()))?;
    let want = existing
        .iter()
        .find(|j| matches!(j.state(), JobState::Want(WantState::Open)))
        .cloned();
    let inflight = existing.iter().find(|j| {
        j.kind() == JobKind::Pull && !matches!(j.state(), JobState::Pull(PullState::Installed))
    });
    let pull = match inflight {
        Some(job) if pull_matches(library_root, title_id, remote, file_len, &final_name)? => {
            job.clone()
        }
        Some(job) => {
            // The remote this job was pulling is gone or changed (an *arr
            // upgrade, a re-download). Its `.partial` can never complete;
            // drop that staging and let the job carry on with the new remote
            // rather than failing every run from here on.
            tracing::warn!(
                job = %job.id(),
                title = %title_id,
                "in-flight pull no longer matches its remote; discarding stale staging"
            );
            discard_stale_staging(library_root, title_id)?;
            job.clone()
        }
        None => jobs
            .create(JobKind::Pull, title_id, want.as_ref().map(Job::id))
            .await
            .map_err(|err| ApplyError::Jobs(err.to_string()))?,
    };

    let mut pull = pull;
    if matches!(pull.state(), JobState::Pull(PullState::Queued)) {
        write_pull_intent(library_root, title_id, remote, file_len, &final_name)?;
        pull = jobs
            .advance(pull.id(), JobEvent::Pull(PullEvent::Start))
            .await
            .map_err(|err| ApplyError::Jobs(err.to_string()))?;
    }

    if matches!(pull.state(), JobState::Pull(PullState::Pulling)) {
        write_pull_intent(library_root, title_id, remote, file_len, &final_name)?;
        let spec = PullSpec {
            library_root: library_root.to_path_buf(),
            title_id: title_id.clone(),
            final_name: final_name.clone(),
            remote: remote.clone(),
            file_len,
            range_len,
            concurrency: concurrency.max(1),
        };
        pull_file(source, &spec)
            .await
            .map_err(|err| ApplyError::Transfer(err.to_string()))?;
        pull = jobs
            .advance(pull.id(), JobEvent::Pull(PullEvent::FinishRanges))
            .await
            .map_err(|err| ApplyError::Jobs(err.to_string()))?;
    }

    if matches!(pull.state(), JobState::Pull(PullState::Verifying)) {
        let staged = library_root.join(staging_path(title_id, &final_name)?);
        let handle = VerifiedStagingHandle::verify(library_root, title_id, staged, placement)?;
        let outcome = install(library_root, title_id, &handle)?;
        let path_str = dest_rel
            .to_str()
            .ok_or_else(|| ApplyError::Jobs("schema path is not utf-8".into()))?;
        pull = jobs
            .advance(pull.id(), JobEvent::Pull(PullEvent::Install))
            .await
            .map_err(|err| ApplyError::Jobs(err.to_string()))?;
        titles
            .record_install(title_id, &outcome.whole_file_b3, path_str)
            .await
            .map_err(|err| ApplyError::Titles(err.to_string()))?;
        if let Some(want) = want {
            jobs.advance(want.id(), JobEvent::Want(WantEvent::Satisfy))
                .await
                .map_err(|err| ApplyError::Jobs(err.to_string()))?;
        }
        // The intent file was the only thing keeping `_incoming/<token>/`
        // alive; drop it so the empty-dir prune can reclaim the directory.
        let _ = fs::remove_file(pull_intent_path(library_root, title_id));
        let _ = mediaops_transfer::prune_empty_incoming(&library_root.join("_incoming"));
        return Ok(InstalledCopy {
            title_id: title_id.clone(),
            path: outcome.path,
            pull_job_id: pull.id(),
            placement: placement.clone(),
        });
    }

    Err(ApplyError::Jobs(format!(
        "pull job {} in unexpected state {}",
        pull.id(),
        pull.state()
    )))
}

async fn satisfy_open_want<J>(jobs: &J, title_id: &TitleId) -> Result<(), ApplyError>
where
    J: JobsRepo,
    J::Error: std::fmt::Display,
{
    let existing = jobs
        .list_by_title(title_id)
        .await
        .map_err(|err| ApplyError::Jobs(err.to_string()))?;
    for want in existing
        .iter()
        .filter(|j| matches!(j.state(), JobState::Want(WantState::Open)))
    {
        jobs.advance(want.id(), JobEvent::Want(WantEvent::Satisfy))
            .await
            .map_err(|err| ApplyError::Jobs(err.to_string()))?;
    }
    Ok(())
}

fn pull_intent_path(library_root: &Path, title_id: &TitleId) -> PathBuf {
    library_root
        .join("_incoming")
        .join(title_id.staging_token())
        .join("pull-intent.json")
}

/// Remove `_incoming/<token>/` for a title whose in-flight remote changed.
/// The only thing this can hold is a `.partial`, its sidecar, a fully staged
/// file for the old remote, and the intent file — none of which can complete.
fn discard_stale_staging(library_root: &Path, title_id: &TitleId) -> Result<(), ApplyError> {
    let dir = library_root
        .join("_incoming")
        .join(title_id.staging_token());
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(ApplyError::Jobs(format!(
            "discard stale staging {}: {err}",
            dir.display()
        ))),
    }
}

fn write_pull_intent(
    library_root: &Path,
    title_id: &TitleId,
    remote: &RemoteRef,
    file_len: u64,
    final_name: &str,
) -> Result<(), ApplyError> {
    let path = pull_intent_path(library_root, title_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| ApplyError::Jobs(err.to_string()))?;
    }
    let body = format!(
        "{}\n{}\n{}\n{}\n",
        remote.root_id(),
        remote.rel_path().display(),
        file_len,
        final_name
    );
    fs::write(&path, body).map_err(|err| ApplyError::Jobs(err.to_string()))
}

fn read_pull_intent(
    library_root: &Path,
    title_id: &TitleId,
) -> Result<Option<(String, PathBuf, u64, String)>, ApplyError> {
    let path = pull_intent_path(library_root, title_id);
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).map_err(|err| ApplyError::Jobs(err.to_string()))?;
    let mut lines = text.lines();
    let root_id = lines.next().unwrap_or_default().to_string();
    let rel_path = PathBuf::from(lines.next().unwrap_or_default());
    let file_len = lines
        .next()
        .unwrap_or_default()
        .parse::<u64>()
        .map_err(|err| ApplyError::Jobs(err.to_string()))?;
    let final_name = lines.next().unwrap_or_default().to_string();
    Ok(Some((root_id, rel_path, file_len, final_name)))
}

fn pull_matches(
    library_root: &Path,
    title_id: &TitleId,
    remote: &RemoteRef,
    file_len: u64,
    final_name: &str,
) -> Result<bool, ApplyError> {
    match read_pull_intent(library_root, title_id)? {
        Some((root_id, rel_path, intent_len, intent_name)) => Ok(root_id == remote.root_id()
            && rel_path == remote.rel_path()
            && intent_len == file_len
            && intent_name == final_name),
        None => Ok(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{
        Blake3Hex, JobError, JobEvent, JobKind, JobState, PullEvent, RemoteRef, SKIP_UPGRADE_NEVER,
        TitleId, TitleIndexEntry, TitleIndexError, WantState, parse_placement,
    };
    use mediaops_transfer::TransferError;
    use std::collections::HashMap;
    use std::fs;
    use std::sync::Mutex;
    use std::time::UNIX_EPOCH;

    const DS: &str = "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n";

    struct MemJobs {
        jobs: Mutex<Vec<Job>>,
        next: Mutex<i64>,
    }

    impl MemJobs {
        fn new() -> Self {
            Self {
                jobs: Mutex::new(Vec::new()),
                next: Mutex::new(1),
            }
        }
    }

    impl JobsRepo for MemJobs {
        type Error = JobError;

        async fn get(&self, id: JobId) -> Result<Option<Job>, Self::Error> {
            Ok(self
                .jobs
                .lock()
                .expect("lock")
                .iter()
                .find(|j| j.id() == id)
                .cloned())
        }

        async fn list(&self) -> Result<Vec<Job>, Self::Error> {
            Ok(self.jobs.lock().expect("lock").clone())
        }

        async fn list_by_title(&self, title_id: &TitleId) -> Result<Vec<Job>, Self::Error> {
            Ok(self
                .jobs
                .lock()
                .expect("lock")
                .iter()
                .filter(|j| j.title_id() == title_id)
                .cloned()
                .collect())
        }

        async fn create(
            &self,
            kind: JobKind,
            title_id: &TitleId,
            parent_job_id: Option<JobId>,
        ) -> Result<Job, Self::Error> {
            let mut next = self.next.lock().expect("next");
            let id = JobId::new(*next)?;
            *next += 1;
            let job = Job::new(id, title_id.clone(), JobState::initial(kind), parent_job_id)?;
            self.jobs.lock().expect("lock").push(job.clone());
            Ok(job)
        }

        async fn advance(&self, id: JobId, event: JobEvent) -> Result<Job, Self::Error> {
            let mut jobs = self.jobs.lock().expect("lock");
            let idx = jobs
                .iter()
                .position(|j| j.id() == id)
                .ok_or(JobError::InvalidId(0))?;
            let next = jobs[idx].advance(event)?;
            jobs[idx] = next.clone();
            Ok(next)
        }
    }

    struct MemTitles {
        rows: Mutex<HashMap<String, TitleIndexEntry>>,
    }

    impl MemTitles {
        fn new() -> Self {
            Self {
                rows: Mutex::new(HashMap::new()),
            }
        }
    }

    impl TitleIndexRepo for MemTitles {
        type Error = TitleIndexError;

        async fn get(&self, title_id: &TitleId) -> Result<Vec<TitleIndexEntry>, Self::Error> {
            Ok(self
                .rows
                .lock()
                .expect("lock")
                .values()
                .filter(|r| r.title_id() == title_id)
                .cloned()
                .collect())
        }

        async fn get_path(&self, path: &str) -> Result<Option<TitleIndexEntry>, Self::Error> {
            Ok(self.rows.lock().expect("lock").get(path).cloned())
        }

        async fn list(&self) -> Result<Vec<TitleIndexEntry>, Self::Error> {
            Ok(self.rows.lock().expect("lock").values().cloned().collect())
        }

        async fn record_install(
            &self,
            title_id: &TitleId,
            digest: &Blake3Hex,
            path: &str,
        ) -> Result<(), Self::Error> {
            let mut rows = self.rows.lock().expect("lock");
            if let Some(existing) = rows.get(path) {
                if existing.install_b3() != digest {
                    return Err(TitleIndexError::InstallDigestImmutable);
                }
                return Ok(());
            }
            rows.insert(
                path.to_string(),
                TitleIndexEntry::new(
                    title_id.clone(),
                    path.to_string(),
                    digest.clone(),
                    digest.clone(),
                ),
            );
            Ok(())
        }

        async fn record_replace(
            &self,
            path: &str,
            current_b3: &Blake3Hex,
        ) -> Result<(), Self::Error> {
            let mut rows = self.rows.lock().expect("lock");
            let existing = rows
                .get(path)
                .cloned()
                .ok_or(TitleIndexError::NotInstalled)?;
            rows.insert(
                path.to_string(),
                TitleIndexEntry::new(
                    existing.title_id().clone(),
                    existing.path().to_string(),
                    existing.install_b3().clone(),
                    current_b3.clone(),
                ),
            );
            Ok(())
        }

        async fn import_rows(&self, rows: &[TitleIndexEntry]) -> Result<(), Self::Error> {
            let mut map = self.rows.lock().expect("lock");
            if !map.is_empty() {
                return Err(TitleIndexError::NotEmpty);
            }
            for row in rows {
                map.insert(row.path().to_string(), row.clone());
            }
            Ok(())
        }

        async fn rewrite_absolute_prefix(
            &self,
            old_root: &str,
            new_root: &str,
        ) -> Result<u64, Self::Error> {
            let mut map = self.rows.lock().expect("lock");
            let mut rewritten = 0_u64;
            for row in map.values_mut() {
                let Some(new_path) =
                    mediaops_core::rewrite_absolute_under(row.path(), old_root, new_root)
                else {
                    continue;
                };
                *row = TitleIndexEntry::new(
                    row.title_id().clone(),
                    new_path,
                    row.install_b3().clone(),
                    row.current_b3().clone(),
                );
                rewritten += 1;
            }
            Ok(rewritten)
        }
    }

    struct MemSource {
        body: Vec<u8>,
        fail_from: Option<u64>,
        hits: Mutex<Vec<(u64, u64)>>,
    }

    impl RangeSource for MemSource {
        async fn get_range(
            &self,
            _remote: &RemoteRef,
            offset: u64,
            len: u64,
        ) -> Result<Vec<u8>, TransferError> {
            self.hits.lock().expect("hits").push((offset, len));
            if self.fail_from.is_some_and(|n| offset >= n) {
                return Err(TransferError::Rpc("killed mid-file".into()));
            }
            let start = offset as usize;
            let end = start + len as usize;
            Ok(self.body[start..end].to_vec())
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-apply-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn movie_copy(body_len: u64) -> Action {
        let rel = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
        let (title_id, placement) = parse_placement(rel).expect("placement");
        Action::Copy {
            title_id,
            remote: RemoteRef::from_wire_parts("seed".into(), PathBuf::from(rel)).expect("ref"),
            file_len: body_len,
            placement,
        }
    }

    #[tokio::test]
    async fn hash_mismatch_refuses() {
        let plan = Plan::from_toml_bytes(DS.as_bytes())
            .expect("plan")
            .with_actions(vec![movie_copy(4)]);
        let jobs = MemJobs::new();
        let titles = MemTitles::new();
        let src = Arc::new(MemSource {
            body: b"abcd".to_vec(),
            fail_from: None,
            hits: Mutex::new(Vec::new()),
        });
        let root = scratch("mismatch");
        let err = apply(
            &plan,
            b"schema_version = 1\nmax_copy_gib = 2\nmin_free_gib = 0\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n",
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
                control: None,
            },
        )
        .await
        .expect_err("mismatch");
        assert!(err.is_snapshot_mismatch());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn install_goes_through_pathschema_spaces_and_scene_tags_fail() {
        let spaced = Action::Copy {
            title_id: TitleId::movie_key("The.Matrix", 1999).expect("id"),
            remote: RemoteRef::from_wire_parts(
                "seed".into(),
                PathBuf::from("movies/The.Matrix.(1999)/The.Matrix.(1999).mkv"),
            )
            .expect("ref"),
            file_len: 4,
            placement: Placement::movie("The Matrix", 1999, "mkv"),
        };
        let plan = Plan::from_toml_bytes(DS.as_bytes())
            .expect("plan")
            .with_actions(vec![spaced]);
        let jobs = MemJobs::new();
        let titles = MemTitles::new();
        let src = Arc::new(MemSource {
            body: b"abcd".to_vec(),
            fail_from: None,
            hits: Mutex::new(Vec::new()),
        });
        let root = scratch("spaces");
        let report = apply(
            &plan,
            DS.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
                control: None,
            },
        )
        .await
        .expect("a bad copy is reported, not fatal");
        assert_eq!(report.copies, 0);
        assert_eq!(report.copy_failed.len(), 1);
        assert!(
            report.copy_failed[0].error.contains("space refused"),
            "{:?}",
            report.copy_failed
        );
        assert!(!root.join("movies").exists(), "nothing installed");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn copy_from_scene_named_remote_installs_on_pathschema_not_scene_name() {
        let action = Action::Copy {
            title_id: TitleId::movie_key("The.Matrix", 1999).expect("id"),
            remote: RemoteRef::from_wire_parts(
                "seed".into(),
                PathBuf::from("The.Matrix.1999.REPACK.mkv"),
            )
            .expect("ref"),
            file_len: 4,
            placement: Placement::movie("The.Matrix", 1999, "mkv"),
        };
        let plan = Plan::from_toml_bytes(DS.as_bytes())
            .expect("plan")
            .with_actions(vec![action]);
        let jobs = MemJobs::new();
        let titles = MemTitles::new();
        let src = Arc::new(MemSource {
            body: b"abcd".to_vec(),
            fail_from: None,
            hits: Mutex::new(Vec::new()),
        });
        let root = scratch("hold-copy-schema");
        let report = apply(
            &plan,
            DS.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
                control: None,
            },
        )
        .await
        .expect("copy");
        assert_eq!(report.copies, 1);
        let installed = report.installed[0].path.to_str().expect("utf8");
        assert!(
            installed.contains("movies/The.Matrix.(1999)/The.Matrix.(1999).mkv"),
            "install must use PathSchema, not the scene name: {installed}"
        );
        assert!(!installed.contains("REPACK"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn partial_resume_still_works_when_apply_is_killed_mid_file() {
        const MIB: usize = 1 << 20;
        let ds = "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 1\nmax_nvenc = 1\nlock = false\n";
        let body = vec![b'x'; 2 * MIB];
        let action = movie_copy(body.len() as u64);
        let plan = Plan::from_toml_bytes(ds.as_bytes())
            .expect("plan")
            .with_actions(vec![action.clone()]);
        let jobs = MemJobs::new();
        let titles = MemTitles::new();
        let root = scratch("resume");
        let src = Arc::new(MemSource {
            body: body.clone(),
            fail_from: Some(MIB as u64),
            hits: Mutex::new(Vec::new()),
        });
        let report = apply(
            &plan,
            ds.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
                control: None,
            },
        )
        .await
        .expect("a dropped transfer is reported, not fatal");
        assert_eq!(report.copies, 0);
        assert_eq!(report.copy_failed.len(), 1);
        assert!(
            report.copy_failed[0].error.contains("killed"),
            "{:?}",
            report.copy_failed
        );

        let src = Arc::new(MemSource {
            body: body.clone(),
            fail_from: None,
            hits: Mutex::new(Vec::new()),
        });
        let report = apply(
            &plan,
            ds.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
                control: None,
            },
        )
        .await
        .expect("resume");
        assert_eq!(report.copies, 1);
        let installed = &report.installed[0].path;
        assert_eq!(fs::read(installed).expect("read"), body);
        let listed = titles.list().await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].title_id(),
            &TitleId::movie_key("The.Matrix", 1999).expect("id")
        );
        assert!(!listed[0].path_missing());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn lock_true_snapshot_does_not_copy() {
        let locked = "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 8\nmax_nvenc = 1\nlock = true\n";
        let plan = Plan::from_toml_bytes(locked.as_bytes())
            .expect("plan")
            .with_actions(vec![movie_copy(4)]);
        let jobs = MemJobs::new();
        let titles = MemTitles::new();
        let src = Arc::new(MemSource {
            body: b"abcd".to_vec(),
            fail_from: None,
            hits: Mutex::new(Vec::new()),
        });
        let root = scratch("locked");
        let report = apply(
            &plan,
            locked.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
                control: None,
            },
        )
        .await
        .expect("apply");
        assert_eq!(report.copies, 0);
        assert_eq!(report.skips, 1);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn apply_parents_pull_on_open_want_and_satisfies() {
        let plan = Plan::from_toml_bytes(DS.as_bytes())
            .expect("plan")
            .with_actions(vec![movie_copy(4)]);
        let jobs = MemJobs::new();
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let want = jobs
            .create(JobKind::Want, &title, None)
            .await
            .expect("want");
        let titles = MemTitles::new();
        let src = Arc::new(MemSource {
            body: b"abcd".to_vec(),
            fail_from: None,
            hits: Mutex::new(Vec::new()),
        });
        let root = scratch("want-parent");
        let report = apply(
            &plan,
            DS.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
                control: None,
            },
        )
        .await
        .expect("apply");
        assert_eq!(report.copies, 1);
        let pull = jobs
            .get(report.installed[0].pull_job_id)
            .await
            .expect("get")
            .expect("pull");
        assert_eq!(pull.parent_job_id(), Some(want.id()));
        let want = jobs.get(want.id()).await.expect("get").expect("want");
        assert!(matches!(want.state(), JobState::Want(WantState::Satisfied)));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn upgrade_never_skip_satisfies_open_want() {
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let plan = Plan::from_toml_bytes(DS.as_bytes())
            .expect("plan")
            .with_actions(vec![Action::Skip {
                title_id: Some(title.clone()),
                reason: SKIP_UPGRADE_NEVER.to_string(),
            }]);
        let jobs = MemJobs::new();
        let want = jobs
            .create(JobKind::Want, &title, None)
            .await
            .expect("want");
        let titles = MemTitles::new();
        let src = Arc::new(MemSource {
            body: b"abcd".to_vec(),
            fail_from: None,
            hits: Mutex::new(Vec::new()),
        });
        let root = scratch("upgrade-satisfy");
        let report = apply(
            &plan,
            DS.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
                control: None,
            },
        )
        .await
        .expect("apply");
        assert_eq!(report.skips, 1);
        let want = jobs.get(want.id()).await.expect("get").expect("want");
        assert!(matches!(want.state(), JobState::Want(WantState::Satisfied)));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn in_flight_pull_with_different_remote_is_refused() {
        let plan = Plan::from_toml_bytes(DS.as_bytes())
            .expect("plan")
            .with_actions(vec![movie_copy(4)]);
        let jobs = MemJobs::new();
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let pull = jobs
            .create(JobKind::Pull, &title, None)
            .await
            .expect("pull");
        jobs.advance(pull.id(), JobEvent::Pull(PullEvent::Start))
            .await
            .expect("start");
        let root = scratch("mismatch-pull");
        write_pull_intent(
            &root,
            &title,
            &RemoteRef::from_wire_parts(
                "seed".into(),
                PathBuf::from("movies/Other.(2000)/Other.(2000).mkv"),
            )
            .expect("ref"),
            4,
            "The.Matrix.(1999).mkv",
        )
        .expect("intent");
        let titles = MemTitles::new();
        let src = Arc::new(MemSource {
            body: b"abcd".to_vec(),
            fail_from: None,
            hits: Mutex::new(Vec::new()),
        });
        // Leave a stale partial from the old remote behind as well.
        let stale = root
            .join("_incoming")
            .join(title.staging_token())
            .join("The.Matrix.(1999).mkv.partial");
        fs::write(&stale, b"old").expect("stale partial");
        let report = apply(
            &plan,
            DS.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
                control: None,
            },
        )
        .await
        .expect("a changed remote restarts the pull, it does not wedge it");
        assert_eq!(report.copies, 1, "{report:?}");
        assert!(report.copy_failed.is_empty());
        assert!(!stale.exists(), "stale staging is discarded");
        assert_eq!(
            fs::read(&report.installed[0].path).expect("installed"),
            b"abcd"
        );
        let _ = fs::remove_dir_all(root);
    }

    struct FakeControl {
        calls: Mutex<usize>,
        unmonitors: Mutex<Vec<TitleId>>,
        unmonitor_err: bool,
        deletes: Mutex<Vec<RemoteRef>>,
        delete_outcome: mediaops_core::DeleteRemoteOutcome,
    }

    impl FakeControl {
        fn new() -> Self {
            Self {
                calls: Mutex::new(0),
                unmonitors: Mutex::new(Vec::new()),
                unmonitor_err: false,
                deletes: Mutex::new(Vec::new()),
                delete_outcome: mediaops_core::DeleteRemoteOutcome::Deleted,
            }
        }
    }

    impl ControlPort for FakeControl {
        fn df(
            &self,
        ) -> mediaops_core::BoxFuture<'_, Result<mediaops_core::DfSnapshot, ControlError>> {
            Box::pin(async {
                Ok(mediaops_core::DfSnapshot {
                    free: mediaops_core::Bytes::new(0),
                    semver: "0.1.0".into(),
                    proto_package: "mediaops.v1".into(),
                })
            })
        }
        fn unmonitor<'a>(
            &'a self,
            title_id: &'a TitleId,
        ) -> mediaops_core::BoxFuture<'a, Result<(), ControlError>> {
            self.unmonitors.lock().expect("lock").push(title_id.clone());
            let fail = self.unmonitor_err;
            Box::pin(async move {
                if fail {
                    return Err(ControlError::runtime("sonarr 500"));
                }
                Ok(())
            })
        }
        fn delete_remote<'a>(
            &'a self,
            remote: &'a RemoteRef,
        ) -> mediaops_core::BoxFuture<'a, Result<mediaops_core::DeleteRemoteOutcome, ControlError>>
        {
            self.deletes.lock().expect("lock").push(remote.clone());
            let outcome = self.delete_outcome;
            Box::pin(async move { Ok(outcome) })
        }
        fn grab_apply<'a>(
            &'a self,
            _: &'a [u8],
        ) -> mediaops_core::BoxFuture<'a, Result<mediaops_core::GrabApplyReport, ControlError>>
        {
            Box::pin(async {
                *self.calls.lock().expect("lock") += 1;
                Ok(mediaops_core::GrabApplyReport {
                    noop: true,
                    diff: String::new(),
                })
            })
        }
        fn edge_check(
            &self,
        ) -> mediaops_core::BoxFuture<'_, Result<mediaops_core::EdgeApiReport, ControlError>>
        {
            Box::pin(async {
                Ok(mediaops_core::EdgeApiReport {
                    fingerprint: String::new(),
                    invariant_ok: true,
                    drift: String::new(),
                })
            })
        }
        fn edge_apply<'a>(
            &'a self,
            _: &'a [u8],
        ) -> mediaops_core::BoxFuture<'a, Result<mediaops_core::GrabApplyReport, ControlError>>
        {
            Box::pin(async {
                Ok(mediaops_core::GrabApplyReport {
                    noop: true,
                    diff: String::new(),
                })
            })
        }
        fn key_discovery(
            &self,
        ) -> mediaops_core::BoxFuture<'_, Result<mediaops_core::KeyPresence, ControlError>>
        {
            Box::pin(async { Ok(mediaops_core::KeyPresence::default()) })
        }
        fn guard_preview(
            &self,
        ) -> mediaops_core::BoxFuture<'_, Result<Vec<mediaops_core::GuardPreviewItem>, ControlError>>
        {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn hold_list(
            &self,
        ) -> mediaops_core::BoxFuture<'_, Result<Vec<mediaops_core::HoldLiveItem>, ControlError>>
        {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn hold_reject<'a>(
            &'a self,
            _: &'a mediaops_core::HoldKey,
        ) -> mediaops_core::BoxFuture<'a, Result<(), ControlError>> {
            Box::pin(async { Ok(()) })
        }
        fn wanted_missing(
            &self,
        ) -> mediaops_core::BoxFuture<'_, Result<Vec<TitleId>, ControlError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    #[tokio::test]
    async fn grab_apply_dispatches_through_control_port() {
        let plan = Plan::from_toml_bytes(DS.as_bytes())
            .expect("plan")
            .with_actions(vec![Action::GrabApply]);
        let jobs = MemJobs::new();
        let titles = MemTitles::new();
        let src = Arc::new(MemSource {
            body: b"abcd".to_vec(),
            fail_from: None,
            hits: Mutex::new(Vec::new()),
        });
        let root = scratch("grab-apply");
        let control = FakeControl::new();
        apply(
            &plan,
            DS.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
                control: Some(&control),
            },
        )
        .await
        .expect("apply");
        assert_eq!(*control.calls.lock().expect("lock"), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn failing_unmonitor_is_reported_and_does_not_abort_the_copy() {
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let plan = Plan::from_toml_bytes(DS.as_bytes())
            .expect("plan")
            .with_actions(vec![
                Action::Copy {
                    title_id: title.clone(),
                    remote: RemoteRef::from_wire_parts(
                        "seed".into(),
                        PathBuf::from("The.Matrix.1999.mkv"),
                    )
                    .expect("ref"),
                    file_len: 4,
                    placement: Placement::movie("The.Matrix", 1999, "mkv"),
                },
                Action::Unmonitor {
                    title_id: title.clone(),
                },
            ]);
        let jobs = MemJobs::new();
        let titles = MemTitles::new();
        let src = Arc::new(MemSource {
            body: b"abcd".to_vec(),
            fail_from: None,
            hits: Mutex::new(Vec::new()),
        });
        let root = scratch("unmonitor-fails");
        let control = FakeControl {
            unmonitor_err: true,
            ..FakeControl::new()
        };
        // The caller still needs `installed` back: those copies owe a
        // post-install encode that no later run will hand them.
        let report = apply(
            &plan,
            DS.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
                control: Some(&control),
            },
        )
        .await
        .expect("grabber failure must not abort the run");
        assert_eq!(report.copies, 1);
        assert_eq!(report.installed.len(), 1);
        assert_eq!(
            report.unmonitor_failed,
            vec![UnmonitorFailure {
                title_id: title,
                error: "sonarr 500".into(),
            }]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn unmonitor_dispatches_through_control_port() {
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let plan = Plan::from_toml_bytes(DS.as_bytes())
            .expect("plan")
            .with_actions(vec![Action::Unmonitor {
                title_id: title.clone(),
            }]);
        let jobs = MemJobs::new();
        let titles = MemTitles::new();
        let src = Arc::new(MemSource {
            body: b"abcd".to_vec(),
            fail_from: None,
            hits: Mutex::new(Vec::new()),
        });
        let root = scratch("unmonitor-apply");
        let control = FakeControl::new();
        apply(
            &plan,
            DS.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
                control: Some(&control),
            },
        )
        .await
        .expect("apply");
        assert_eq!(*control.unmonitors.lock().expect("lock"), vec![title]);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn copy_does_not_call_delete_remote() {
        let plan = Plan::from_toml_bytes(DS.as_bytes())
            .expect("plan")
            .with_actions(vec![movie_copy(4)]);
        let jobs = MemJobs::new();
        let titles = MemTitles::new();
        let src = Arc::new(MemSource {
            body: b"abcd".to_vec(),
            fail_from: None,
            hits: Mutex::new(Vec::new()),
        });
        let root = scratch("copy-no-delete");
        let control = FakeControl::new();
        apply(
            &plan,
            DS.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
                control: Some(&control),
            },
        )
        .await
        .expect("copy");
        assert!(
            control.deletes.lock().expect("lock").is_empty(),
            "Copy must not imply DeleteRemote"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn delete_remote_dispatches_and_records_skipped_seeding() {
        let remote = RemoteRef::from_wire_parts(
            "seed".into(),
            PathBuf::from("movies/The.Matrix.(1999)/The.Matrix.(1999).mkv"),
        )
        .expect("ref");
        let plan = Plan::from_toml_bytes(DS.as_bytes())
            .expect("plan")
            .with_actions(vec![Action::DeleteRemote {
                remote: remote.clone(),
            }]);
        let jobs = MemJobs::new();
        let titles = MemTitles::new();
        let src = Arc::new(MemSource {
            body: b"abcd".to_vec(),
            fail_from: None,
            hits: Mutex::new(Vec::new()),
        });
        let root = scratch("delete-remote-apply");
        let control = FakeControl {
            delete_outcome: mediaops_core::DeleteRemoteOutcome::SkippedSeeding,
            ..FakeControl::new()
        };
        let report = apply(
            &plan,
            DS.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
                control: Some(&control),
            },
        )
        .await
        .expect("apply");
        assert_eq!(report.skipped_seeding, 1);
        assert_eq!(report.deleted, 0);
        assert_eq!(control.deletes.lock().expect("lock").as_slice(), &[remote]);
        let _ = fs::remove_dir_all(root);
    }
}
