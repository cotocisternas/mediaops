mod support;
#[path = "support/sync.rs"]
mod sync_support;

use mediaops_core::{Kind, Spec, StatusBody, SyncDisposition, SyncPhase, WorkerKind};
use sync_support::{MATRIX, configured, inventory, now, publish, rows, scheduled, start};

const OTHER: &str = "Alien.(1979)/Alien.(1979).mkv";

#[tokio::test]
async fn concurrent_requests_share_one_copy_job() {
    let home = configured("sync-concurrent").await;
    let left = start(&home, "sync-left").await;
    let right = start(&home, "sync-right").await;
    publish(&home, 1, &[MATRIX]).await;
    let results = [scheduled(left).await, scheduled(right).await];
    let mut created = 0;
    let mut reused = 0;
    for result in results {
        if let StatusBody::Sync(status) = result.status {
            created += status
                .entries
                .iter()
                .filter(|entry| entry.disposition == SyncDisposition::Queued)
                .count();
            reused += status
                .entries
                .iter()
                .filter(|entry| entry.disposition == SyncDisposition::AlreadyQueued)
                .count();
        }
    }
    assert_eq!((created, reused), (1, 1));
    assert_eq!(home.api.list(Some(Kind::Job)).await.expect("jobs").len(), 1);
}

#[tokio::test]
async fn sync_without_wants_creates_finite_per_file_jobs() {
    let home = configured("sync-finite").await;
    let pending = start(&home, "sync-first").await;
    publish(&home, 1, &[MATRIX]).await;
    let request = scheduled(pending).await;
    let StatusBody::Sync(status) = &request.status else {
        panic!("sync status");
    };
    assert_eq!(
        status
            .entries
            .iter()
            .filter(|e| e.disposition == SyncDisposition::Queued)
            .count(),
        1
    );
    assert!(
        home.api
            .list(Some(Kind::Want))
            .await
            .expect("wants")
            .is_empty()
    );
    publish(&home, 2, &[MATRIX, OTHER]).await;
    let repeated = home
        .api
        .sync("sync-first", false)
        .await
        .expect("same request");
    assert_eq!(repeated, request);
    assert_eq!(home.api.list(Some(Kind::Job)).await.expect("jobs").len(), 1);
}

#[tokio::test]
async fn scan_started_before_request_does_not_satisfy_freshness() {
    let home = configured("sync-fresh").await;
    let inv = inventory(&home).await;
    inv.begin_inventory().await.expect("old scan started");
    let pending = start(&home, "sync-fresh").await;
    rows(&inv, 1, &[MATRIX]).await;
    inv.heartbeat(WorkerKind::Inventory, true, Some((1, now())))
        .await
        .expect("old scan committed");
    let request = home
        .api
        .get(Kind::Sync, "sync-fresh")
        .await
        .expect("request");
    assert!(
        matches!(&request.status, StatusBody::Sync(st) if st.phase == SyncPhase::WaitingInventory)
    );
    assert!(
        home.api
            .list(Some(Kind::Job))
            .await
            .expect("jobs")
            .is_empty()
    );
    publish(&home, 2, &[MATRIX]).await;
    let request = scheduled(pending).await;
    assert!(matches!(request.status, StatusBody::Sync(st) if st.list_generation == 2));
}

#[tokio::test]
async fn repeat_sync_reuses_existing_job_without_reauthorizing_it() {
    let home = configured("sync-dedup").await;
    let pending = start(&home, "sync-original").await;
    publish(&home, 1, &[MATRIX]).await;
    scheduled(pending).await;
    let pending = start(&home, "sync-again").await;
    publish(&home, 2, &[MATRIX]).await;
    let result = scheduled(pending).await;
    assert!(
        matches!(result.status, StatusBody::Sync(st) if st.entries[0].disposition == SyncDisposition::AlreadyQueued)
    );
    let jobs = home.api.list(Some(Kind::Job)).await.expect("jobs");
    assert_eq!(jobs.len(), 1);
    assert!(matches!(&jobs[0].spec, Spec::Job(spec) if spec.sync_name == "sync-original"));
}

#[tokio::test]
async fn existing_unproved_destination_blocks_copy_without_overwrite() {
    let home = configured("sync-existing").await;
    let dest = home.dir.join("library/movies").join(MATRIX);
    std::fs::create_dir_all(dest.parent().expect("parent")).expect("dirs");
    std::fs::write(&dest, b"keep").expect("file");
    let pending = start(&home, "sync-existing").await;
    publish(&home, 1, &[MATRIX]).await;
    let result = scheduled(pending).await;
    assert!(
        matches!(result.status, StatusBody::Sync(st) if st.entries[0].disposition == SyncDisposition::Blocked)
    );
    assert!(
        home.api
            .list(Some(Kind::Job))
            .await
            .expect("jobs")
            .is_empty()
    );
    assert_eq!(std::fs::read(dest).expect("original"), b"keep");
}

#[tokio::test]
async fn deleting_unbound_sync_job_does_not_recreate_it() {
    let home = configured("sync-delete").await;
    let pending = start(&home, "sync-delete").await;
    publish(&home, 1, &[MATRIX]).await;
    let request = scheduled(pending).await;
    assert!(home.api.delete(Kind::Sync, "sync-delete").await.is_err());
    let job = home
        .api
        .list(Some(Kind::Job))
        .await
        .expect("jobs")
        .remove(0);
    home.api
        .delete(Kind::Job, &job.metadata.name)
        .await
        .expect("delete pending job");
    assert_eq!(
        home.api
            .sync("sync-delete", false)
            .await
            .expect("idempotent request"),
        request
    );
    assert!(
        home.api
            .list(Some(Kind::Job))
            .await
            .expect("jobs")
            .is_empty()
    );
}
