//! sqlite adapter. This story adds `probes` (AD-8). `title_index` / `jobs` wait on deferred 1.3.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mediaops_core::Probe;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(String),
    #[error("join: {0}")]
    Join(String),
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
        self.with(move |conn| {
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
        })
        .await
    }

    pub async fn put_probe(&self, probe: &Probe) -> Result<(), StoreError> {
        if probe.range_concurrency == 0 {
            return Err(StoreError::Sqlite(
                "range_concurrency must be greater than zero".into(),
            ));
        }
        let fingerprint = probe.endpoint_fingerprint.clone();
        let n = i64::from(probe.range_concurrency);
        self.with(move |conn| {
            conn.execute(
                "INSERT INTO probes (endpoint_fingerprint, range_concurrency)
                 VALUES (?1, ?2)
                 ON CONFLICT(endpoint_fingerprint) DO UPDATE SET range_concurrency = excluded.range_concurrency",
                params![fingerprint, n],
            )
            .map_err(|err| StoreError::Sqlite(err.to_string()))?;
            Ok(())
        })
        .await
    }
}

fn open_conn(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open(path).map_err(|err| StoreError::Sqlite(err.to_string()))?;
    conn.busy_timeout(Duration::from_secs(5))
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

fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|err| StoreError::Sqlite(err.to_string()))?;
    if version > 1 {
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
    Ok(())
}

/// Open a throwaway file-backed store for tests.
pub async fn open_file(path: &Path) -> Result<Store, StoreError> {
    Store::open(path).await
}

#[cfg(test)]
mod tests {
    use super::*;

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
        conn.pragma_update(None, "user_version", 2)
            .expect("user_version");
        drop(conn);
        let err = Store::open(&path).await.expect_err("future schema");
        assert!(err.to_string().contains("user_version"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
