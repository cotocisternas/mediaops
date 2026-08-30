//! sqlite adapter (AD-8). Schema `user_version` 2: `probes` (v1) plus
//! `title_index` / `jobs`. `holds_decisions` waits on Epic 6.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mediaops_core::{
    Job, JobError, JobEvent, JobId, JobKind, JobState, JobsRepo, Probe, ProbeRepo, TitleId,
    TitleIndexEntry, TitleIndexError, TitleIndexRepo, advance,
};
use rusqlite::{Connection, OptionalExtension, params};

const SCHEMA_VERSION: i64 = 2;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(String),
    #[error("join: {0}")]
    Join(String),
    #[error(transparent)]
    Job(#[from] JobError),
    #[error(transparent)]
    TitleIndex(#[from] TitleIndexError),
    #[error("job {0} not found")]
    JobNotFound(JobId),
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
        F: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_conn(&path)?;
            f(&conn)
        })
        .await
        .map_err(|err| StoreError::Join(err.to_string()))?
    }

    pub async fn get_probe(&self, fingerprint: &str) -> Result<Option<Probe>, StoreError> {
        let fingerprint = fingerprint.to_string();
        self.with(move |conn| get_probe(conn, &fingerprint)).await
    }

    pub async fn put_probe(&self, probe: &Probe) -> Result<(), StoreError> {
        if probe.range_concurrency == 0 {
            return Err(StoreError::Sqlite(
                "range_concurrency must be greater than zero".into(),
            ));
        }
        let fingerprint = probe.endpoint_fingerprint.clone();
        let n = i64::from(probe.range_concurrency);
        self.with(move |conn| put_probe(conn, &fingerprint, n)).await
    }

    pub async fn get_title(
        &self,
        title_id: &TitleId,
    ) -> Result<Option<TitleIndexEntry>, StoreError> {
        let title_id = title_id.clone();
        self.with(move |conn| get_title(conn, &title_id)).await
    }

    pub async fn record_install(&self, title_id: &TitleId, digest: &str) -> Result<(), StoreError> {
        let title_id = title_id.clone();
        let digest = digest.to_string();
        self.with(move |conn| record_install(conn, &title_id, &digest))
            .await
    }

    pub async fn record_replace(
        &self,
        title_id: &TitleId,
        current_b3: &str,
    ) -> Result<(), StoreError> {
        let title_id = title_id.clone();
        let current_b3 = current_b3.to_string();
        self.with(move |conn| record_replace(conn, &title_id, &current_b3))
            .await
    }

    pub async fn get_job(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        self.with(move |conn| get_job(conn, id)).await
    }

    pub async fn create_job(
        &self,
        kind: JobKind,
        parent_job_id: Option<JobId>,
    ) -> Result<Job, StoreError> {
        self.with(move |conn| create_job(conn, kind, parent_job_id))
            .await
    }

    pub async fn advance(&self, id: JobId, event: JobEvent) -> Result<Job, StoreError> {
        self.with(move |conn| advance_job(conn, id, event)).await
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

    async fn record_install(&self, title_id: &TitleId, digest: &str) -> Result<(), StoreError> {
        Store::record_install(self, title_id, digest).await
    }

    async fn record_replace(&self, title_id: &TitleId, current_b3: &str) -> Result<(), StoreError> {
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
        parent_job_id: Option<JobId>,
    ) -> Result<Job, StoreError> {
        Store::create_job(self, kind, parent_job_id).await
    }

    async fn advance(&self, id: JobId, event: JobEvent) -> Result<Job, StoreError> {
        Store::advance(self, id, event).await
    }
}

fn open_conn(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open(path).map_err(|err| StoreError::Sqlite(err.to_string()))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|err| StoreError::Sqlite(err.to_string()))?;
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(|err| StoreError::Sqlite(err.to_string()))?;
    Ok(conn)
}

fn range_concurrency_from_i64(n: i64) -> Result<u32, StoreError> {
    let n = u32::try_from(n)
        .map_err(|_| StoreError::Sqlite(format!("range_concurrency {n} is out of range")))?;
    if n == 0 {
        return Err(StoreError::Sqlite(
            "range_concurrency must be greater than zero".into(),
        ));
    }
    Ok(n)
}

fn user_version(conn: &Connection) -> Result<i64, StoreError> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|err| StoreError::Sqlite(err.to_string()))
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
        .map_err(|err| StoreError::Sqlite(err.to_string()))?;
    }
    if version < 2 {
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
        .map_err(|err| StoreError::Sqlite(err.to_string()))?;
    }
    Ok(())
}

fn get_probe(conn: &Connection, fingerprint: &str) -> Result<Option<Probe>, StoreError> {
    let row = conn
        .query_row(
            "SELECT endpoint_fingerprint, range_concurrency FROM probes WHERE endpoint_fingerprint = ?1",
            params![fingerprint],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|err| StoreError::Sqlite(err.to_string()))?;
    match row {
        None => Ok(None),
        Some((endpoint_fingerprint, n)) => Ok(Some(Probe {
            endpoint_fingerprint,
            range_concurrency: range_concurrency_from_i64(n)?,
        })),
    }
}

