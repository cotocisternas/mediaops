//! Home API object store. Separate file (`api.db`), not `state.db`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mediaops_core::{
    CLUSTER_NAME, ClusterSpec, ClusterStatus, EventStatus, HoldSpec, HoldStatus, HomeError,
    HomeObject, JobSpec, JobStatus, Kind, NodeSpec, NodeStatus, ObjectMeta, RemoteFileStatus,
    SECRET_NAME, SecretSpec, Spec, StatusBody, TitleSpec, TitleStatus, WantSpec, WantStatus,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{StoreError, sqlite};

const API_SCHEMA_VERSION: i64 = 3;
const WATCH_HISTORY_LIMIT: i64 = 4096;

/// sqlite owner of Home objects. Only `mediaops-api` opens this.
#[derive(Debug, Clone)]
pub struct ApiStore {
    path: PathBuf,
    _owner: std::sync::Arc<std::fs::File>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchType {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreEvent {
    pub watch: WatchType,
    pub object: HomeObject,
}

impl ApiStore {
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        tokio::task::spawn_blocking(move || {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| StoreError::Sqlite(err.to_string()))?;
            }
            use std::os::unix::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
                .map_err(|e| StoreError::Sqlite(e.to_string()))?;
            use std::os::unix::fs::PermissionsExt;
            if !file
                .metadata()
                .map_err(|e| StoreError::Sqlite(e.to_string()))?
                .is_file()
            {
                return Err(StoreError::Sqlite("api.db must be a regular file".into()));
            }
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| StoreError::Sqlite(e.to_string()))?;
            file.try_lock()
                .map_err(|e| StoreError::Sqlite(format!("api.db is already owned: {e}")))?;
            let mut conn = open_conn(&path)?;
            migrate(&mut conn)?;
            Ok(Self {
                path,
                _owner: std::sync::Arc::new(file),
            })
        })
        .await
        .map_err(|err| StoreError::Join(err.to_string()))?
    }

    async fn with<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_conn(&path)?;
            f(&mut conn)
        })
        .await
        .map_err(|err| StoreError::Join(err.to_string()))?
    }

    pub async fn get(&self, kind: Kind, name: &str) -> Result<Option<HomeObject>, StoreError> {
        let name = name.to_string();
        self.with(move |conn| get_object(conn, kind, &name)).await
    }

    pub async fn list(&self, kind: Option<Kind>) -> Result<Vec<HomeObject>, StoreError> {
        self.with(move |conn| list_objects(conn, kind)).await
    }

    /// Create or replace spec. `expected_rv` of 0 means create-only or first write.
    pub async fn apply(&self, mut obj: HomeObject) -> Result<(HomeObject, WatchType), StoreError> {
        self.with(move |conn| {
            if obj.metadata.name.is_empty() {
                obj.metadata.name = default_name(obj.kind);
            }
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sqlite)?;
            let existing = get_object(&tx, obj.kind, &obj.metadata.name)?;
            let watch = if let Some(prev) = existing {
                if obj.metadata.resource_version != prev.metadata.resource_version {
                    return Err(StoreError::from(HomeError::Conflict {
                        kind: obj.kind,
                        name: obj.metadata.name.clone(),
                    }));
                }
                if obj.spec != prev.spec {
                    obj.metadata.generation = prev.metadata.generation + 1;
                } else {
                    obj.metadata.generation = prev.metadata.generation;
                }
                obj.metadata.uid = prev.metadata.uid;
                // Apply replaces spec and preserves observed status. A kind
                // with no spec carries everything in status, so preserving it
                // there would make the object write-once.
                if !obj.spec.is_status_only() {
                    obj.status = prev.status;
                }
                WatchType::Modified
            } else {
                if obj.metadata.resource_version != 0 {
                    return Err(StoreError::from(HomeError::Conflict {
                        kind: obj.kind,
                        name: obj.metadata.name.clone(),
                    }));
                }
                if obj.metadata.name.is_empty() {
                    obj.metadata.name = default_name(obj.kind);
                }
                if obj.metadata.name.is_empty() {
                    return Err(StoreError::from(HomeError::Invalid(
                        "object name is required".into(),
                    )));
                }
                obj.metadata.generation = 1;
                obj.metadata.uid = String::new();
                if obj.kind == Kind::Want {
                    obj.status = StatusBody::empty(Kind::Want);
                }
                WatchType::Added
            };
            let rv = next_rv(&tx)?;
            obj.metadata.resource_version = rv;
            if obj.metadata.uid.is_empty() {
                obj.metadata.uid = format!("{}-{}", obj.kind.store_key(), rv);
            }
            upsert(&tx, &obj)?;
            record_event(&tx, &obj, watch)?;
            tx.commit().map_err(sqlite)?;
            Ok((obj, watch))
        })
        .await
    }

    pub async fn patch_status(
        &self,
        kind: Kind,
        name: &str,
        status: StatusBody,
        expected_rv: i64,
    ) -> Result<HomeObject, StoreError> {
        let name = name.to_string();
        self.with(move |conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sqlite)?;
            let mut obj = get_object(&tx, kind, &name)?.ok_or_else(|| {
                StoreError::from(HomeError::NotFound {
                    kind,
                    name: name.clone(),
                })
            })?;
            if expected_rv != obj.metadata.resource_version {
                return Err(StoreError::from(HomeError::Conflict {
                    kind,
                    name: name.clone(),
                }));
            }
            obj.status = status;
            obj.metadata.resource_version = next_rv(&tx)?;
            upsert(&tx, &obj)?;
            record_event(&tx, &obj, WatchType::Modified)?;
            tx.commit().map_err(sqlite)?;
            Ok(obj)
        })
        .await
    }

    pub async fn patch_spec(
        &self,
        kind: Kind,
        name: &str,
        spec: Spec,
        expected_rv: i64,
    ) -> Result<HomeObject, StoreError> {
        let name = name.to_string();
        self.with(move |conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sqlite)?;
            let mut obj = get_object(&tx, kind, &name)?.ok_or_else(|| {
                StoreError::from(HomeError::NotFound {
                    kind,
                    name: name.clone(),
                })
            })?;
            if expected_rv != obj.metadata.resource_version {
                return Err(StoreError::from(HomeError::Conflict {
                    kind,
                    name: name.clone(),
                }));
            }
            if obj.spec != spec {
                obj.metadata.generation += 1;
            }
            obj.spec = spec;
            obj.metadata.resource_version = next_rv(&tx)?;
            upsert(&tx, &obj)?;
            record_event(&tx, &obj, WatchType::Modified)?;
            tx.commit().map_err(sqlite)?;
            Ok(obj)
        })
        .await
    }

    pub async fn delete(
        &self,
        kind: Kind,
        name: &str,
        expected_rv: i64,
    ) -> Result<HomeObject, StoreError> {
        let name = name.to_string();
        self.with(move |conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sqlite)?;
            let mut obj = get_object(&tx, kind, &name)?.ok_or_else(|| {
                StoreError::from(HomeError::NotFound {
                    kind,
                    name: name.clone(),
                })
            })?;
            if obj.metadata.resource_version != expected_rv {
                return Err(HomeError::Conflict {
                    kind,
                    name: name.clone(),
                }
                .into());
            }
            tx.execute(
                "DELETE FROM objects WHERE kind = ?1 AND name = ?2",
                params![kind.store_key(), name],
            )
            .map_err(sqlite)?;
            // The tombstone needs a fresh rv: a watcher resuming from the
            // pre-delete revision filters out anything at or below it.
            obj.metadata.resource_version = next_rv(&tx)?;
            record_event(&tx, &obj, WatchType::Deleted)?;
            tx.commit().map_err(sqlite)?;
            Ok(obj)
        })
        .await
    }

    pub async fn current_rv(&self) -> Result<i64, StoreError> {
        self.with(|conn| {
            conn.query_row(
                "SELECT value FROM meta WHERE key = 'resource_version'",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite)
        })
        .await
    }

    pub async fn snapshot(&self, kind: Option<Kind>) -> Result<(Vec<HomeObject>, i64), StoreError> {
        self.with(move |conn| {
            let tx = conn.transaction().map_err(sqlite)?;
            let mut items = list_objects(&tx, kind)?;
            items.sort_by_key(|o| o.metadata.resource_version);
            let rv = tx
                .query_row(
                    "SELECT value FROM meta WHERE key = 'resource_version'",
                    [],
                    |row| row.get(0),
                )
                .map_err(sqlite)?;
            Ok((items, rv))
        })
        .await
    }

    /// Ordered, durable history, including tombstones and controller writes.
    /// Zero is an ordinary replay cursor here; only the API treats zero as a
    /// request for an initial snapshot. A delayed empty snapshot can expire.
    pub async fn events_after(&self, after: i64) -> Result<Vec<StoreEvent>, StoreError> {
        self.with(move |conn| {
            let tx = conn.transaction().map_err(sqlite)?;
            let floor: i64 = tx.query_row("SELECT value FROM meta WHERE key = 'watch_replay_floor'", [], |row| row.get(0)).map_err(sqlite)?;
            if after < floor {
                return Err(HomeError::Expired { requested: after, oldest: floor.saturating_add(1) }.into());
            }
            let mut stmt = tx.prepare("SELECT watch, kind, name, uid, generation, resource_version, spec_json, status_json FROM history WHERE resource_version > ?1 ORDER BY resource_version LIMIT 256").map_err(sqlite)?;
            let mut rows = stmt.query([after]).map_err(sqlite)?;
            let mut events = Vec::new();
            while let Some(row) = rows.next().map_err(sqlite)? {
                let watch: String = row.get(0).map_err(sqlite)?;
                let kind: String = row.get(1).map_err(sqlite)?;
                let name: String = row.get(2).map_err(sqlite)?;
                events.push(StoreEvent {
                    watch: match watch.as_str() { "added" => WatchType::Added, "deleted" => WatchType::Deleted, _ => WatchType::Modified },
                    object: decode_object(Kind::parse(&kind)?, &name,
                        row.get(3).map_err(sqlite)?, row.get(4).map_err(sqlite)?, row.get(5).map_err(sqlite)?,
                        row.get(6).map_err(sqlite)?, row.get(7).map_err(sqlite)?)?,
                });
            }
            Ok(events)
        }).await
    }
}

