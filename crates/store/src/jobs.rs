use mediaops_core::{Job, JobEvent, JobId, JobKind, JobState, TitleId};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{StoreError, sqlite};

pub(crate) fn list_jobs(conn: &mut Connection) -> Result<Vec<Job>, StoreError> {
    jobs_query(
        conn,
        "SELECT id, title_id, kind, state, parent_job_id FROM jobs ORDER BY id",
        None,
    )
}

pub(crate) fn list_jobs_by_title(
    conn: &mut Connection,
    title_id: &TitleId,
) -> Result<Vec<Job>, StoreError> {
    jobs_query(
        conn,
        "SELECT id, title_id, kind, state, parent_job_id FROM jobs WHERE title_id = ?1 ORDER BY id",
        Some(title_id.render()),
    )
}

fn jobs_query(
    conn: &Connection,
    sql: &str,
    title_id: Option<String>,
) -> Result<Vec<Job>, StoreError> {
    // `&Connection` is enough for SELECT; callers pass `&mut` from `Store::with`.
    let mut stmt = conn.prepare(sql).map_err(sqlite)?;
    let mapped = match title_id {
        Some(key) => {
            let rows = stmt.query_map(params![key], job_row).map_err(sqlite)?;
            collect_jobs(rows)?
        }
        None => {
            let rows = stmt.query_map([], job_row).map_err(sqlite)?;
            collect_jobs(rows)?
        }
    };
    Ok(mapped)
}

fn job_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(i64, String, String, String, Option<i64>)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

fn collect_jobs(
    rows: impl Iterator<Item = rusqlite::Result<(i64, String, String, String, Option<i64>)>>,
) -> Result<Vec<Job>, StoreError> {
    let mut out = Vec::new();
    for row in rows {
        let (raw_id, title_id, kind, state, parent) = row.map_err(sqlite)?;
        out.push(job_from_row(raw_id, &title_id, &kind, &state, parent)?);
    }
    Ok(out)
}

pub(crate) fn get_job(conn: &Connection, id: JobId) -> Result<Option<Job>, StoreError> {
    let row = conn
        .query_row(
            "SELECT id, title_id, kind, state, parent_job_id FROM jobs WHERE id = ?1",
            params![id.get()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite)?;
    match row {
        None => Ok(None),
        Some((raw_id, title_id, kind, state, parent)) => Ok(Some(job_from_row(
            raw_id, &title_id, &kind, &state, parent,
        )?)),
    }
}

fn job_from_row(
    raw_id: i64,
    title_id: &str,
    kind: &str,
    state: &str,
    parent: Option<i64>,
) -> Result<Job, StoreError> {
    let id = JobId::new(raw_id)?;
    let title_id = TitleId::parse(title_id)?;
    let kind = JobKind::parse(kind)?;
    let state = JobState::parse(kind, state)?;
    let parent_job_id = match parent {
        None => None,
        Some(raw) => Some(JobId::new(raw)?),
    };
    Ok(Job::new(id, title_id, state, parent_job_id)?)
}

fn parent_kind_matches(
    conn: &Connection,
    parent: JobId,
    expected: JobKind,
) -> Result<(), StoreError> {
    let job = get_job(conn, parent)?.ok_or(StoreError::JobNotFound(parent))?;
    if job.kind() != expected {
        return Err(mediaops_core::JobError::ParentKindMismatch {
            parent,
            expected,
            actual: job.kind(),
        }
        .into());
    }
    Ok(())
}

fn validate_parent(
    conn: &Connection,
    kind: JobKind,
    parent_job_id: Option<JobId>,
) -> Result<(), StoreError> {
    match kind {
        JobKind::Want | JobKind::Hold => {
            if parent_job_id.is_some() {
                return Err(mediaops_core::JobError::UnexpectedParent { kind }.into());
            }
            Ok(())
        }
        JobKind::Encode => match parent_job_id {
            None => Ok(()),
            Some(pid) => parent_kind_matches(conn, pid, JobKind::Pull),
        },
        JobKind::Pull => match parent_job_id {
            None => Ok(()),
            Some(pid) => parent_kind_matches(conn, pid, JobKind::Want),
        },
    }
}

pub(crate) fn create_job(
    conn: &mut Connection,
    kind: JobKind,
    title_id: TitleId,
    parent_job_id: Option<JobId>,
) -> Result<Job, StoreError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite)?;
    validate_parent(&tx, kind, parent_job_id)?;
    let state = JobState::initial(kind);
    tx.execute(
        "INSERT INTO jobs (title_id, kind, state, parent_job_id) VALUES (?1, ?2, ?3, ?4)",
        params![
            title_id.render(),
            kind.as_str(),
            state.as_str(),
            parent_job_id.map(JobId::get)
        ],
    )
    .map_err(sqlite)?;
    let id = JobId::new(tx.last_insert_rowid())?;
    let job = Job::new(id, title_id, state, parent_job_id)?;
    tx.commit().map_err(sqlite)?;
    Ok(job)
}

