use mediaops_core::{Blake3Hex, TitleId, TitleIndexEntry, TitleIndexError};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{StoreError, sqlite};

pub(crate) fn get_title(
    conn: &Connection,
    title_id: &TitleId,
) -> Result<Option<TitleIndexEntry>, StoreError> {
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
        .map_err(sqlite)?;
    match row {
        None => Ok(None),
        Some((raw_id, install_b3, current_b3)) => {
            let parsed = TitleId::parse(&raw_id)?;
            Ok(Some(TitleIndexEntry::new(
                parsed,
                Blake3Hex::parse(&install_b3)?,
                Blake3Hex::parse(&current_b3)?,
            )))
        }
    }
}

pub(crate) fn record_install(
    conn: &Connection,
    title_id: &TitleId,
    digest: &Blake3Hex,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO title_index (title_id, install_b3, current_b3) VALUES (?1, ?2, ?3)
         ON CONFLICT(title_id) DO NOTHING",
        params![title_id.render(), digest.as_str(), digest.as_str()],
    )
    .map_err(sqlite)?;
    match get_title(conn, title_id)? {
        Some(existing) if existing.install_b3() == digest => Ok(()),
        Some(_) => Err(StoreError::TitleIndex(
            TitleIndexError::InstallDigestImmutable,
        )),
        None => Err(StoreError::Sqlite(
            "title_index insert did not produce a row".into(),
        )),
    }
}

pub(crate) fn record_replace(
    conn: &Connection,
    title_id: &TitleId,
    current_b3: &Blake3Hex,
) -> Result<(), StoreError> {
    let n = conn
        .execute(
            "UPDATE title_index SET current_b3 = ?1 WHERE title_id = ?2",
            params![current_b3.as_str(), title_id.render()],
        )
        .map_err(sqlite)?;
    if n == 0 {
        return Err(StoreError::TitleIndex(TitleIndexError::NotInstalled));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Store, scratch};
    use mediaops_core::TitleIndexRepo;

    fn digest(fill: char) -> Blake3Hex {
        Blake3Hex::parse(&fill.to_string().repeat(64)).expect("digest")
    }

    #[tokio::test]
    async fn title_index_install_b3_is_immutable_and_replace_updates_current() {
        let dir = scratch("title-index");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let title = TitleId::movie("603").expect("title");
        let a = digest('a');
        let b = digest('b');
        assert!(store.get_title(&title).await.expect("get").is_none());
        store.record_install(&title, &a).await.expect("install");
        store.record_install(&title, &a).await.expect("idempotent");
        let err = store
            .record_install(&title, &b)
            .await
            .expect_err("immutable");
        assert!(err.to_string().contains("immutable"), "{err}");
        store.record_replace(&title, &b).await.expect("replace");
        let entry = store.get_title(&title).await.expect("get").expect("row");
        assert_eq!(entry.install_b3(), &a);
        assert_eq!(entry.current_b3(), &b);
        let other = TitleId::movie("604").expect("other");
        let err = store.record_replace(&other, &b).await.expect_err("missing");
        assert!(err.to_string().contains("no title_index row"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn title_index_repo_trait_is_implemented() {
        let dir = scratch("title-trait");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let title = TitleId::movie("603").expect("title");
        let a = digest('a');
        TitleIndexRepo::record_install(&store, &title, &a)
            .await
            .expect("put");
        let entry = TitleIndexRepo::get(&store, &title)
            .await
            .expect("get")
            .expect("row");
        assert_eq!(entry.install_b3(), &a);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn corrupt_digest_is_not_a_sqlite_error() {
        let dir = scratch("bad-digest");
        let path = dir.join("state.db");
        let store = Store::open(&path).await.expect("open");
        let title = TitleId::movie("603").expect("title");
        let conn = rusqlite::Connection::open(&path).expect("raw");
        conn.execute(
            "INSERT INTO title_index (title_id, install_b3, current_b3) VALUES (?1, ?2, ?3)",
            params![title.render(), "not-a-digest", "not-a-digest"],
        )
        .expect("insert");
        drop(conn);
        let err = store.get_title(&title).await.expect_err("bad digest");
        assert!(matches!(err, StoreError::Digest(_)), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
