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
            "SELECT title_id, path, install_b3, current_b3 FROM title_index WHERE title_id = ?1",
            params![key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite)?;
    match row {
        None => Ok(None),
        Some((raw_id, path, install_b3, current_b3)) => {
            let parsed = TitleId::parse(&raw_id)?;
            Ok(Some(TitleIndexEntry::new(
                parsed,
                path,
                Blake3Hex::parse(&install_b3)?,
                Blake3Hex::parse(&current_b3)?,
            )))
        }
    }
}

pub(crate) fn list_titles(conn: &mut Connection) -> Result<Vec<TitleIndexEntry>, StoreError> {
    let mut stmt = conn
        .prepare("SELECT title_id, path, install_b3, current_b3 FROM title_index ORDER BY title_id")
        .map_err(sqlite)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(sqlite)?;
    let mut out = Vec::new();
    for row in rows {
        let (raw_id, path, install_b3, current_b3) = row.map_err(sqlite)?;
        out.push(TitleIndexEntry::new(
            TitleId::parse(&raw_id)?,
            path,
            Blake3Hex::parse(&install_b3)?,
            Blake3Hex::parse(&current_b3)?,
        ));
    }
    Ok(out)
}

pub(crate) fn record_install(
    conn: &Connection,
    title_id: &TitleId,
    digest: &Blake3Hex,
    path: &str,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO title_index (title_id, path, install_b3, current_b3) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(title_id) DO NOTHING",
        params![title_id.render(), path, digest.as_str(), digest.as_str()],
    )
    .map_err(sqlite)?;
    match get_title(conn, title_id)? {
        Some(existing) if existing.install_b3() == digest => {
            if existing.path_missing() && !path.is_empty() {
                conn.execute(
                    "UPDATE title_index SET path = ?1 WHERE title_id = ?2 AND path = ''",
                    params![path, title_id.render()],
                )
                .map_err(sqlite)?;
            }
            Ok(())
        }
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
        let path = "movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv";
        store
            .record_install(&title, &a, path)
            .await
            .expect("install");
        store
            .record_install(&title, &a, path)
            .await
            .expect("idempotent");
        let err = store
            .record_install(&title, &b, path)
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
        TitleIndexRepo::record_install(
            &store,
            &title,
            &a,
            "movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv",
        )
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
            "INSERT INTO title_index (title_id, path, install_b3, current_b3) VALUES (?1, ?2, ?3, ?4)",
            params![title.render(), "", "not-a-digest", "not-a-digest"],
        )
        .expect("insert");
        drop(conn);
        let err = store.get_title(&title).await.expect_err("bad digest");
        assert!(matches!(err, StoreError::Digest(_)), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
