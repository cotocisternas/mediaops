//! sqlite adapter (AD-8). Schema `user_version` 3: `probes` (v1), `title_index` /
//! `jobs` (v2) with `jobs.title_id` (v3). `holds_decisions` waits on Epic 6.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mediaops_core::{
    Blake3Hex, Job, JobEvent, JobId, JobKind, JobsRepo, Probe, ProbeRepo, TitleId, TitleIndexEntry,
    TitleIndexRepo,
};
use rusqlite::Connection;

mod jobs;
mod probes;
mod title_index;

const SCHEMA_VERSION: i64 = 3;

const JOBS_DDL: &str = "CREATE TABLE jobs (
                id INTEGER PRIMARY KEY NOT NULL,
                title_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                state TEXT NOT NULL,
                parent_job_id INTEGER,
                FOREIGN KEY (parent_job_id) REFERENCES jobs(id)
            );";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(String),
    #[error("join: {0}")]
    Join(String),
    #[error(transparent)]
    Job(#[from] mediaops_core::JobError),
    #[error(transparent)]
    TitleIndex(#[from] mediaops_core::TitleIndexError),
    #[error(transparent)]
    Digest(#[from] mediaops_core::DigestError),
    #[error(transparent)]
    TitleId(#[from] mediaops_core::TitleIdError),
    #[error("job {0} not found")]
    JobNotFound(JobId),
    #[error("job {0} state changed during advance")]
    JobConflict(JobId),
}

#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        tokio::task::spawn_blocking(move || {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| StoreError::Sqlite(err.to_string()))?;
            }
            let conn = open_conn(&path)?;
            migrate(&conn)?;
            Ok(Self { path })
        })
        .await
        .map_err(|err| StoreError::Join(err.to_string()))?
    }

    async fn with<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_conn(&path)?;
            f(&mut conn)
        })
        .await
        .map_err(|err| StoreError::Join(err.to_string()))?
    }

    pub async fn get_probe(&self, fingerprint: &str) -> Result<Option<Probe>, StoreError> {
        let fingerprint = fingerprint.to_string();
        self.with(move |conn| probes::get_probe(conn, &fingerprint))
            .await
    }

    pub async fn put_probe(&self, probe: &Probe) -> Result<(), StoreError> {
        probes::reject_zero_concurrency(probe.range_concurrency)?;
        let fingerprint = probe.endpoint_fingerprint.clone();
        let n = i64::from(probe.range_concurrency);
        self.with(move |conn| probes::put_probe(conn, &fingerprint, n))
            .await
    }

    pub async fn get_title(
        &self,
        title_id: &TitleId,
    ) -> Result<Option<TitleIndexEntry>, StoreError> {
        let title_id = title_id.clone();
        self.with(move |conn| title_index::get_title(conn, &title_id))
            .await
    }

    pub async fn record_install(
        &self,
        title_id: &TitleId,
        digest: &Blake3Hex,
    ) -> Result<(), StoreError> {
        let title_id = title_id.clone();
        let digest = digest.clone();
        self.with(move |conn| title_index::record_install(conn, &title_id, &digest))
            .await
    }

    pub async fn record_replace(
        &self,
        title_id: &TitleId,
        current_b3: &Blake3Hex,
    ) -> Result<(), StoreError> {
        let title_id = title_id.clone();
        let current_b3 = current_b3.clone();
        self.with(move |conn| title_index::record_replace(conn, &title_id, &current_b3))
            .await
    }

    pub async fn get_job(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        self.with(move |conn| jobs::get_job(conn, id)).await
    }

    pub async fn create_job(
        &self,
        kind: JobKind,
        title_id: &TitleId,
        parent_job_id: Option<JobId>,
    ) -> Result<Job, StoreError> {
        let title_id = title_id.clone();
        self.with(move |conn| jobs::create_job(conn, kind, title_id, parent_job_id))
            .await
    }

    pub async fn advance(&self, id: JobId, event: JobEvent) -> Result<Job, StoreError> {
        self.with(move |conn| jobs::advance_job(conn, id, event))
            .await
    }
}

impl ProbeRepo for Store {
    type Error = StoreError;

    async fn get_probe(&self, fingerprint: &str) -> Result<Option<Probe>, StoreError> {
        Store::get_probe(self, fingerprint).await
    }

    async fn put_probe(&self, probe: &Probe) -> Result<(), StoreError> {
        Store::put_probe(self, probe).await
    }
}

impl TitleIndexRepo for Store {
    type Error = StoreError;

    async fn get(&self, title_id: &TitleId) -> Result<Option<TitleIndexEntry>, StoreError> {
        Store::get_title(self, title_id).await
    }

    async fn record_install(
        &self,
        title_id: &TitleId,
        digest: &Blake3Hex,
    ) -> Result<(), StoreError> {
        Store::record_install(self, title_id, digest).await
    }

    async fn record_replace(
        &self,
        title_id: &TitleId,
        current_b3: &Blake3Hex,
    ) -> Result<(), StoreError> {
        Store::record_replace(self, title_id, current_b3).await
    }
}

impl JobsRepo for Store {
    type Error = StoreError;

    async fn get(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        Store::get_job(self, id).await
    }

    async fn create(
        &self,
        kind: JobKind,
        title_id: &TitleId,
        parent_job_id: Option<JobId>,
    ) -> Result<Job, StoreError> {
        Store::create_job(self, kind, title_id, parent_job_id).await
    }

    async fn advance(&self, id: JobId, event: JobEvent) -> Result<Job, StoreError> {
        Store::advance(self, id, event).await
    }
}

fn open_conn(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open(path).map_err(sqlite)?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(sqlite)?;
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(sqlite)?;
    Ok(conn)
}