fn default_name(kind: Kind) -> String {
    match kind {
        Kind::Cluster => CLUSTER_NAME.to_string(),
        Kind::Secret => SECRET_NAME.to_string(),
        _ => String::new(),
    }
}

fn open_conn(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open(path).map_err(sqlite)?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(sqlite)?;
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(sqlite)?;
    Ok(conn)
}

fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    // DDL and the version marker must commit together. Dropping this
    // transaction after any error also rolls back partial schema creation.
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(sqlite)?;
    let version: i64 = tx
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sqlite)?;
    if version > API_SCHEMA_VERSION {
        return Err(StoreError::Sqlite(format!(
            "unsupported api.db user_version {version}"
        )));
    }
    if version < 1 {
        tx.execute_batch(
            "CREATE TABLE objects (
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                uid TEXT NOT NULL,
                generation INTEGER NOT NULL,
                resource_version INTEGER NOT NULL,
                spec_json TEXT NOT NULL,
                status_json TEXT NOT NULL,
                PRIMARY KEY (kind, name)
            );
            CREATE INDEX objects_kind ON objects (kind);
            CREATE TABLE meta (
                key TEXT PRIMARY KEY NOT NULL,
                value INTEGER NOT NULL
            );
            INSERT INTO meta (key, value) VALUES ('resource_version', 0);",
        )
        .map_err(sqlite)?;
    }
    if version < 2 {
        tx.execute_batch(
            "
            CREATE TABLE history (
                resource_version INTEGER PRIMARY KEY, watch TEXT NOT NULL,
                kind TEXT NOT NULL, name TEXT NOT NULL, uid TEXT NOT NULL,
                generation INTEGER NOT NULL, spec_json TEXT NOT NULL, status_json TEXT NOT NULL
            );",
        )
        .map_err(sqlite)?;
    }
    if version < 3 {
        // Version 1 retained only current objects: historical deletions and
        // intermediate updates cannot be reconstructed from that snapshot.
        // Version 2 seeded history from those objects without recording its
        // replay boundary, so its older history cannot be trusted either.
        // Clients must snapshot once at this migration's current watermark.
        tx.execute_batch(
            "INSERT INTO meta (key, value)
                SELECT 'watch_replay_floor', value FROM meta WHERE key = 'resource_version';
             DELETE FROM history;",
        )
        .map_err(sqlite)?;
        tx.pragma_update(None, "user_version", API_SCHEMA_VERSION)
            .map_err(sqlite)?;
    }
    tx.commit().map_err(sqlite)
}