fn put_probe(conn: &Connection, fingerprint: &str, n: i64) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO probes (endpoint_fingerprint, range_concurrency)
         VALUES (?1, ?2)
         ON CONFLICT(endpoint_fingerprint) DO UPDATE SET range_concurrency = excluded.range_concurrency",
        params![fingerprint, n],
    )
    .map_err(|err| StoreError::Sqlite(err.to_string()))?;
    Ok(())
}

fn get_title(conn: &Connection, title_id: &TitleId) -> Result<Option<TitleIndexEntry>, StoreError> {
    let key = title_id.render();
    let row = conn
        .query_row(
            "SELECT title_id, install_b3, current_b3 FROM title_index WHERE title_id = ?1",
            params![key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|err| StoreError::Sqlite(err.to_string()))?;
    match row {
        None => Ok(None),
        Some((raw_id, install_b3, current_b3)) => {
            let parsed = TitleId::parse(&raw_id)
                .map_err(|err| StoreError::Sqlite(err.to_string()))?;
            Ok(Some(TitleIndexEntry::new(parsed, install_b3, current_b3)?))
        }
    }
}

fn record_install(conn: &Connection, title_id: &TitleId, digest: &str) -> Result<(), StoreError> {
    let entry = TitleIndexEntry::new(title_id.clone(), digest, digest)?;
    match get_title(conn, title_id)? {
        None => {
            conn.execute(
                "INSERT INTO title_index (title_id, install_b3, current_b3) VALUES (?1, ?2, ?3)",
                params![
                    entry.title_id().render(),
                    entry.install_b3(),
                    entry.current_b3()
                ],
            )
            .map_err(|err| StoreError::Sqlite(err.to_string()))?;
            Ok(())
        }
        Some(existing) if existing.install_b3() == digest => Ok(()),
        Some(_) => Err(StoreError::TitleIndex(
            TitleIndexError::InstallDigestImmutable,
        )),
    }
}

fn record_replace(conn: &Connection, title_id: &TitleId, current_b3: &str) -> Result<(), StoreError> {
    let _ = TitleIndexEntry::new(title_id.clone(), current_b3, current_b3)?;
    let Some(existing) = get_title(conn, title_id)? else {
        return Err(StoreError::TitleIndex(TitleIndexError::NotInstalled));
    };
    conn.execute(
        "UPDATE title_index SET current_b3 = ?1 WHERE title_id = ?2",
        params![current_b3, existing.title_id().render()],
    )
    .map_err(|err| StoreError::Sqlite(err.to_string()))?;
    Ok(())
}

fn get_job(conn: &Connection, id: JobId) -> Result<Option<Job>, StoreError> {
    let row = conn
        .query_row(
            "SELECT id, kind, state, parent_job_id FROM jobs WHERE id = ?1",
            params![id.get()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|err| StoreError::Sqlite(err.to_string()))?;
    match row {
        None => Ok(None),
        Some((raw_id, kind, state, parent)) => Ok(Some(job_from_row(raw_id, &kind, &state, parent)?)),
    }
}

fn job_from_row(
    raw_id: i64,
    kind: &str,
    state: &str,
    parent: Option<i64>,
) -> Result<Job, StoreError> {
    let id = JobId::new(raw_id)?;
    let kind = JobKind::parse(kind)?;
    let state = JobState::parse(kind, state)?;
    let parent_job_id = match parent {
        None => None,
        Some(raw) => Some(JobId::new(raw)?),
    };
    Ok(Job::new(id, state, parent_job_id)?)
}

fn create_job(
    conn: &Connection,
    kind: JobKind,
    parent_job_id: Option<JobId>,
) -> Result<Job, StoreError> {
    let state = JobState::initial(kind);
    let parent = parent_job_id.map(JobId::get);
    conn.execute(
        "INSERT INTO jobs (kind, state, parent_job_id) VALUES (?1, ?2, ?3)",
        params![kind.as_str(), state.as_str(), parent],
    )
    .map_err(|err| StoreError::Sqlite(err.to_string()))?;
    let id = JobId::new(conn.last_insert_rowid())?;
    Ok(Job::new(id, state, parent_job_id)?)
}

fn advance_job(conn: &Connection, id: JobId, event: JobEvent) -> Result<Job, StoreError> {
    let job = get_job(conn, id)?.ok_or(StoreError::JobNotFound(id))?;
    let next = advance(&job.state(), event)?;
    conn.execute(
        "UPDATE jobs SET kind = ?1, state = ?2 WHERE id = ?3",
        params![next.kind().as_str(), next.as_str(), id.get()],
    )
    .map_err(|err| StoreError::Sqlite(err.to_string()))?;
    Ok(Job::new(id, next, job.parent_job_id())?)
}

/// Open a throwaway file-backed store for tests.
pub async fn open_file(path: &Path) -> Result<Store, StoreError> {
    Store::open(path).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{EncodeEvent, EncodeState, PullEvent, PullState, WantState, encode_ready};

    const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn scratch(tag: &str) -> PathBuf {
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

    #[tokio::test]
    async fn probes_round_trip_and_fingerprint_is_the_key() {
        let dir = scratch("round-trip");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let probe = Probe {
            endpoint_fingerprint: "abc".into(),
            range_concurrency: 4,
        };
        assert!(store.get_probe("abc").await.expect("get").is_none());
        store.put_probe(&probe).await.expect("put");
        assert_eq!(
            store.get_probe("abc").await.expect("get").as_ref(),
            Some(&probe)
        );
        store
            .put_probe(&Probe {
                endpoint_fingerprint: "abc".into(),
                range_concurrency: 8,
            })
            .await
            .expect("update");
        assert_eq!(
            store
                .get_probe("abc")
                .await
                .expect("get")
                .unwrap()
                .range_concurrency,
            8
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn range_concurrency_zero_is_rejected() {
        let dir = scratch("zero");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let err = store
            .put_probe(&Probe {
                endpoint_fingerprint: "abc".into(),
                range_concurrency: 0,
            })
            .await
            .expect_err("zero");
        assert!(err.to_string().contains("range_concurrency"));
        let conn = Connection::open(dir.join("state.db")).expect("raw");
        conn.execute(
            "INSERT INTO probes (endpoint_fingerprint, range_concurrency) VALUES ('abc', 0)",
            [],
        )
        .expect("insert 0");
        drop(conn);
        let err = store.get_probe("abc").await.expect_err("get zero");
        assert!(err.to_string().contains("range_concurrency"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn future_schema_user_version_is_an_error() {
        let dir = scratch("future");
        let path = dir.join("state.db");
        let conn = Connection::open(&path).expect("raw");
        conn.pragma_update(None, "user_version", 3)
            .expect("user_version");
        drop(conn);
        let err = Store::open(&path).await.expect_err("future schema");
        assert!(err.to_string().contains("user_version"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn v1_probes_migrate_to_v2_and_keep_working() {
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
        assert_eq!(version, 2);
        conn.query_row("SELECT COUNT(*) FROM title_index", [], |row| row.get::<_, i64>(0))
            .expect("title_index exists");
        conn.query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get::<_, i64>(0))
            .expect("jobs exists");
        drop(conn);

        let title = TitleId::movie("603").expect("title");
        store.record_install(&title, DIGEST_A).await.expect("install");
        let job = store
            .create_job(JobKind::Want, None)
            .await
            .expect("create");
        assert_eq!(job.state(), JobState::Want(WantState::Open));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn title_index_install_b3_is_immutable_and_replace_updates_current() {
        let dir = scratch("title-index");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let title = TitleId::movie("603").expect("title");
        assert!(store.get_title(&title).await.expect("get").is_none());
        store.record_install(&title, DIGEST_A).await.expect("install");
        store.record_install(&title, DIGEST_A).await.expect("idempotent");
        let err = store
            .record_install(&title, DIGEST_B)
            .await
            .expect_err("immutable");
        assert!(err.to_string().contains("immutable"), "{err}");
        store.record_replace(&title, DIGEST_B).await.expect("replace");
        let entry = store.get_title(&title).await.expect("get").expect("row");
        assert_eq!(entry.install_b3(), DIGEST_A);
        assert_eq!(entry.current_b3(), DIGEST_B);
        let other = TitleId::movie("604").expect("other");
        let err = store
            .record_replace(&other, DIGEST_B)
            .await
            .expect_err("missing");
        assert!(err.to_string().contains("no title_index row"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn jobs_advance_is_the_sole_state_write_and_illegal_is_a_repo_error() {
        let dir = scratch("jobs");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let want = store
            .create_job(JobKind::Want, None)
            .await
            .expect("want");
        let pull = store
            .create_job(JobKind::Pull, Some(want.id()))
            .await
            .expect("pull");
        assert_eq!(pull.parent_job_id(), Some(want.id()));
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
            .create_job(JobKind::Encode, Some(pull.id()))
            .await
            .expect("encode");
        let parent = store.get_job(pull.id()).await.expect("get").expect("parent");
        assert!(encode_ready(&encode, Some(&parent)));
        store
            .advance(encode.id(), JobEvent::Encode(EncodeEvent::Start))
            .await
            .expect("enc start");
        let started = store.get_job(encode.id()).await.expect("get").expect("row");
        assert_eq!(started.state(), JobState::Encode(EncodeState::Encoding));
        assert!(!encode_ready(&started, Some(&parent)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn missing_parent_job_is_a_foreign_key_error() {
        let dir = scratch("fk");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let dangling = JobId::new(99).expect("id");
        let err = store
            .create_job(JobKind::Pull, Some(dangling))
            .await
            .expect_err("fk");
        assert!(err.to_string().contains("sqlite"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn probe_repo_trait_is_implemented() {
        let dir = scratch("trait");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        ProbeRepo::put_probe(
            &store,
            &Probe {
                endpoint_fingerprint: "fp".into(),
                range_concurrency: 2,
            },
        )
        .await
        .expect("put");
        assert_eq!(
            ProbeRepo::get_probe(&store, "fp")
                .await
                .expect("get")
                .expect("row")
                .range_concurrency,
            2
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
