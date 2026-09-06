mod support;
#[path = "support/sync.rs"]
mod sync_support;

use mediaops_core::{
    Actor, HoldDecisionSpec, HoldSpec, HoldStatus, HomeObject, Kind, Placement, Spec, StatusBody,
    SyncDisposition, WorkerKind,
};
use mediaops_home_client::HomeApi;
use sync_support::{MATRIX, configured, inventory, now, publish, scheduled, start};

#[tokio::test]
async fn sync_job_binds_without_want_but_forged_sync_job_is_denied() {
    let home = configured("sync-bind").await;
    let pending = start(&home, "sync-bind").await;
    publish(&home, 1, &[MATRIX]).await;
    scheduled(pending).await;
    let pull = HomeApi::connect(&home.socket, Actor::Pull)
        .await
        .expect("pull");
    pull.heartbeat(WorkerKind::Pull, true, None)
        .await
        .expect("ready pull");
    let mut job = home
        .api
        .list(Some(Kind::Job))
        .await
        .expect("jobs")
        .remove(0);
    let controller = HomeApi::connect(&home.socket, Actor::Controller)
        .await
        .expect("controller");
    let forged = HomeObject::new(
        Kind::Job,
        "forged-copy",
        job.spec.clone(),
        StatusBody::empty(Kind::Job),
    );
    assert!(
        controller
            .apply(forged)
            .await
            .expect_err("forged Sync Job refused")
            .is_denied()
    );
    if let Spec::Job(spec) = &mut job.spec {
        spec.node_name = "pull".into();
    }
    let scheduler = HomeApi::connect(&home.socket, Actor::Scheduler)
        .await
        .expect("scheduler");
    let bound = scheduler
        .patch(job, "bind")
        .await
        .expect("bind authorized snapshot");
    assert!(
        matches!(bound.spec, Spec::Job(spec) if spec.node_name == "pull" && spec.sync_name == "sync-bind")
    );
    assert!(
        home.api
            .list(Some(Kind::Want))
            .await
            .expect("wants")
            .is_empty()
    );
}

#[tokio::test]
async fn undecided_hold_blocks_sync_without_changing_its_decision() {
    let home = configured("sync-hold").await;
    let pending = start(&home, "sync-hold").await;
    let inv = inventory(&home).await;
    inv.begin_inventory().await.expect("begin");
    sync_support::rows(&inv, 1, &[MATRIX]).await;
    inv.apply(HomeObject::new(
        Kind::Hold,
        "movie:tmdb:603-release",
        Spec::Hold(HoldSpec {
            title_id: "movie:tmdb:603".into(),
            release_id: "release".into(),
            decision: HoldDecisionSpec::Empty,
        }),
        StatusBody::Hold(HoldStatus {
            list_generation: 1,
            remote_root: "box".into(),
            remote_path: MATRIX.into(),
            placement: Some(Placement::movie("The.Matrix", 1999, "mkv")),
            ..Default::default()
        }),
    ))
    .await
    .expect("hold");
    inv.heartbeat(WorkerKind::Inventory, true, Some((1, now())))
        .await
        .expect("publish");
    let result = scheduled(pending).await;
    assert!(
        matches!(result.status, StatusBody::Sync(st) if st.entries[0].disposition == SyncDisposition::Blocked)
    );
    let hold = home
        .api
        .get(Kind::Hold, "movie:tmdb:603-release")
        .await
        .expect("hold");
    assert!(matches!(hold.spec, Spec::Hold(spec) if spec.decision == HoldDecisionSpec::Empty));
    assert!(
        home.api
            .list(Some(Kind::Job))
            .await
            .expect("jobs")
            .is_empty()
    );
}

#[tokio::test]
async fn changed_source_configuration_refuses_unbound_sync_job() {
    let home = configured("sync-source-config").await;
    let pending = start(&home, "sync-source-config").await;
    publish(&home, 1, &[MATRIX]).await;
    scheduled(pending).await;
    let pull = HomeApi::connect(&home.socket, Actor::Pull)
        .await
        .expect("pull");
    pull.heartbeat(WorkerKind::Pull, true, None)
        .await
        .expect("ready");
    let mut cluster = home.api.get(Kind::Cluster, "home").await.expect("cluster");
    if let Spec::Cluster(config) = &mut cluster.spec {
        config.roots[0].path = "/different-seedbox/path".into();
    }
    home.api.apply(cluster).await.expect("change root");
    let mut job = home
        .api
        .list(Some(Kind::Job))
        .await
        .expect("jobs")
        .remove(0);
    if let Spec::Job(spec) = &mut job.spec {
        spec.node_name = "pull".into();
    }
    let scheduler = HomeApi::connect(&home.socket, Actor::Scheduler)
        .await
        .expect("scheduler");
    assert!(scheduler.patch(job, "bind").await.is_err());
}