fn record_event(conn: &Connection, obj: &HomeObject, watch: WatchType) -> Result<(), StoreError> {
    conn.execute("INSERT INTO history (resource_version, watch, kind, name, uid, generation, spec_json, status_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![obj.metadata.resource_version,
            match watch { WatchType::Added => "added", WatchType::Modified => "modified", WatchType::Deleted => "deleted" },
            obj.kind.store_key(), obj.metadata.name, obj.metadata.uid, obj.metadata.generation,
            encode_spec(&obj.spec)?, encode_status(&obj.status)?]).map_err(sqlite)?;
    let compacted_through = obj
        .metadata
        .resource_version
        .saturating_sub(WATCH_HISTORY_LIMIT);
    conn.execute(
        "DELETE FROM history WHERE resource_version <= ?1",
        [compacted_through],
    )
    .map_err(sqlite)?;
    conn.execute(
        "UPDATE meta SET value = MAX(value, ?1) WHERE key = 'watch_replay_floor'",
        [compacted_through],
    )
    .map_err(sqlite)?;
    Ok(())
}

fn next_rv(conn: &Connection) -> Result<i64, StoreError> {
    conn.execute(
        "UPDATE meta SET value = value + 1 WHERE key = 'resource_version'",
        [],
    )
    .map_err(sqlite)?;
    conn.query_row(
        "SELECT value FROM meta WHERE key = 'resource_version'",
        [],
        |row| row.get(0),
    )
    .map_err(sqlite)
}

