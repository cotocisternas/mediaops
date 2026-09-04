use mediaops_core::{Blake3Hex, TitleId, TitleIndexEntry, TitleIndexError, rewrite_absolute_under};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{StoreError, sqlite};

type Row = (String, String, String, String);

fn entry_from_row(
    (raw_id, path, install_b3, current_b3): Row,
) -> Result<TitleIndexEntry, StoreError> {
    Ok(TitleIndexEntry::new(
        TitleId::parse(&raw_id)?,
        path,
        Blake3Hex::parse(&install_b3)?,
        Blake3Hex::parse(&current_b3)?,
    ))
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok((
        row.get::<_, String>(0)?,
        row.get::<_, String>(1)?,
        row.get::<_, String>(2)?,
        row.get::<_, String>(3)?,
    ))
}

/// Every row for a title (a movie has one; a show or album has one per file).
pub(crate) fn get_title(
    conn: &Connection,
    title_id: &TitleId,
) -> Result<Vec<TitleIndexEntry>, StoreError> {
    let mut stmt = conn
        .prepare(
            "SELECT title_id, path, install_b3, current_b3 FROM title_index
             WHERE title_id = ?1 ORDER BY path",
        )
        .map_err(sqlite)?;
    let rows = stmt
        .query_map(params![title_id.render()], map_row)
        .map_err(sqlite)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(entry_from_row(row.map_err(sqlite)?)?);
    }
    Ok(out)
}

pub(crate) fn get_path(
    conn: &Connection,
    path: &str,
) -> Result<Option<TitleIndexEntry>, StoreError> {
    if path.is_empty() {
        return Ok(None);
    }
    let row = conn
        .query_row(
            "SELECT title_id, path, install_b3, current_b3 FROM title_index WHERE path = ?1",
            params![path],
            map_row,
        )
        .optional()
        .map_err(sqlite)?;
    row.map(entry_from_row).transpose()
}

pub(crate) fn list_titles(conn: &mut Connection) -> Result<Vec<TitleIndexEntry>, StoreError> {
    let mut stmt = conn
        .prepare(
            "SELECT title_id, path, install_b3, current_b3 FROM title_index
             ORDER BY title_id, path",
        )
        .map_err(sqlite)?;
    let rows = stmt.query_map([], map_row).map_err(sqlite)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(entry_from_row(row.map_err(sqlite)?)?);
    }
    Ok(out)
}

/// First install of `path`. Idempotent for the same digest; a different
/// digest for the same path is `InstallDigestImmutable`. A pre-v5 row for this
/// title with an empty path and the same digest is backfilled instead.
pub(crate) fn record_install(
    conn: &Connection,
    title_id: &TitleId,
    digest: &Blake3Hex,
    path: &str,
) -> Result<(), StoreError> {
    if path.is_empty() {
        return Err(StoreError::Sqlite(
            "title_index path must not be empty".into(),
        ));
    }
    if let Some(existing) = get_path(conn, path)? {
        if existing.install_b3() == digest {
            return Ok(());
        }
        return Err(StoreError::TitleIndex(
            TitleIndexError::InstallDigestImmutable,
        ));
    }
    let backfilled = conn
        .execute(
            "UPDATE title_index SET path = ?1
             WHERE title_id = ?2 AND path = '' AND install_b3 = ?3",
            params![path, title_id.render(), digest.as_str()],
        )
        .map_err(sqlite)?;
    if backfilled > 0 {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO title_index (title_id, path, install_b3, current_b3) VALUES (?1, ?2, ?3, ?4)",
        params![title_id.render(), path, digest.as_str(), digest.as_str()],
    )
    .map_err(sqlite)?;
    Ok(())
}

