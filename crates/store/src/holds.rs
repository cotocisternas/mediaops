use mediaops_core::{HoldDecision, HoldKey, ReleaseId, TitleId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{StoreError, sqlite};

pub(crate) fn get_decision(
    conn: &Connection,
    key: &HoldKey,
) -> Result<Option<HoldDecision>, StoreError> {
    let row = conn
        .query_row(
            "SELECT decision FROM holds_decisions WHERE title_id = ?1 AND release_id = ?2",
            params![key.title_id.render(), key.release_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite)?;
    match row {
        None => Ok(None),
        Some(raw) => Ok(Some(HoldDecision::parse(&raw)?)),
    }
}

pub(crate) fn list_decided(conn: &Connection) -> Result<Vec<HoldKey>, StoreError> {
    let mut stmt = conn
        .prepare("SELECT title_id, release_id FROM holds_decisions ORDER BY title_id, release_id")
        .map_err(sqlite)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(sqlite)?;
    let mut out = Vec::new();
    for row in rows {
        let (title_id, release_id) = row.map_err(sqlite)?;
        out.push(HoldKey::new(
            TitleId::parse(&title_id)?,
            ReleaseId::parse(&release_id)?,
        ));
    }
    Ok(out)
}

pub(crate) fn put_decision(
    conn: &Connection,
    key: &HoldKey,
    decision: HoldDecision,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO holds_decisions (title_id, release_id, decision) VALUES (?1, ?2, ?3)
         ON CONFLICT(title_id, release_id) DO UPDATE SET decision = excluded.decision",
        params![
            key.title_id.render(),
            key.release_id.as_str(),
            decision.as_str()
        ],
    )
    .map_err(sqlite)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{Store, scratch};
    use mediaops_core::{HoldDecision, HoldKey, HoldsRepo, ReleaseId, TitleId};

    fn key() -> HoldKey {
        HoldKey::new(
            TitleId::movie("603").expect("title"),
            ReleaseId::usenet("The.Matrix.1999.nzb").expect("release"),
        )
    }

    #[tokio::test]
    async fn put_get_and_list_decided_round_trip() {
        let dir = scratch("holds-repo");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let key = key();
        assert!(store.get(&key).await.expect("get").is_none());
        store.put(&key, HoldDecision::Rejected).await.expect("put");
        assert_eq!(
            store.get(&key).await.expect("get"),
            Some(HoldDecision::Rejected)
        );
        store
            .put(&key, HoldDecision::Approved)
            .await
            .expect("upsert");
        assert_eq!(
            store.get(&key).await.expect("get"),
            Some(HoldDecision::Approved)
        );
        let listed = store.list_decided().await.expect("list");
        assert_eq!(listed, vec![key]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