fn get_object(conn: &Connection, kind: Kind, name: &str) -> Result<Option<HomeObject>, StoreError> {
    conn.query_row(
        "SELECT uid, generation, resource_version, spec_json, status_json
         FROM objects WHERE kind = ?1 AND name = ?2",
        params![kind.store_key(), name],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        },
    )
    .optional()
    .map_err(sqlite)?
    .map(
        |(uid, generation, resource_version, spec_json, status_json)| {
            decode_object(
                kind,
                name,
                uid,
                generation,
                resource_version,
                spec_json,
                status_json,
            )
        },
    )
    .transpose()
}

fn list_objects(conn: &Connection, kind: Option<Kind>) -> Result<Vec<HomeObject>, StoreError> {
    let mut out = Vec::new();
    if let Some(kind) = kind {
        let mut stmt = conn
            .prepare(
                "SELECT name, uid, generation, resource_version, spec_json, status_json
                 FROM objects WHERE kind = ?1 ORDER BY name",
            )
            .map_err(sqlite)?;
        let rows = stmt
            .query_map(params![kind.store_key()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(sqlite)?;
        for row in rows {
            let (name, uid, generation, rv, spec_json, status_json) = row.map_err(sqlite)?;
            out.push(decode_object(
                kind,
                &name,
                uid,
                generation,
                rv,
                spec_json,
                status_json,
            )?);
        }
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT kind, name, uid, generation, resource_version, spec_json, status_json
                 FROM objects ORDER BY kind, name",
            )
            .map_err(sqlite)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(sqlite)?;
        for row in rows {
            let (kind_key, name, uid, generation, rv, spec_json, status_json) =
                row.map_err(sqlite)?;
            let kind = Kind::parse(&kind_key).map_err(|e| StoreError::Sqlite(e.to_string()))?;
            out.push(decode_object(
                kind,
                &name,
                uid,
                generation,
                rv,
                spec_json,
                status_json,
            )?);
        }
    }
    Ok(out)
}

fn encode_spec(spec: &Spec) -> Result<String, StoreError> {
    let value = match spec {
        Spec::Cluster(s) => serde_json::to_value(s),
        Spec::Secret(s) => serde_json::to_value(s),
        Spec::Title(s) => serde_json::to_value(s),
        Spec::Want(s) => serde_json::to_value(s),
        Spec::Job(s) => serde_json::to_value(s),
        Spec::Hold(s) => serde_json::to_value(s),
        Spec::RemoteFile | Spec::Event => serde_json::to_value(serde_json::Map::new()),
        Spec::Node(s) => serde_json::to_value(s),
    }
    .map_err(|e| StoreError::Sqlite(e.to_string()))?;
    serde_json::to_string(&value).map_err(|e| StoreError::Sqlite(e.to_string()))
}

fn encode_status(status: &StatusBody) -> Result<String, StoreError> {
    let value = match status {
        StatusBody::Cluster(s) => serde_json::to_value(s),
        StatusBody::Secret => serde_json::to_value(serde_json::Map::new()),
        StatusBody::Title(s) => serde_json::to_value(s),
        StatusBody::Want(s) => serde_json::to_value(s),
        StatusBody::Job(s) => serde_json::to_value(s),
        StatusBody::Hold(s) => serde_json::to_value(s),
        StatusBody::RemoteFile(s) => serde_json::to_value(s),
        StatusBody::Node(s) => serde_json::to_value(s),
        StatusBody::Event(s) => serde_json::to_value(s),
    }
    .map_err(|e| StoreError::Sqlite(e.to_string()))?;
    serde_json::to_string(&value).map_err(|e| StoreError::Sqlite(e.to_string()))
}

fn upsert(conn: &Connection, obj: &HomeObject) -> Result<(), StoreError> {
    let spec_json = encode_spec(&obj.spec)?;
    let status_json = encode_status(&obj.status)?;
    conn.execute(
        "INSERT INTO objects (kind, name, uid, generation, resource_version, spec_json, status_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(kind, name) DO UPDATE SET
            uid = excluded.uid,
            generation = excluded.generation,
            resource_version = excluded.resource_version,
            spec_json = excluded.spec_json,
            status_json = excluded.status_json",
        params![
            obj.kind.store_key(),
            obj.metadata.name,
            obj.metadata.uid,
            obj.metadata.generation,
            obj.metadata.resource_version,
            spec_json,
            status_json
        ],
    )
    .map_err(sqlite)?;
    Ok(())
}

fn decode_spec(kind: Kind, spec_json: &str) -> Result<Spec, StoreError> {
    let err = |e: serde_json::Error| StoreError::Sqlite(e.to_string());
    Ok(match kind {
        Kind::Cluster => {
            Spec::Cluster(serde_json::from_str::<ClusterSpec>(spec_json).map_err(err)?)
        }
        Kind::Secret => Spec::Secret(serde_json::from_str::<SecretSpec>(spec_json).map_err(err)?),
        Kind::Title => Spec::Title(serde_json::from_str::<TitleSpec>(spec_json).map_err(err)?),
        Kind::Want => Spec::Want(serde_json::from_str::<WantSpec>(spec_json).map_err(err)?),
        Kind::Job => Spec::Job(serde_json::from_str::<JobSpec>(spec_json).map_err(err)?),
        Kind::Hold => Spec::Hold(serde_json::from_str::<HoldSpec>(spec_json).map_err(err)?),
        Kind::RemoteFile => Spec::RemoteFile,
        Kind::Node => Spec::Node(serde_json::from_str::<NodeSpec>(spec_json).map_err(err)?),
        Kind::Event => Spec::Event,
    })
}

fn decode_status(kind: Kind, status_json: &str) -> Result<StatusBody, StoreError> {
    let err = |e: serde_json::Error| StoreError::Sqlite(e.to_string());
    Ok(match kind {
        Kind::Cluster => {
            StatusBody::Cluster(serde_json::from_str::<ClusterStatus>(status_json).map_err(err)?)
        }
        Kind::Secret => StatusBody::Secret,
        Kind::Title => {
            StatusBody::Title(serde_json::from_str::<TitleStatus>(status_json).map_err(err)?)
        }
        Kind::Want => {
            StatusBody::Want(serde_json::from_str::<WantStatus>(status_json).map_err(err)?)
        }
        Kind::Job => StatusBody::Job(serde_json::from_str::<JobStatus>(status_json).map_err(err)?),
        Kind::Hold => {
            StatusBody::Hold(serde_json::from_str::<HoldStatus>(status_json).map_err(err)?)
        }
        Kind::RemoteFile => StatusBody::RemoteFile(
            serde_json::from_str::<RemoteFileStatus>(status_json).map_err(err)?,
        ),
        Kind::Node => {
            StatusBody::Node(serde_json::from_str::<NodeStatus>(status_json).map_err(err)?)
        }
        Kind::Event => {
            StatusBody::Event(serde_json::from_str::<EventStatus>(status_json).map_err(err)?)
        }
    })
}

fn decode_object(
    kind: Kind,
    name: &str,
    uid: String,
    generation: i64,
    resource_version: i64,
    spec_json: String,
    status_json: String,
) -> Result<HomeObject, StoreError> {
    let spec = decode_spec(kind, &spec_json)?;
    let status = decode_status(kind, &status_json)?;
    Ok(HomeObject {
        api_version: mediaops_core::HOME_API_VERSION.to_string(),
        kind,
        metadata: ObjectMeta {
            name: name.to_string(),
            uid,
            generation,
            resource_version,
        },
        spec,
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{WantPhase, WantSpec, WantStatus};

    fn want(name: &str) -> HomeObject {
        HomeObject::new(
            Kind::Want,
            name,
            Spec::Want(WantSpec {
                title_id: name.into(),
            }),
            StatusBody::empty(Kind::Want),
        )
    }

    fn legacy_database(path: &Path, version: i64, keep_survivor: bool) -> Connection {
        let conn = open_conn(path).expect("legacy database");
        conn.execute_batch(
            "CREATE TABLE objects (
                kind TEXT NOT NULL, name TEXT NOT NULL, uid TEXT NOT NULL,
                generation INTEGER NOT NULL, resource_version INTEGER NOT NULL,
                spec_json TEXT NOT NULL, status_json TEXT NOT NULL,
                PRIMARY KEY (kind, name)
             );
             CREATE INDEX objects_kind ON objects (kind);
             CREATE TABLE meta (key TEXT PRIMARY KEY NOT NULL, value INTEGER NOT NULL);
             INSERT INTO meta VALUES ('resource_version', 3);",
        )
        .expect("legacy schema");
        // Revision 1 may survive; a different object was added at revision 2
        // and deleted at revision 3. Neither old schema retained that deletion.
        if keep_survivor {
            let mut survivor = want("movie:tmdb:603");
            survivor.metadata.uid = "want-1".into();
            survivor.metadata.generation = 1;
            survivor.metadata.resource_version = 1;
            upsert(&conn, &survivor).expect("surviving object");
        }
        if version == 2 {
            conn.execute_batch(
                "CREATE TABLE history (
                    resource_version INTEGER PRIMARY KEY, watch TEXT NOT NULL,
                    kind TEXT NOT NULL, name TEXT NOT NULL, uid TEXT NOT NULL,
                    generation INTEGER NOT NULL, spec_json TEXT NOT NULL, status_json TEXT NOT NULL
                 );
                 INSERT INTO history SELECT resource_version, 'added', kind, name, uid,
                    generation, spec_json, status_json FROM objects;",
            )
            .expect("legacy synthetic history");
        }
        conn.pragma_update(None, "user_version", version)
            .expect("legacy version");
        conn
    }

    #[tokio::test]
    async fn empty_snapshot_cursor_zero_expires_after_compaction() {
        let dir = crate::scratch("api-empty-snapshot-expiry");
        let path = dir.join("api.db");
        let store = ApiStore::open(&path).await.expect("open");
        let (items, cursor) = store.snapshot(None).await.expect("empty snapshot");
        assert!(items.is_empty());
        assert_eq!(cursor, 0);
        store
            .with(|conn| {
                // Batch writes keep the fixture fast while exercising the
                // production event insertion and compaction at every revision.
                let tx = conn.transaction().map_err(sqlite)?;
                let mut object = want("movie:tmdb:603");
                object.metadata.uid = "want-1".into();
                object.metadata.generation = 1;
                for revision in 1..=WATCH_HISTORY_LIMIT + 1 {
                    object.metadata.resource_version = next_rv(&tx)?;
                    record_event(
                        &tx,
                        &object,
                        if revision == 1 {
                            WatchType::Added
                        } else {
                            WatchType::Modified
                        },
                    )?;
                }
                upsert(&tx, &object)?;
                tx.commit().map_err(sqlite)
            })
            .await
            .expect("compact history");
        assert!(matches!(
            store.events_after(cursor).await,
            Err(StoreError::Home(HomeError::Expired {
                requested: 0,
                oldest: 2
            }))
        ));
        let retained = store.events_after(1).await.expect("boundary remains valid");
        assert_eq!(retained[0].object.metadata.resource_version, 2);
        drop(store);
        let reopened = ApiStore::open(&path).await.expect("reopen");
        assert!(matches!(
            reopened.events_after(0).await,
            Err(StoreError::Home(HomeError::Expired { .. }))
        ));
        drop(reopened);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn legacy_migrations_expire_missing_tombstones_even_with_no_surviving_objects() {
        for version in [1, 2] {
            for keep_survivor in [false, true] {
                let dir = crate::scratch("api-legacy-replay-floor");
                let path = dir.join("api.db");
                drop(legacy_database(&path, version, keep_survivor));
                let store = ApiStore::open(&path).await.expect("migrate");
                for cursor in 0..3 {
                    assert!(matches!(
                        store.events_after(cursor).await,
                        Err(StoreError::Home(HomeError::Expired {
                            requested,
                            oldest: 4
                        })) if requested == cursor
                    ));
                }
                let (snapshot, cursor) = store.snapshot(None).await.expect("snapshot");
                assert_eq!(snapshot.len(), usize::from(keep_survivor));
                assert_eq!(cursor, 3);
                assert!(
                    store
                        .events_after(cursor)
                        .await
                        .expect("caught up")
                        .is_empty()
                );
                let (created, _) = store
                    .apply(want("movie:tmdb:604"))
                    .await
                    .expect("new event");
                let events = store
                    .events_after(cursor)
                    .await
                    .expect("post-migration replay");
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].object, created);
                assert_eq!(created.metadata.resource_version, 4);
                drop(store);
                let reopened = ApiStore::open(&path).await.expect("reopen");
                assert_eq!(
                    reopened.events_after(cursor).await.expect("durable"),
                    events
                );
                assert!(matches!(
                    reopened.events_after(2).await,
                    Err(StoreError::Home(HomeError::Expired { .. }))
                ));
                drop(reopened);
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    #[tokio::test]
    async fn failed_schema_creation_rolls_back_ddl_and_version_then_reopens() {
        let dir = crate::scratch("api-schema-rollback");
        let path = dir.join("api.db");
        let mut conn = open_conn(&path).expect("connection");
        // Enough pages for the first table and indexes, but not the complete
        // schema: SQLite fails after DDL has already run inside the transaction.
        conn.pragma_update(None, "max_page_count", 4)
            .expect("page limit");
        let error = migrate(&mut conn).expect_err("disk-full schema failure");
        assert!(error.to_string().contains("full"), "{error}");
        assert!(
            conn.is_autocommit(),
            "failed migration left a transaction open"
        );
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, 0);
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .expect("tables");
        assert_eq!(tables, 0, "partial schema must roll back");
        drop(conn);
        let store = ApiStore::open(&path)
            .await
            .expect("reopen after failed creation");
        store
            .apply(want("movie:tmdb:603"))
            .await
            .expect("usable schema");
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn failed_legacy_migration_preserves_old_schema_and_data_then_reopens() {
        let dir = crate::scratch("api-migration-rollback");
        let path = dir.join("api.db");
        let mut conn = legacy_database(&path, 1, true);
        let pages: i64 = conn
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .expect("pages");
        conn.pragma_update(None, "max_page_count", pages)
            .expect("page limit");
        assert!(migrate(&mut conn).is_err(), "migration needs another page");
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, 1);
        assert_eq!(list_objects(&conn, None).expect("old data").len(), 1);
        drop(conn);
        let store = ApiStore::open(&path).await.expect("retry migration");
        assert_eq!(store.list(None).await.expect("preserved data").len(), 1);
        assert!(matches!(
            store.events_after(0).await,
            Err(StoreError::Home(HomeError::Expired { .. }))
        ));
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn zero_version_cannot_replace_patch_or_delete_and_concurrent_writes_have_one_winner() {
        let dir = crate::scratch("api-cas");
        let store = ApiStore::open(dir.join("api.db")).await.expect("open");
        let (created, _) = store.apply(want("movie:tmdb:603")).await.expect("create");
        assert!(store.apply(want("movie:tmdb:603")).await.is_err());
        assert!(
            store
                .patch_status(
                    Kind::Want,
                    &created.metadata.name,
                    created.status.clone(),
                    0
                )
                .await
                .is_err()
        );
        assert!(
            store
                .patch_spec(Kind::Want, &created.metadata.name, created.spec.clone(), 0)
                .await
                .is_err()
        );
        assert!(
            store
                .delete(Kind::Want, &created.metadata.name, 0)
                .await
                .is_err()
        );
        let (first, second) =
            tokio::join!(store.apply(created.clone()), store.apply(created.clone()));
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        let history = store.events_after(0).await.expect("history");
        assert_eq!(history.len(), 2, "failed writes never create revisions");
        drop(store);
        let reopened = ApiStore::open(dir.join("api.db")).await.expect("reopen");
        assert_eq!(reopened.events_after(0).await.expect("durable"), history);
        drop(reopened);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn watches_include_status_and_tombstone_and_expire_explicitly() {
        let dir = crate::scratch("api-history");
        let store = ApiStore::open(dir.join("api.db")).await.expect("open");
        let (created, _) = store.apply(want("movie:tmdb:603")).await.expect("create");
        let modified = store
            .patch_status(
                Kind::Want,
                &created.metadata.name,
                StatusBody::Want(WantStatus {
                    phase: WantPhase::Satisfied,
                }),
                created.metadata.resource_version,
            )
            .await
            .expect("status");
        let deleted = store
            .delete(
                Kind::Want,
                &created.metadata.name,
                modified.metadata.resource_version,
            )
            .await
            .expect("delete");
        let events = store
            .events_after(created.metadata.resource_version)
            .await
            .expect("events");
        assert_eq!(
            events.iter().map(|e| e.watch).collect::<Vec<_>>(),
            [WatchType::Modified, WatchType::Deleted]
        );
        assert_eq!(events[1].object, deleted);
        store
            .with(|conn| {
                conn.execute(
                    "UPDATE meta SET value = ?1 WHERE key = 'resource_version'",
                    [WATCH_HISTORY_LIMIT + 10],
                )
                .map_err(sqlite)?;
                Ok(())
            })
            .await
            .expect("advance clock");
        store
            .apply(want("movie:tmdb:604"))
            .await
            .expect("prune old history");
        assert!(matches!(
            store.events_after(1).await,
            Err(StoreError::Home(HomeError::Expired { .. }))
        ));
        let (snapshot, rv) = store.snapshot(None).await.expect("relist");
        assert_eq!(snapshot.len(), 1);
        assert!(rv > WATCH_HISTORY_LIMIT);
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn database_is_private_exclusive_and_refuses_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::scratch("api-private");
        let store = ApiStore::open(dir.join("api.db")).await.expect("open");
        assert_eq!(
            std::fs::metadata(dir.join("api.db"))
                .expect("meta")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            ApiStore::open(dir.join("api.db")).await.is_err(),
            "second owner denied"
        );
        std::os::unix::fs::symlink(dir.join("api.db"), dir.join("alias.db")).expect("symlink");
        assert!(ApiStore::open(dir.join("alias.db")).await.is_err());
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn apply_get_conflict_and_delete() {
        let dir = crate::scratch("api-store");
        let store = ApiStore::open(dir.join("api.db")).await.expect("open");
        let obj = HomeObject::new(
            Kind::Want,
            "movie:tmdb:603",
            Spec::Want(WantSpec {
                title_id: "movie:tmdb:603".into(),
            }),
            StatusBody::Want(WantStatus {
                phase: WantPhase::Open,
            }),
        );
        let (created, watch) = store.apply(obj.clone()).await.expect("apply");
        assert_eq!(watch, WatchType::Added);
        assert_eq!(created.metadata.generation, 1);
        assert!(created.metadata.resource_version > 0);

        let got = store
            .get(Kind::Want, "movie:tmdb:603")
            .await
            .expect("get")
            .expect("row");
        assert_eq!(got.metadata.uid, created.metadata.uid);

        let mut stale = created.clone();
        stale.metadata.resource_version = 999;
        let err = store.apply(stale).await.expect_err("conflict");
        assert!(err.to_string().contains("resourceVersion"), "{err}");

        store
            .delete(
                Kind::Want,
                "movie:tmdb:603",
                created.metadata.resource_version,
            )
            .await
            .expect("delete");
        assert!(
            store
                .get(Kind::Want, "movie:tmdb:603")
                .await
                .expect("get")
                .is_none()
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
