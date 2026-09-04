//! sqlite adapter (AD-8). Schema `user_version` 6: `probes` (v1), `title_index` /
//! `jobs` (v2) with `jobs.title_id` (v3), `machine` kv (v4), `title_index.path`
//! (v5), `holds_decisions` (v6).

use std::path::{Path, PathBuf};
use std::time::Duration;

use mediaops_core::{
    Blake3Hex, HoldDecision, HoldKey, HoldsRepo, Job, JobEvent, JobId, JobKind, JobsRepo, Probe,
    ProbeRepo, TitleId, TitleIndexEntry, TitleIndexRepo,
};
use rusqlite::Connection;

mod holds;
mod jobs;
mod machine;
mod probes;
mod title_index;

const SCHEMA_VERSION: i64 = 6;

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
    #[error(transparent)]
    Hold(#[from] mediaops_core::HoldError),
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

    pub async fn list_titles(&self) -> Result<Vec<TitleIndexEntry>, StoreError> {
        self.with(|conn| title_index::list_titles(conn)).await
    }

    pub async fn record_install(
        &self,
        title_id: &TitleId,
        digest: &Blake3Hex,
        path: &str,
    ) -> Result<(), StoreError> {
        let title_id = title_id.clone();
        let digest = digest.clone();
        let path = path.to_string();
        self.with(move |conn| title_index::record_install(conn, &title_id, &digest, &path))
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

    pub async fn import_rows(&self, rows: &[TitleIndexEntry]) -> Result<(), StoreError> {
        let rows = rows.to_vec();
        self.with(move |conn| title_index::import_rows(conn, &rows))
            .await
    }

    pub async fn rewrite_absolute_prefix(
        &self,
        old_root: &str,
        new_root: &str,
    ) -> Result<u64, StoreError> {
        let old_root = old_root.to_string();
        let new_root = new_root.to_string();
        self.with(move |conn| title_index::rewrite_absolute_prefix(conn, &old_root, &new_root))
            .await
    }

    pub async fn get_job(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        self.with(move |conn| jobs::get_job(conn, id)).await
    }

    pub async fn list_jobs(&self) -> Result<Vec<Job>, StoreError> {
        self.with(|conn| jobs::list_jobs(conn)).await
    }

    pub async fn list_jobs_by_title(&self, title_id: &TitleId) -> Result<Vec<Job>, StoreError> {
        let title_id = title_id.clone();
        self.with(move |conn| jobs::list_jobs_by_title(conn, &title_id))
            .await
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

    pub async fn get_machine(&self, key: &str) -> Result<Option<String>, StoreError> {
        let key = key.to_string();
        self.with(move |conn| machine::get(conn, &key)).await
    }

    pub async fn put_machine(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let key = key.to_string();
        let value = value.to_string();
        self.with(move |conn| machine::put(conn, &key, &value))
            .await
    }

    pub async fn get_hold(&self, key: &HoldKey) -> Result<Option<HoldDecision>, StoreError> {
        let key = key.clone();
        self.with(move |conn| holds::get_decision(conn, &key)).await
    }

    pub async fn list_decided(&self) -> Result<Vec<HoldKey>, StoreError> {
        self.with(|conn| holds::list_decided(conn)).await
    }

    pub async fn put_hold(&self, key: &HoldKey, decision: HoldDecision) -> Result<(), StoreError> {
        let key = key.clone();
        self.with(move |conn| holds::put_decision(conn, &key, decision))
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

    async fn list(&self) -> Result<Vec<TitleIndexEntry>, StoreError> {
        Store::list_titles(self).await
    }

    async fn record_install(
        &self,
        title_id: &TitleId,
        digest: &Blake3Hex,
        path: &str,
    ) -> Result<(), StoreError> {
        Store::record_install(self, title_id, digest, path).await
    }

    async fn record_replace(
        &self,
        title_id: &TitleId,
        current_b3: &Blake3Hex,
    ) -> Result<(), StoreError> {
        Store::record_replace(self, title_id, current_b3).await
    }

    async fn import_rows(&self, rows: &[TitleIndexEntry]) -> Result<(), StoreError> {
        Store::import_rows(self, rows).await
    }

    async fn rewrite_absolute_prefix(
        &self,
        old_root: &str,
        new_root: &str,
    ) -> Result<u64, StoreError> {
        Store::rewrite_absolute_prefix(self, old_root, new_root).await
    }
}

impl JobsRepo for Store {
    type Error = StoreError;

    async fn get(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        Store::get_job(self, id).await
    }

    async fn list(&self) -> Result<Vec<Job>, StoreError> {
        Store::list_jobs(self).await
    }

    async fn list_by_title(&self, title_id: &TitleId) -> Result<Vec<Job>, StoreError> {
        Store::list_jobs_by_title(self, title_id).await
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

impl HoldsRepo for Store {
    type Error = StoreError;

    async fn get(&self, key: &HoldKey) -> Result<Option<HoldDecision>, StoreError> {
        Store::get_hold(self, key).await
    }

    async fn list_decided(&self) -> Result<Vec<HoldKey>, StoreError> {
        Store::list_decided(self).await
    }

    async fn put(&self, key: &HoldKey, decision: HoldDecision) -> Result<(), StoreError> {
        Store::put_hold(self, key, decision).await
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
        conn.pragma_update(None, "user_version", 3)
            .map_err(sqlite)?;
    }
    if version < 4 {
        conn.execute_batch(
            "CREATE TABLE machine (
                key TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );
            PRAGMA user_version = 4;",
        )
        .map_err(sqlite)?;
    }
    if version < 5 {
        ensure_title_index_path(conn)?;
        conn.pragma_update(None, "user_version", 5)
            .map_err(sqlite)?;
    }
    if version < 6 {
        conn.execute_batch(
            "CREATE TABLE holds_decisions (
                title_id TEXT NOT NULL,
                release_id TEXT NOT NULL,
                decision TEXT NOT NULL,
                PRIMARY KEY (title_id, release_id)
            );
            PRAGMA user_version = 6;",
        )
        .map_err(sqlite)?;
    }
    Ok(())
}

fn title_index_has_path(conn: &Connection) -> Result<bool, StoreError> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(title_index)")
        .map_err(sqlite)?;
    let cols = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sqlite)?;
    for col in cols {
        if col.map_err(sqlite)? == "path" {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_title_index_path(conn: &Connection) -> Result<(), StoreError> {
    if title_index_has_path(conn)? {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE title_index ADD COLUMN path TEXT NOT NULL DEFAULT '';")
        .map_err(sqlite)
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
        conn.pragma_update(None, "user_version", 99)
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
        assert_eq!(version, 6);
        conn.query_row("SELECT COUNT(*) FROM title_index", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("title_index exists");
        conn.query_row("SELECT COUNT(*) FROM machine", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("machine exists");
        conn.prepare("SELECT title_id FROM jobs")
            .expect("jobs.title_id exists");
        conn.prepare("SELECT path FROM title_index")
            .expect("title_index.path exists");
        conn.prepare("SELECT title_id, release_id, decision FROM holds_decisions")
            .expect("holds_decisions exists");
        drop(conn);

        let title = TitleId::movie("603").expect("title");
        store
            .record_install(
                &title,
                &Blake3Hex::parse(&"a".repeat(64)).expect("d"),
                "movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv",
            )
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
        assert_eq!(version, 6);
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

    #[tokio::test]
    async fn machine_kv_round_trips() {
        let dir = scratch("machine");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        assert!(
            store
                .get_machine("library_root")
                .await
                .expect("get")
                .is_none()
        );
        store
            .put_machine("library_root", "/data/media")
            .await
            .expect("put");
        assert_eq!(
            store
                .get_machine("library_root")
                .await
                .expect("get")
                .as_deref(),
            Some("/data/media")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn write_v4_title_without_path(path: &Path) {
        let conn = Connection::open(path).expect("raw");
        conn.execute_batch(
            "CREATE TABLE probes (
                endpoint_fingerprint TEXT PRIMARY KEY NOT NULL,
                range_concurrency INTEGER NOT NULL
            );
            CREATE TABLE title_index (
                title_id TEXT PRIMARY KEY NOT NULL,
                install_b3 TEXT NOT NULL,
                current_b3 TEXT NOT NULL
            );
            CREATE TABLE jobs (
                id INTEGER PRIMARY KEY NOT NULL,
                title_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                state TEXT NOT NULL,
                parent_job_id INTEGER,
                FOREIGN KEY (parent_job_id) REFERENCES jobs(id)
            );
            CREATE TABLE machine (
                key TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );
            PRAGMA user_version = 4;",
        )
        .expect("v4");
        let title = TitleId::movie("603").expect("title");
        let digest = "a".repeat(64);
        conn.execute(
            "INSERT INTO title_index (title_id, install_b3, current_b3) VALUES (?1, ?2, ?3)",
            rusqlite::params![title.render(), digest, digest],
        )
        .expect("row");
    }

    #[tokio::test]
    async fn v4_title_index_row_migrates_empty_path_and_record_install_backfills() {
        let dir = scratch("v4-path");
        let path = dir.join("state.db");
        write_v4_title_without_path(&path);
        let store = Store::open(&path).await.expect("open");
        let title = TitleId::movie("603").expect("title");
        let entry = store.get_title(&title).await.expect("get").expect("row");
        assert!(entry.path_missing(), "v4 row must migrate to empty path");
        let digest = Blake3Hex::parse(&"a".repeat(64)).expect("d");
        let schema = "movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv";
        store
            .record_install(&title, &digest, schema)
            .await
            .expect("backfill");
        let entry = store.get_title(&title).await.expect("get").expect("row");
        assert_eq!(entry.path(), schema);
        let listed = store.list_titles().await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path(), schema);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn write_v5(path: &Path) {
        let conn = Connection::open(path).expect("raw");
        conn.execute_batch(
            "CREATE TABLE probes (
                endpoint_fingerprint TEXT PRIMARY KEY NOT NULL,
                range_concurrency INTEGER NOT NULL
            );
            CREATE TABLE title_index (
                title_id TEXT PRIMARY KEY NOT NULL,
                path TEXT NOT NULL DEFAULT '',
                install_b3 TEXT NOT NULL,
                current_b3 TEXT NOT NULL
            );
            CREATE TABLE jobs (
                id INTEGER PRIMARY KEY NOT NULL,
                title_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                state TEXT NOT NULL,
                parent_job_id INTEGER,
                FOREIGN KEY (parent_job_id) REFERENCES jobs(id)
            );
            CREATE TABLE machine (
                key TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );
            PRAGMA user_version = 5;",
        )
        .expect("v5");
    }

    #[tokio::test]
    async fn v5_migrates_holds_decisions_keyed_by_title_and_release() {
        let dir = scratch("v5-holds");
        let path = dir.join("state.db");
        write_v5(&path);
        let store = Store::open(&path).await.expect("open");
        let conn = Connection::open(&path).expect("raw");
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, 6);
        let info: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='holds_decisions'",
                [],
                |row| row.get(0),
            )
            .expect("table");
        assert!(
            info.contains("PRIMARY KEY")
                && info.contains("title_id")
                && info.contains("release_id"),
            "{info}"
        );
        drop(conn);
        let key = mediaops_core::HoldKey::new(
            TitleId::movie("603").expect("title"),
            mediaops_core::ReleaseId::torrent("deadbeef").expect("release"),
        );
        store
            .put_hold(&key, mediaops_core::HoldDecision::Rejected)
            .await
            .expect("put");
        assert_eq!(store.list_decided().await.expect("list"), vec![key]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn create_want_is_idempotent_for_one_open_row() {
        let dir = scratch("want-unique");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let title = TitleId::movie("603").expect("title");
        let a = store
            .create_job(JobKind::Want, &title, None)
            .await
            .expect("first");
        let b = store
            .create_job(JobKind::Want, &title, None)
            .await
            .expect("second");
        assert_eq!(a.id(), b.id());
        let jobs = store.list_jobs_by_title(&title).await.expect("list");
        let opens = jobs
            .iter()
            .filter(|j| matches!(j.state(), mediaops_core::JobState::Want(WantState::Open)))
            .count();
        assert_eq!(opens, 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
