use mediaops_core::{
    HomeError, HomeObject, Kind, Spec, StatusBody, SyncDisposition, SyncPhase, TitleSpec,
};
use rusqlite::Connection;

use super::{ApiStore, WatchType, get_object, next_rv, record_event, upsert};
use crate::{StoreError, sqlite};

impl ApiStore {
    /// Commit the finite request and its new Jobs in one transaction.
    pub async fn schedule_sync(
        &self,
        expected_snapshot_rv: i64,
        mut request: HomeObject,
    ) -> Result<HomeObject, StoreError> {
        self.with(move |conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sqlite)?;
            let rv: i64 = tx
                .query_row(
                    "SELECT value FROM meta WHERE key = 'resource_version'",
                    [],
                    |row| row.get(0),
                )
                .map_err(sqlite)?;
            let stored = get_object(&tx, Kind::Sync, &request.metadata.name)?;
            if rv != expected_snapshot_rv || !stored.is_some_and(|old| {
                old.metadata.resource_version == request.metadata.resource_version
                    && matches!(old.status, StatusBody::Sync(st) if st.phase == SyncPhase::Captured)
            }) {
                return Err(HomeError::Conflict {
                    kind: Kind::Sync,
                    name: request.metadata.name.clone(),
                }
                .into());
            }
            let StatusBody::Sync(status) = &mut request.status else {
                return Err(HomeError::Invalid("Sync status required".into()).into());
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .and_then(|d| i64::try_from(d.as_secs()).ok())
                .unwrap_or(i64::MAX);
            if now >= status.deadline_unix {
                return Err(HomeError::Invalid(
                    "Sync planning deadline expired before commit".into(),
                )
                .into());
            }
            for entry in &mut status.entries {
                if entry.disposition != SyncDisposition::WouldQueue {
                    continue;
                }
                let spec = entry.job.as_ref().ok_or_else(|| {
                    HomeError::Invalid("queued Sync entry needs a Job snapshot".into())
                })?;
                if spec.sync_name != request.metadata.name
                    || entry.job_name.is_empty()
                    || !spec.node_name.is_empty()
                {
                    return Err(HomeError::Invalid("invalid Sync Job authority".into()).into());
                }
                if get_object(&tx, Kind::Job, &entry.job_name)?.is_some() {
                    return Err(HomeError::Conflict {
                        kind: Kind::Job,
                        name: entry.job_name.clone(),
                    }
                    .into());
                }
                let mut job = HomeObject::new(
                    Kind::Job,
                    &entry.job_name,
                    Spec::Job(spec.clone()),
                    StatusBody::empty(Kind::Job),
                );
                job.validate()?;
                if get_object(&tx, Kind::Title, &spec.title_id)?.is_none() {
                    let mut title = HomeObject::new(
                        Kind::Title,
                        &spec.title_id,
                        Spec::Title(TitleSpec {
                            title_id: spec.title_id.clone(),
                            desired_present: true,
                        }),
                        StatusBody::empty(Kind::Title),
                    );
                    create(&tx, &mut title)?;
                }
                create(&tx, &mut job)?;
                entry.job_uid = job.metadata.uid;
                entry.disposition = SyncDisposition::Queued;
            }
            status.phase = SyncPhase::Scheduled;
            status.message = "Planning complete; Pull Jobs execute independently".into();
            request.metadata.resource_version = next_rv(&tx)?;
            upsert(&tx, &request)?;
            record_event(&tx, &request, WatchType::Modified)?;
            tx.commit().map_err(sqlite)?;
            Ok(request)
        })
        .await
    }
}

fn create(conn: &Connection, obj: &mut HomeObject) -> Result<(), StoreError> {
    obj.metadata.generation = 1;
    obj.metadata.resource_version = next_rv(conn)?;
    obj.metadata.uid = format!("{}-{}", obj.kind.store_key(), obj.metadata.resource_version);
    upsert(conn, obj)?;
    record_event(conn, obj, WatchType::Added)
}
