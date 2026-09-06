use mediaops_core::{
    ClusterSpec, HomeObject, JobSpec, Kind, Spec, StatusBody, SyncDisposition, SyncEntry,
    SyncPhase, SyncSpec, SyncStatus,
};

use super::ApiStore;

fn request(root: &str, malformed_second: bool) -> HomeObject {
    let mut entries = Vec::new();
    for (name, title, path) in [
        (
            "matrix",
            "movie:key:thematrix.1999",
            "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv",
        ),
        (
            "alien",
            "movie:key:alien.1979",
            "movies/Alien.(1979)/Alien.(1979).mkv",
        ),
    ] {
        let mut job = JobSpec {
            sync_name: "sync-atomic".into(),
            title_id: title.into(),
            remote_root: "box".into(),
            remote_path: path.into(),
            dest_rel: path.into(),
            file_len: 4,
            range_len: 4,
            range_concurrency: 1,
            library_root: root.into(),
            worker_kind: "pull".into(),
            ..Default::default()
        };
        if malformed_second && name == "alien" {
            job.range_len = 0;
        }
        entries.push(SyncEntry {
            job_name: name.into(),
            job: Some(job),
            disposition: SyncDisposition::WouldQueue,
            ..Default::default()
        });
    }
    HomeObject::new(
        Kind::Sync,
        "sync-atomic",
        Spec::Sync(SyncSpec::default()),
        StatusBody::Sync(SyncStatus {
            phase: SyncPhase::Captured,
            deadline_unix: i64::MAX,
            cluster: Some(ClusterSpec {
                library_root: root.into(),
                ..Default::default()
            }),
            entries,
            ..Default::default()
        }),
    )
}

#[tokio::test]
async fn scheduling_rolls_back_every_job_and_title_if_one_entry_is_invalid() {
    let dir = scratch("rollback");
    let store = ApiStore::open(dir.join("api.db")).await.expect("store");
    let request = store
        .apply(request(dir.to_str().expect("path"), true))
        .await
        .expect("request")
        .0;
    let version = store.current_rv().await.expect("rv");
    assert!(store.schedule_sync(version, request.clone()).await.is_err());
    assert!(store.list(Some(Kind::Job)).await.expect("jobs").is_empty());
    assert!(
        store
            .list(Some(Kind::Title))
            .await
            .expect("titles")
            .is_empty()
    );
    assert_eq!(
        store.get(Kind::Sync, "sync-atomic").await.expect("get"),
        Some(request)
    );
    assert_eq!(store.current_rv().await.expect("rv"), version);
    drop(store);
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[tokio::test]
async fn scheduling_records_exact_job_uids_in_the_same_durable_transaction() {
    let dir = scratch("commit");
    let store = ApiStore::open(dir.join("api.db")).await.expect("store");
    let request = store
        .apply(request(dir.to_str().expect("path"), false))
        .await
        .expect("request")
        .0;
    let version = store.current_rv().await.expect("rv");
    let committed = store.schedule_sync(version, request).await.expect("commit");
    drop(store);
    let reopened = ApiStore::open(dir.join("api.db")).await.expect("reopen");
    assert_eq!(
        reopened.get(Kind::Sync, "sync-atomic").await.expect("get"),
        Some(committed.clone())
    );
    let StatusBody::Sync(status) = committed.status else {
        panic!("status");
    };
    assert_eq!(status.phase, SyncPhase::Scheduled);
    for entry in status.entries {
        let job = reopened
            .get(Kind::Job, &entry.job_name)
            .await
            .expect("get job")
            .expect("job");
        assert_eq!(entry.job_uid, job.metadata.uid);
        assert_eq!(entry.disposition, SyncDisposition::Queued);
    }
    assert_eq!(
        reopened
            .list(Some(Kind::Title))
            .await
            .expect("titles")
            .len(),
        2
    );
    drop(reopened);
    std::fs::remove_dir_all(dir).expect("cleanup");
}

fn scratch(name: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "mediaops-sync-{name}-{}-{nanos}",
        std::process::id()
    ))
}

#[tokio::test]
async fn version_three_upgrade_preserves_objects_and_watch_history() {
    let dir = scratch("v3");
    let db = dir.join("api.db");
    let store = ApiStore::open(&db).await.expect("store");
    let want = store
        .apply(HomeObject::new(
            Kind::Want,
            "movie:tmdb:603",
            Spec::Want(mediaops_core::WantSpec {
                title_id: "movie:tmdb:603".into(),
            }),
            StatusBody::empty(Kind::Want),
        ))
        .await
        .expect("want")
        .0;
    let history = store.events_after(0).await.expect("history");
    drop(store);
    let conn = super::open_conn(&db).expect("connection");
    conn.pragma_update(None, "user_version", 3)
        .expect("legacy marker");
    drop(conn);
    let reopened = ApiStore::open(&db).await.expect("upgrade");
    assert_eq!(
        reopened
            .get(Kind::Want, "movie:tmdb:603")
            .await
            .expect("want"),
        Some(want)
    );
    assert_eq!(reopened.events_after(0).await.expect("history"), history);
    let conn = super::open_conn(&db).expect("connection");
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("marker");
    assert_eq!(version, 4);
    drop(conn);
    drop(reopened);
    std::fs::remove_dir_all(dir).expect("cleanup");
}