pub(crate) fn record_replace(
    conn: &Connection,
    path: &str,
    current_b3: &Blake3Hex,
) -> Result<(), StoreError> {
    let n = conn
        .execute(
            "UPDATE title_index SET current_b3 = ?1 WHERE path = ?2 AND path <> ''",
            params![current_b3.as_str(), path],
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
            "UPDATE title_index SET path = ?1 WHERE path = ?2",
            params![new_path, row.path()],
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

    fn digest(fill: char) -> Blake3Hex {
        Blake3Hex::parse(&fill.to_string().repeat(64)).expect("digest")
    }

    const MATRIX: &str = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";

    #[tokio::test]
    async fn title_index_install_b3_is_immutable_and_replace_updates_current() {
        let dir = scratch("title-index");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let title = TitleId::movie_key("The.Matrix", 1999).expect("title");
        let a = digest('a');
        let b = digest('b');
        assert!(store.get_title(&title).await.expect("get").is_empty());
        store
            .record_install(&title, &a, MATRIX)
            .await
            .expect("install");
        store
            .record_install(&title, &a, MATRIX)
            .await
            .expect("idempotent");
        let err = store
            .record_install(&title, &b, MATRIX)
            .await
            .expect_err("immutable");
        assert!(err.to_string().contains("immutable"), "{err}");
        let row = store.get_path(MATRIX).await.expect("get").expect("row");
        assert_eq!(row.install_b3(), &a);
        assert_eq!(row.current_b3(), &a);
        assert_eq!(row.path(), MATRIX);

        store.record_replace(MATRIX, &b).await.expect("replace");
        let rows = store.get_title(&title).await.expect("get");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].install_b3(), &a);
        assert_eq!(rows[0].current_b3(), &b);
        let listed = store.list_titles().await.expect("list");
        assert_eq!(listed, rows);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_show_is_one_row_per_episode_sharing_one_title_id() {
        let dir = scratch("title-index-episodes");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let show = TitleId::series_key("Silo", 2023).expect("title");
        let e1 = "series/Silo.(2023)/Season.01/Silo.(2023).S01E01.mkv";
        let e2 = "series/Silo.(2023)/Season.01/Silo.(2023).S01E02.mkv";
        store
            .record_install(&show, &digest('1'), e1)
            .await
            .expect("e1");
        store
            .record_install(&show, &digest('2'), e2)
            .await
            .expect("e2");
        let rows = store.get_title(&show).await.expect("get");
        assert_eq!(rows.len(), 2, "two episodes are two rows");
        assert_eq!(rows[0].path(), e1);
        assert_eq!(rows[1].path(), e2);
        assert_eq!(rows[1].install_b3(), &digest('2'));
        // Replacing one episode leaves the other's digest alone.
        store
            .record_replace(e2, &digest('9'))
            .await
            .expect("replace");
        let rows = store.get_title(&show).await.expect("get");
        assert_eq!(rows[0].current_b3(), &digest('1'));
        assert_eq!(rows[1].current_b3(), &digest('9'));
        assert!(
            store
                .record_replace(
                    "series/Nope.(2000)/Season.01/Nope.(2000).S01E01.mkv",
                    &digest('e')
                )
                .await
                .is_err(),
            "replace of an unindexed path is NotInstalled"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn replace_without_install_is_not_installed() {
        let dir = scratch("title-index-replace");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let err = store
            .record_replace(MATRIX, &digest('b'))
            .await
            .expect_err("no row");
        assert!(err.to_string().contains("no title_index row"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn import_rows_keeps_distinct_digests_and_refuses_non_empty() {
        let dir = scratch("title-index-import");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let title = TitleId::movie_key("The.Matrix", 1999).expect("title");
        let rows = vec![TitleIndexEntry::new(
            title.clone(),
            MATRIX,
            digest('a'),
            digest('b'),
        )];
        store.import_rows(&rows).await.expect("import");
        let back = store.get_path(MATRIX).await.expect("get").expect("row");
        assert_eq!(back.install_b3(), &digest('a'));
        assert_eq!(back.current_b3(), &digest('b'));
        let err = store.import_rows(&rows).await.expect_err("non-empty");
        assert!(err.to_string().contains("not empty"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn rewrite_absolute_prefix_only_touches_absolute_rows_under_old_root() {
        let dir = scratch("title-index-rewrite");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let rel_title = TitleId::movie_key("The.Matrix", 1999).expect("rel");
        let abs_title = TitleId::movie_key("Coco", 2017).expect("abs");
        let other_title = TitleId::movie_key("Up", 2009).expect("other");
        store
            .record_install(&rel_title, &digest('a'), MATRIX)
            .await
            .expect("rel");
        store
            .record_install(
                &abs_title,
                &digest('b'),
                "/data/old/movies/Coco.(2017)/Coco.(2017).mkv",
            )
            .await
            .expect("abs");
        store
            .record_install(
                &other_title,
                &digest('c'),
                "/elsewhere/movies/Up.(2009)/Up.(2009).mkv",
            )
            .await
            .expect("other");
        let n = store
            .rewrite_absolute_prefix("/data/old", "/data/new")
            .await
            .expect("rewrite");
        assert_eq!(n, 1);
        let rows = store.list_titles().await.expect("list");
        let paths: Vec<&str> = rows.iter().map(TitleIndexEntry::path).collect();
        assert!(paths.contains(&MATRIX));
        assert!(paths.contains(&"/data/new/movies/Coco.(2017)/Coco.(2017).mkv"));
        assert!(paths.contains(&"/elsewhere/movies/Up.(2009)/Up.(2009).mkv"));
        assert_eq!(
            store
                .rewrite_absolute_prefix("", "/x")
                .await
                .expect("empty old"),
            0
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn pre_v5_empty_path_row_is_backfilled_by_matching_install() {
        let dir = scratch("title-index-backfill");
        let store = Store::open(dir.join("state.db")).await.expect("open");
        let title = TitleId::movie_key("The.Matrix", 1999).expect("title");
        // Simulate a pre-v5 row: empty path.
        store
            .import_rows(&[TitleIndexEntry::new(
                title.clone(),
                "",
                digest('a'),
                digest('a'),
            )])
            .await
            .expect("import");
        assert!(store.get_title(&title).await.expect("get")[0].path_missing());
        store
            .record_install(&title, &digest('a'), MATRIX)
            .await
            .expect("backfill");
        let rows = store.get_title(&title).await.expect("get");
        assert_eq!(rows.len(), 1, "backfilled, not duplicated");
        assert_eq!(rows[0].path(), MATRIX);
        // A different digest for the same title is a new file, not a clash.
        let other = "movies/The.Matrix.(1999)/The.Matrix.(1999).Remastered.mkv";
        store
            .record_install(&title, &digest('f'), other)
            .await
            .expect("second file");
        assert_eq!(store.get_title(&title).await.expect("get").len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
