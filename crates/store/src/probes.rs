use mediaops_core::Probe;
use rusqlite::{Connection, OptionalExtension, params};

use crate::{StoreError, sqlite};

pub(crate) fn reject_zero_concurrency(n: u32) -> Result<(), StoreError> {
    if n == 0 {
        return Err(StoreError::Sqlite(
            "range_concurrency must be greater than zero".into(),
        ));
    }
    Ok(())
}

fn range_concurrency_from_i64(n: i64) -> Result<u32, StoreError> {
    let n = u32::try_from(n)
        .map_err(|_| StoreError::Sqlite(format!("range_concurrency {n} is out of range")))?;
    reject_zero_concurrency(n)?;
    Ok(n)
}

pub(crate) fn get_probe(conn: &Connection, fingerprint: &str) -> Result<Option<Probe>, StoreError> {
    let row = conn
        .query_row(
            "SELECT endpoint_fingerprint, range_concurrency FROM probes WHERE endpoint_fingerprint = ?1",
            params![fingerprint],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(sqlite)?;
    match row {
        None => Ok(None),
        Some((endpoint_fingerprint, n)) => Ok(Some(Probe {
            endpoint_fingerprint,
            range_concurrency: range_concurrency_from_i64(n)?,
        })),
    }
}

pub(crate) fn put_probe(conn: &Connection, fingerprint: &str, n: i64) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO probes (endpoint_fingerprint, range_concurrency)
         VALUES (?1, ?2)
         ON CONFLICT(endpoint_fingerprint) DO UPDATE SET range_concurrency = excluded.range_concurrency",
        params![fingerprint, n],
    )
    .map_err(sqlite)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{Store, scratch};
    use mediaops_core::{Probe, ProbeRepo};

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
        let conn = rusqlite::Connection::open(dir.join("state.db")).expect("raw");
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
