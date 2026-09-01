//! Apply a Plan artifact in the locked CLI process.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mediaops_core::{
    Action, InstallError, Job, JobEvent, JobId, JobKind, JobState, JobsRepo, PathSchemaError,
    Placement, Plan, PlanError, PullEvent, PullState, TitleId, TitleIndexRepo,
    VerifiedStagingHandle, WantEvent, WantState, install, render, staging_path,
};
use mediaops_transfer::{PullSpec, RangeSource, pull_file};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledCopy {
    pub title_id: TitleId,
    pub path: PathBuf,
    pub pull_job_id: JobId,
    pub placement: Placement,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApplyReport {
    pub copies: usize,
    pub skips: usize,
    pub installed: Vec<InstalledCopy>,
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
                let installed = apply_copy(
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
                .await?;
                report.copies += 1;
                report.installed.push(installed);
            }
            Action::Skip { .. } => report.skips += 1,
            Action::Encode { .. }
            | Action::Review
            | Action::Unmonitor
            | Action::DeleteRemote
            | Action::Reclaim
            | Action::EdgeApply
            | Action::GrabApply => {}
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
    let pull = match existing.iter().find(|j| {
        j.kind() == JobKind::Pull && !matches!(j.state(), JobState::Pull(PullState::Installed))
    }) {
        Some(job) => job.clone(),
        None => jobs
            .create(JobKind::Pull, title_id, want.as_ref().map(Job::id))
            .await
            .map_err(|err| ApplyError::Jobs(err.to_string()))?,
    };

    let mut pull = pull;
    if matches!(pull.state(), JobState::Pull(PullState::Queued)) {
        pull = jobs
            .advance(pull.id(), JobEvent::Pull(PullEvent::Start))
            .await
            .map_err(|err| ApplyError::Jobs(err.to_string()))?;
    }

    if matches!(pull.state(), JobState::Pull(PullState::Pulling)) {
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
        titles
            .record_install(title_id, &outcome.whole_file_b3, path_str)
            .await
            .map_err(|err| ApplyError::Titles(err.to_string()))?;
        pull = jobs
            .advance(pull.id(), JobEvent::Pull(PullEvent::Install))
            .await
            .map_err(|err| ApplyError::Jobs(err.to_string()))?;
        if let Some(want) = want {
            let _ = jobs
                .advance(want.id(), JobEvent::Want(WantEvent::Satisfy))
                .await;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{
        Blake3Hex, JobError, RemoteRef, TitleIndexEntry, TitleIndexError, parse_placement,
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

        async fn get(&self, title_id: &TitleId) -> Result<Option<TitleIndexEntry>, Self::Error> {
            Ok(self
                .rows
                .lock()
                .expect("lock")
                .get(&title_id.render())
                .cloned())
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
            let key = title_id.render();
            if let Some(existing) = rows.get(&key) {
                if existing.install_b3() != digest {
                    return Err(TitleIndexError::InstallDigestImmutable);
                }
                return Ok(());
            }
            rows.insert(
                key,
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
            title_id: &TitleId,
            current_b3: &Blake3Hex,
        ) -> Result<(), Self::Error> {
            let mut rows = self.rows.lock().expect("lock");
            let key = title_id.render();
            let existing = rows
                .get(&key)
                .cloned()
                .ok_or(TitleIndexError::NotInstalled)?;
            rows.insert(
                key,
                TitleIndexEntry::new(
                    existing.title_id().clone(),
                    existing.path().to_string(),
                    existing.install_b3().clone(),
                    current_b3.clone(),
                ),
            );
            Ok(())
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
        let rel = "movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv";
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
            title_id: TitleId::movie("603").expect("id"),
            remote: RemoteRef::from_wire_parts(
                "seed".into(),
                PathBuf::from("movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv"),
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
        let err = apply(
            &plan,
            DS.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
            },
        )
        .await
        .expect_err("spaces");
        assert!(
            matches!(
                err,
                ApplyError::PathSchema(PathSchemaError::SpaceRefused(_))
            ),
            "{err}"
        );
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
        let err = apply(
            &plan,
            ds.as_bytes(),
            ApplyCtx {
                jobs: &jobs,
                titles: &titles,
                source: src,
                library_root: &root,
                concurrency: 1,
            },
        )
        .await
        .expect_err("killed");
        assert!(err.to_string().contains("killed"), "{err}");

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
            },
        )
        .await
        .expect("resume");
        assert_eq!(report.copies, 1);
        let installed = &report.installed[0].path;
        assert_eq!(fs::read(installed).expect("read"), body);
        let _ = fs::remove_dir_all(root);
    }
}
