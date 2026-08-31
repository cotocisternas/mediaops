use rusqlite::Connection;

use crate::{StoreError, sqlite};

pub(crate) fn get(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    let mut stmt = conn
        .prepare("SELECT value FROM machine WHERE key = ?1")
        .map_err(sqlite)?;
    let mut rows = stmt.query(rusqlite::params![key]).map_err(sqlite)?;
    match rows.next().map_err(sqlite)? {
        Some(row) => Ok(Some(row.get(0).map_err(sqlite)?)),
        None => Ok(None),
    }
}

pub(crate) fn put(conn: &Connection, key: &str, value: &str) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO machine (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )
    .map_err(sqlite)?;
    Ok(())
}
