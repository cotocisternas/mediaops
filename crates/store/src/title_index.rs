use mediaops_core::{Blake3Hex, TitleId, TitleIndexEntry, TitleIndexError, rewrite_absolute_under};
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

pub(crate) fn import_rows(
    conn: &mut Connection,
    rows: &[TitleIndexEntry],
) -> Result<(), StoreError> {
    let tx = conn.transaction().map_err(sqlite)?;
    let n: i64 = tx
        .query_row("SELECT COUNT(*) FROM title_index", [], |row| row.get(0))
        .map_err(sqlite)?;
    if n > 0 {
        return Err(StoreError::TitleIndex(TitleIndexError::NotEmpty));
    }
    for row in rows {
        tx.execute(
            "INSERT INTO title_index (title_id, path, install_b3, current_b3) VALUES (?1, ?2, ?3, ?4)",
            params![
                row.title_id().render(),
                row.path(),
                row.install_b3().as_str(),
                row.current_b3().as_str(),
            ],
        )
        .map_err(sqlite)?;
    }
    tx.commit().map_err(sqlite)?;
    Ok(())
}

pub(crate) fn rewrite_absolute_prefix(
    conn: &mut Connection,
    old_root: &str,
    new_root: &str,
) -> Result<u64, StoreError> {
    if old_root.is_empty() || old_root == new_root {
        return Ok(0);
    }
    let rows = list_titles(conn)?;
    let tx = conn.transaction().map_err(sqlite)?;
    let mut rewritten = 0_u64;
    for row in rows {
        let Some(new_path) = rewrite_absolute_under(row.path(), old_root, new_root) else {
            continue;
        };
        tx.execute(
            "UPDATE title_index SET path = ?1 WHERE title_id = ?2",
            params![new_path, row.title_id().render()],
        )
        .map_err(sqlite)?;
        rewritten += 1;
    }
    tx.commit().map_err(sqlite)?;
    Ok(rewritten)
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

    #[tokio::test]
    async fn import_rows_keeps_distinct_digests_and_refuses_non_empty() {
        let dir = scratch("title-import");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let title = TitleId::movie("603").expect("title");
        let a = digest('a');
        let b = digest('b');
        let path = "movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv";
        let row = TitleIndexEntry::new(title.clone(), path, a.clone(), b.clone());
        store.import_rows(&[row.clone()]).await.expect("import");
        let entry = store.get_title(&title).await.expect("get").expect("row");
        assert_eq!(entry.install_b3(), &a);
        assert_eq!(entry.current_b3(), &b);
        assert_eq!(entry.path(), path);
        let err = store
            .import_rows(std::slice::from_ref(&row))
            .await
            .expect_err("non-empty");
        assert!(
            matches!(err, StoreError::TitleIndex(TitleIndexError::NotEmpty)),
            "{err}"
        );
        let listed = store.list_titles().await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].current_b3(), &b);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn rewrite_absolute_prefix_skips_relative_rows() {
        let dir = scratch("title-rewrite");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let rel = TitleId::movie("603").expect("rel");
        let abs = TitleId::movie("604").expect("abs");
        let other = TitleId::movie("605").expect("other");
        let a = digest('a');
        let rel_path = "movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv";
        store
            .import_rows(&[
                TitleIndexEntry::new(rel.clone(), rel_path, a.clone(), a.clone()),
                TitleIndexEntry::new(
                    abs.clone(),
                    "/data/old/movies/Other.(1999).{tmdb-604}/Other.(1999).mkv",
                    a.clone(),
                    digest('b'),
                ),
                TitleIndexEntry::new(
                    other.clone(),
                    "/other/movies/Skip.(1999).{tmdb-605}/Skip.(1999).mkv",
                    a.clone(),
                    a.clone(),
                ),
            ])
            .await
            .expect("import");
        let n = store
            .rewrite_absolute_prefix("/data/old", "/mnt/new")
            .await
            .expect("rewrite");
        assert_eq!(n, 1);
        assert_eq!(
            store
                .get_title(&rel)
                .await
                .expect("rel")
                .expect("row")
                .path(),
            rel_path
        );
        assert_eq!(
            store
                .get_title(&abs)
                .await
                .expect("abs")
                .expect("row")
                .path(),
            "/mnt/new/movies/Other.(1999).{tmdb-604}/Other.(1999).mkv"
        );
        assert_eq!(
            store
                .get_title(&other)
                .await
                .expect("other")
                .expect("row")
                .path(),
            "/other/movies/Skip.(1999).{tmdb-605}/Skip.(1999).mkv"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