pub(crate) fn sqlite(err: impl ToString) -> StoreError {
    StoreError::Sqlite(err.to_string())
}

fn user_version(conn: &Connection) -> Result<i64, StoreError> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sqlite)
}

fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let version = user_version(conn)?;
    if version > SCHEMA_VERSION {
        return Err(StoreError::Sqlite(format!(
            "unsupported schema user_version {version}"
        )));
    }
    if version < 1 {
        conn.execute_batch(
            "CREATE TABLE probes (
                endpoint_fingerprint TEXT PRIMARY KEY NOT NULL,
                range_concurrency INTEGER NOT NULL
            );
            PRAGMA user_version = 1;",
        )
        .map_err(sqlite)?;
    }
    if version < 2 {
        conn.execute_batch(&format!(
            "CREATE TABLE title_index (
                title_id TEXT PRIMARY KEY NOT NULL,
                install_b3 TEXT NOT NULL,
                current_b3 TEXT NOT NULL
            );
            {JOBS_DDL}
            PRAGMA user_version = 2;"
        ))
        .map_err(sqlite)?;
    }
    if version < 3 {
        ensure_jobs_title_id(conn)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(sqlite)?;
    }
    Ok(())
}

fn jobs_has_title_id(conn: &Connection) -> Result<bool, StoreError> {
    let mut stmt = conn.prepare("PRAGMA table_info(jobs)").map_err(sqlite)?;
    let cols = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sqlite)?;
    for col in cols {
        if col.map_err(sqlite)? == "title_id" {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_jobs_title_id(conn: &Connection) -> Result<(), StoreError> {
    if jobs_has_title_id(conn)? {
        return Ok(());
    }
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))
        .map_err(sqlite)?;
    if n > 0 {
        return Err(StoreError::Sqlite(
            "cannot migrate jobs rows that have no title_id".into(),
        ));
    }
    conn.execute_batch(&format!("DROP TABLE jobs; {JOBS_DDL}"))
        .map_err(sqlite)
}

#[cfg(test)]
pub(crate) fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mediaops-store-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{JobKind, TitleId, WantState};

    fn write_v1(path: &Path) {
        let conn = Connection::open(path).expect("raw");
        conn.execute_batch(
            "CREATE TABLE probes (
                endpoint_fingerprint TEXT PRIMARY KEY NOT NULL,
                range_concurrency INTEGER NOT NULL
            );
            INSERT INTO probes (endpoint_fingerprint, range_concurrency) VALUES ('abc', 4);
            PRAGMA user_version = 1;",
        )
        .expect("v1");
    }

    fn write_v2_anonymous_jobs(path: &Path) {
        write_v1(path);
        let conn = Connection::open(path).expect("raw");
        conn.execute_batch(
            "CREATE TABLE title_index (
                title_id TEXT PRIMARY KEY NOT NULL,
                install_b3 TEXT NOT NULL,
                current_b3 TEXT NOT NULL
            );
            CREATE TABLE jobs (
                id INTEGER PRIMARY KEY NOT NULL,
                kind TEXT NOT NULL,
                state TEXT NOT NULL,
                parent_job_id INTEGER,
                FOREIGN KEY (parent_job_id) REFERENCES jobs(id)
            );
            PRAGMA user_version = 2;",
        )
        .expect("v2");
    }

    #[tokio::test]
    async fn future_schema_user_version_is_an_error() {
        let dir = scratch("future");
        let path = dir.join("state.db");
        let conn = Connection::open(&path).expect("raw");
        conn.pragma_update(None, "user_version", 4)
            .expect("user_version");
        drop(conn);
        let err = Store::open(&path).await.expect_err("future schema");
        assert!(err.to_string().contains("user_version"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn v1_probes_migrate_to_v3_and_keep_working() {
        let dir = scratch("v1-migrate");
        let path = dir.join("state.db");
        write_v1(&path);
        let store = Store::open(&path).await.expect("open");
        let probe = store.get_probe("abc").await.expect("get").expect("row");
        assert_eq!(probe.range_concurrency, 4);

        let conn = Connection::open(&path).expect("raw");
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, 3);
        conn.query_row("SELECT COUNT(*) FROM title_index", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("title_index exists");
        conn.prepare("SELECT title_id FROM jobs")
            .expect("jobs.title_id exists");
        drop(conn);

        let title = TitleId::movie("603").expect("title");
        store
            .record_install(&title, &Blake3Hex::parse(&"a".repeat(64)).expect("d"))
            .await
            .expect("install");
        let job = store
            .create_job(JobKind::Want, &title, None)
            .await
            .expect("create");
        assert_eq!(job.state(), mediaops_core::JobState::Want(WantState::Open));
        assert_eq!(job.title_id(), &title);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn v2_anonymous_jobs_table_is_rebuilt_when_empty() {
        let dir = scratch("v2-repair");
        let path = dir.join("state.db");
        write_v2_anonymous_jobs(&path);
        let store = Store::open(&path).await.expect("open");
        let title = TitleId::movie("603").expect("title");
        let job = store
            .create_job(JobKind::Want, &title, None)
            .await
            .expect("create");
        assert_eq!(job.title_id(), &title);
        let conn = Connection::open(&path).expect("raw");
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, 3);
        drop(conn);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn v2_anonymous_jobs_with_rows_cannot_be_migrated() {
        let dir = scratch("v2-blocked");
        let path = dir.join("state.db");
        write_v2_anonymous_jobs(&path);
        let conn = Connection::open(&path).expect("raw");
        conn.execute(
            "INSERT INTO jobs (kind, state, parent_job_id) VALUES ('want', 'open', NULL)",
            [],
        )
        .expect("row");
        drop(conn);
        let err = Store::open(&path).await.expect_err("blocked");
        assert!(err.to_string().contains("no title_id"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
