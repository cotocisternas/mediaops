//! sqlite adapter. This story adds `probes` (AD-8). `title_index` / `jobs` wait on deferred 1.3.

use std::path::{Path, PathBuf};

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
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| StoreError::Sqlite(err.to_string()))?;
        }
        let store = Self { path };
        store.with(|conn| migrate(conn))?;
        Ok(store)
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T, StoreError>) -> Result<T, StoreError> {
        let conn = Connection::open(&self.path).map_err(|err| StoreError::Sqlite(err.to_string()))?;
        f(&conn)
    }

    pub fn get_probe(&self, fingerprint: &str) -> Result<Option<Probe>, StoreError> {
        let fingerprint = fingerprint.to_string();
        self.with(move |conn| {
            conn.query_row(
                "SELECT endpoint_fingerprint, range_concurrency FROM probes WHERE endpoint_fingerprint = ?1",
                params![fingerprint],
                |row| {
                    Ok(Probe {
                        endpoint_fingerprint: row.get(0)?,
                        range_concurrency: row.get::<_, i64>(1)? as u32,
                    })
                },
            )
            .optional()
            .map_err(|err| StoreError::Sqlite(err.to_string()))
        })
    }

    pub fn put_probe(&self, probe: &Probe) -> Result<(), StoreError> {
        let fingerprint = probe.endpoint_fingerprint.clone();
        let n = probe.range_concurrency as i64;
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
    }
}

fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|err| StoreError::Sqlite(err.to_string()))?;
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
pub fn open_file(path: &Path) -> Result<Store, StoreError> {
    Store::open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_round_trip_and_fingerprint_is_the_key() {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Store::open(dir.join("state.db")).expect("open");
        let probe = Probe {
            endpoint_fingerprint: "abc".into(),
            range_concurrency: 4,
        };
        assert!(store.get_probe("abc").expect("get").is_none());
        store.put_probe(&probe).expect("put");
        assert_eq!(store.get_probe("abc").expect("get").as_ref(), Some(&probe));
        store
            .put_probe(&Probe {
                endpoint_fingerprint: "abc".into(),
                range_concurrency: 8,
            })
            .expect("update");
        assert_eq!(
            store.get_probe("abc").expect("get").unwrap().range_concurrency,
            8
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
