mod support;
#[path = "support/sync.rs"]
mod sync_support;

use mediaops_core::{Kind, StatusBody, SyncDisposition};
use sync_support::{MATRIX, configured, publish};

#[tokio::test]
async fn preview_waits_for_new_inventory_but_writes_no_request_or_copy_objects() {
    let home = configured("sync-preview").await;
    let preview = home.api.sync("preview-only", true);
    tokio::pin!(preview);
    let mut generation = 0;
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut preview => break result.expect("preview"),
                _ = interval.tick() => { generation += 1; publish(&home, generation, &[MATRIX]).await; }
            }
        }
    }).await.expect("preview deadline");
    assert!(
        matches!(result.status, StatusBody::Sync(st) if st.entries.len() == 1 && st.entries[0].disposition == SyncDisposition::WouldQueue)
    );
    for kind in [Kind::Sync, Kind::Want, Kind::Job, Kind::Title, Kind::Event] {
        assert!(
            home.api.list(Some(kind)).await.expect("objects").is_empty(),
            "preview wrote {kind}"
        );
    }
}

#[tokio::test]
async fn duplicate_sources_are_blocked_instead_of_selected_by_iteration_order() {
    let home = configured("sync-ambiguous").await;
    let pending = sync_support::start(&home, "sync-ambiguous").await;
    publish(
        &home,
        1,
        &[MATRIX, "The.Matrix.(1999)/The.Matrix.(1999).mp4"],
    )
    .await;
    let result = sync_support::scheduled(pending).await;
    assert!(
        matches!(result.status, StatusBody::Sync(st) if st.entries.len() == 2 && st.entries.iter().all(|entry| entry.disposition == SyncDisposition::Blocked))
    );
    assert!(
        home.api
            .list(Some(Kind::Job))
            .await
            .expect("jobs")
            .is_empty()
    );
}