pub(crate) fn advance_job(
    conn: &mut Connection,
    id: JobId,
    event: JobEvent,
) -> Result<Job, StoreError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite)?;
    let job = get_job(&tx, id)?.ok_or(StoreError::JobNotFound(id))?;
    let next = job.advance(event)?;
    let n = tx
        .execute(
            "UPDATE jobs SET state = ?1 WHERE id = ?2 AND state = ?3",
            params![next.state().as_str(), id.get(), job.state().as_str()],
        )
        .map_err(sqlite)?;
    if n == 0 {
        return Err(StoreError::JobConflict(id));
    }
    tx.commit().map_err(sqlite)?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Store, scratch};
    use mediaops_core::{
        EncodeEvent, EncodeState, JobsRepo, PullEvent, PullState, WantState, encode_ready,
    };

    fn title() -> TitleId {
        TitleId::movie("603").expect("title")
    }

    #[tokio::test]
    async fn jobs_advance_is_the_sole_state_write_and_illegal_is_a_repo_error() {
        let dir = scratch("jobs");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let title = title();
        let want = store
            .create_job(JobKind::Want, &title, None)
            .await
            .expect("want");
        let pull = store
            .create_job(JobKind::Pull, &title, Some(want.id()))
            .await
            .expect("pull");
        assert_eq!(pull.parent_job_id(), Some(want.id()));
        assert_eq!(pull.title_id(), &title);
        assert_eq!(pull.state(), JobState::Pull(PullState::Queued));

        let err = store
            .advance(pull.id(), JobEvent::Pull(PullEvent::Install))
            .await
            .expect_err("illegal");
        assert!(err.to_string().contains("illegal"), "{err}");
        let still = store.get_job(pull.id()).await.expect("get").expect("row");
        assert_eq!(still.state(), JobState::Pull(PullState::Queued));

        let pulling = store
            .advance(pull.id(), JobEvent::Pull(PullEvent::Start))
            .await
            .expect("start");
        assert_eq!(pulling.state(), JobState::Pull(PullState::Pulling));
        store
            .advance(pull.id(), JobEvent::Pull(PullEvent::FinishRanges))
            .await
            .expect("ranges");
        let installed = store
            .advance(pull.id(), JobEvent::Pull(PullEvent::Install))
            .await
            .expect("install");
        assert_eq!(installed.state(), JobState::Pull(PullState::Installed));

        let encode = store
            .create_job(JobKind::Encode, &title, Some(pull.id()))
            .await
            .expect("encode");
        let parent = store
            .get_job(pull.id())
            .await
            .expect("get")
            .expect("parent");
        assert!(encode_ready(&encode, Some(&parent), false));
        store
            .advance(encode.id(), JobEvent::Encode(EncodeEvent::Start))
            .await
            .expect("enc start");
        let started = store.get_job(encode.id()).await.expect("get").expect("row");
        assert_eq!(started.state(), JobState::Encode(EncodeState::Encoding));
        assert!(!encode_ready(&started, Some(&parent), false));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn missing_parent_job_is_not_found() {
        let dir = scratch("fk");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let dangling = JobId::new(99).expect("id");
        let err = store
            .create_job(JobKind::Pull, &title(), Some(dangling))
            .await
            .expect_err("missing parent");
        assert!(matches!(err, StoreError::JobNotFound(_)), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn encode_parent_must_be_a_pull_job() {
        let dir = scratch("parent-kind");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let title = title();
        let want = store
            .create_job(JobKind::Want, &title, None)
            .await
            .expect("want");
        let err = store
            .create_job(JobKind::Encode, &title, Some(want.id()))
            .await
            .expect_err("want is not pull");
        assert!(err.to_string().contains("expected pull"), "{err}");
        let local = store
            .create_job(JobKind::Encode, &title, None)
            .await
            .expect("already-local encode");
        assert_eq!(local.parent_job_id(), None);
        assert!(encode_ready(&local, None, true));
        assert!(!encode_ready(&local, None, false));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn jobs_repo_trait_is_implemented() {
        let dir = scratch("jobs-trait");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let title = title();
        let want = JobsRepo::create(&store, JobKind::Want, &title, None)
            .await
            .expect("create");
        let got = JobsRepo::get(&store, want.id())
            .await
            .expect("get")
            .expect("row");
        assert_eq!(got.state(), JobState::Want(WantState::Open));
        let listed = JobsRepo::list(&store).await.expect("list");
        assert_eq!(listed.len(), 1);
        let by_title = JobsRepo::list_by_title(&store, &title)
            .await
            .expect("list title");
        assert_eq!(by_title.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn corrupt_job_title_id_is_not_a_sqlite_error() {
        let dir = scratch("bad-job-title");
        let path = dir.join("state.db");
        let store = Store::open(&path).await.expect("open");
        let conn = rusqlite::Connection::open(&path).expect("raw");
        conn.execute(
            "INSERT INTO jobs (title_id, kind, state, parent_job_id) VALUES ('nope', 'want', 'open', NULL)",
            [],
        )
        .expect("insert");
        let id = JobId::new(conn.last_insert_rowid()).expect("id");
        drop(conn);
        let err = store.get_job(id).await.expect_err("bad title");
        assert!(matches!(err, StoreError::TitleId(_)), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
