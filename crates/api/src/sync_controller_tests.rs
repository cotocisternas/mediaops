use mediaops_core::{
    Bytes, ClusterSpec, HomeObject, Kind, NodeSpec, NodeStatus, PathRoot, RemoteFileStatus, Spec,
    StatusBody, SyncPhase, SyncSpec, SyncStatus, TitleKind, WorkerKind,
};
use mediaops_store::ApiStore;
use tokio::sync::{Mutex, Notify};

use crate::serve::Inner;

async fn fixture(tag: &str) -> (Inner, std::path::PathBuf, ClusterSpec) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "sync-controller-{tag}-{}-{stamp}",
        std::process::id()
    ));
    let store = ApiStore::open(dir.join("api.db")).await.expect("store");
    let config = ClusterSpec {
        library_root: dir.to_string_lossy().into_owned(),
        max_copy: Bytes::new(1024),
        roots: vec![PathRoot {
            id: "box".into(),
            path: "/box".into(),
            kind: Some(TitleKind::Movie),
        }],
        ..Default::default()
    };
    store
        .apply(HomeObject::new(
            Kind::Cluster,
            "home",
            Spec::Cluster(config.clone()),
            StatusBody::empty(Kind::Cluster),
        ))
        .await
        .expect("cluster");
    (
        Inner {
            store,
            mutation: Mutex::new(()),
            wake: Notify::new(),
        },
        dir,
        config,
    )
}

#[tokio::test]
async fn expired_request_is_failed_without_copy_jobs() {
    let (inner, dir, config) = fixture("expired").await;
    inner
        .store
        .apply(HomeObject::new(
            Kind::Sync,
            "sync-expired",
            Spec::Sync(SyncSpec::default()),
            StatusBody::Sync(SyncStatus {
                deadline_unix: crate::sync::now() - 1,
                cluster: Some(config),
                ..Default::default()
            }),
        ))
        .await
        .expect("pending request");
    crate::sync_controller::reconcile(&inner)
        .await
        .expect("reconcile");
    let request = inner
        .store
        .get(Kind::Sync, "sync-expired")
        .await
        .expect("get")
        .expect("request");
    assert!(matches!(request.status, StatusBody::Sync(st) if st.phase == SyncPhase::Failed));
    assert!(
        inner
            .store
            .list(Some(Kind::Job))
            .await
            .expect("jobs")
            .is_empty()
    );
    drop(inner);
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[tokio::test]
async fn captured_manifest_is_recovered_after_store_restart_without_wants() {
    let (inner, dir, config) = fixture("restart").await;
    inner
        .store
        .apply(HomeObject::new(
            Kind::Node,
            "inventory",
            Spec::Node(NodeSpec {
                worker_kind: WorkerKind::Inventory,
            }),
            StatusBody::Node(NodeStatus {
                ready: true,
                last_heartbeat_unix: crate::sync::now(),
                list_completed_unix: crate::sync::now(),
                list_generation: 1,
                scan_started_rv: 2,
                scan_cluster_generation: 1,
                ..Default::default()
            }),
        ))
        .await
        .expect("inventory");
    inner
        .store
        .apply(HomeObject::new(
            Kind::RemoteFile,
            "box/The.Matrix.(1999)/The.Matrix.(1999).mkv",
            Spec::RemoteFile,
            StatusBody::RemoteFile(RemoteFileStatus {
                root_id: "box".into(),
                rel_path: "The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
                len: 4,
                parse_ok: true,
                title_id: "movie:key:thematrix.1999".into(),
                list_generation: 1,
            }),
        ))
        .await
        .expect("remote");
    let mut request = HomeObject::new(
        Kind::Sync,
        "sync-restart",
        Spec::Sync(SyncSpec::default()),
        StatusBody::Sync(SyncStatus {
            phase: SyncPhase::Captured,
            acceptance_rv: 1,
            list_generation: 1,
            deadline_unix: crate::sync::now() + 60,
            cluster_generation: 1,
            cluster: Some(config),
            ..Default::default()
        }),
    );
    let objects = inner.store.snapshot(None).await.expect("snapshot").0;
    let entries = crate::sync_plan::capture(&objects, &request);
    assert_eq!(entries.len(), 1);
    if let StatusBody::Sync(st) = &mut request.status {
        st.entries = entries;
    }
    inner.store.apply(request).await.expect("captured request");
    drop(inner);
    let reopened = Inner {
        store: ApiStore::open(dir.join("api.db")).await.expect("restart"),
        mutation: Mutex::new(()),
        wake: Notify::new(),
    };
    crate::sync_controller::reconcile(&reopened)
        .await
        .expect("recovery");
    let request = reopened
        .store
        .get(Kind::Sync, "sync-restart")
        .await
        .expect("get")
        .expect("request");
    assert!(matches!(request.status, StatusBody::Sync(st) if st.phase == SyncPhase::Scheduled));
    assert_eq!(
        reopened
            .store
            .list(Some(Kind::Job))
            .await
            .expect("jobs")
            .len(),
        1
    );
    assert!(
        reopened
            .store
            .list(Some(Kind::Want))
            .await
            .expect("wants")
            .is_empty()
    );
    drop(reopened);
    std::fs::remove_dir_all(dir).expect("cleanup");
}
