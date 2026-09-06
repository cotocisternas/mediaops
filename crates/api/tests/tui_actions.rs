mod support;

use mediaops_core::{
    Actor, CLUSTER_NAME, ClusterSpec, ClusterStatus, HoldDecisionSpec, HoldSpec, HoldStatus,
    HomeObject, Kind, NodeSpec, NodeStatus, Placement, Spec, StatusBody, WantSpec, WantStatus,
    WorkerKind,
};
use mediaops_home_client::HomeApi;
use mediaops_tui::actions::{Mutation, MutationOutcome, MutationTarget, execute};
use mediaops_tui::cache::ObjectKey;
use mediaops_tui::inventory::open_holds;
use mediaops_tui::{Session, SyncState};

use support::{TestApi, pump_until_current};

#[tokio::test]
async fn apply_and_delete_one_want() {
    let home = TestApi::start("mut-want").await;
    seed_cluster(&home).await;
    let mut session = Session::new(Some(&home.socket));
    session.bootstrap().await;
    pump_until_current(&mut session).await;
    let target = MutationTarget {
        key: ObjectKey::new(Kind::Want, "movie:tmdb:603"),
        uid: String::new(),
        resource_version: 0,
        epoch: session.cache.epoch(),
    };
    let api = session.api.clone().expect("api");
    assert_eq!(
        execute(&api, Mutation::ApplyWant, &target, &session.cache, 0).await,
        MutationOutcome::Applied
    );
    session.bootstrap().await;
    pump_until_current(&mut session).await;
    assert_eq!(session.cache.live_kind(Kind::Want).count(), 1);
    let want = session.cache.live_kind(Kind::Want).next().expect("want");
    let target = MutationTarget {
        key: ObjectKey::new(Kind::Want, want.metadata.name.clone()),
        uid: want.metadata.uid.clone(),
        resource_version: want.metadata.resource_version,
        epoch: session.cache.epoch(),
    };
    assert_eq!(
        execute(&api, Mutation::DeleteWant, &target, &session.cache, 0).await,
        MutationOutcome::Deleted
    );
}

#[tokio::test]
async fn hold_actions_target_exact_release() {
    let home = TestApi::start("mut-hold").await;
    seed_cluster(&home).await;
    let now = unix_now();
    publish_inventory(&home, now, 3).await;
    let inventory = HomeApi::connect(&home.socket, Actor::Inventory)
        .await
        .expect("inv");
    for (name, release) in [("movie:tmdb:1-one", "one"), ("movie:tmdb:1-two", "two")] {
        inventory
            .apply(hold_obj(name, "movie:tmdb:1", release, 3))
            .await
            .expect("hold");
    }
    let mut session = Session::new(Some(&home.socket));
    session.bootstrap().await;
    pump_until_current(&mut session).await;
    assert_eq!(session.sync, SyncState::Current);
    let objects: Vec<_> = session.cache.live().cloned().collect();
    let inbox = open_holds(objects.iter(), 3);
    assert_eq!(inbox.len(), 2);
    let chosen = inbox
        .iter()
        .find(|h| h.metadata.name.ends_with("-two"))
        .expect("two");
    let target = MutationTarget {
        key: ObjectKey::new(Kind::Hold, chosen.metadata.name.clone()),
        uid: chosen.metadata.uid.clone(),
        resource_version: chosen.metadata.resource_version,
        epoch: session.cache.epoch(),
    };
    let api = session.api.clone().expect("api");
    assert_eq!(
        execute(&api, Mutation::ApproveHold, &target, &session.cache, now).await,
        MutationOutcome::Applied
    );
    let holds = home.api.list(Some(Kind::Hold)).await.expect("list");
    let two = holds
        .iter()
        .find(|h| h.metadata.name.ends_with("-two"))
        .expect("two");
    let one = holds
        .iter()
        .find(|h| h.metadata.name.ends_with("-one"))
        .expect("one");
    assert!(matches!(
        &two.spec,
        Spec::Hold(spec) if spec.decision == HoldDecisionSpec::Approved
    ));
    assert!(matches!(
        &one.spec,
        Spec::Hold(spec) if spec.decision == HoldDecisionSpec::Empty
    ));
}

#[tokio::test]
async fn stale_uid_does_not_write() {
    let home = TestApi::start("conflict").await;
    seed_cluster(&home).await;
    home.api
        .apply(HomeObject::new(
            Kind::Want,
            "movie:tmdb:7",
            Spec::Want(WantSpec {
                title_id: "movie:tmdb:7".into(),
            }),
            StatusBody::Want(WantStatus::default()),
        ))
        .await
        .expect("want");
    let mut session = Session::new(Some(&home.socket));
    session.bootstrap().await;
    pump_until_current(&mut session).await;
    let want = session.cache.live_kind(Kind::Want).next().expect("want");
    let target = MutationTarget {
        key: ObjectKey::new(Kind::Want, want.metadata.name.clone()),
        uid: "stale-uid".into(),
        resource_version: want.metadata.resource_version,
        epoch: session.cache.epoch(),
    };
    let api = session.api.clone().expect("api");
    assert_eq!(
        execute(&api, Mutation::DeleteWant, &target, &session.cache, 0).await,
        MutationOutcome::Conflict
    );
}

async fn seed_cluster(home: &TestApi) {
    home.api
        .apply(HomeObject::new(
            Kind::Cluster,
            CLUSTER_NAME,
            Spec::Cluster(ClusterSpec::default()),
            StatusBody::Cluster(ClusterStatus::default()),
        ))
        .await
        .expect("cluster");
}

async fn publish_inventory(home: &TestApi, now: i64, generation: i64) {
    let api = HomeApi::connect(&home.socket, Actor::Inventory)
        .await
        .expect("inv");
    api.apply(HomeObject::new(
        Kind::Node,
        "inventory",
        Spec::Node(NodeSpec {
            worker_kind: WorkerKind::Inventory,
        }),
        StatusBody::Node(NodeStatus {
            list_generation: generation,
            list_completed_unix: now,
            ready: true,
            last_heartbeat_unix: now,
        }),
    ))
    .await
    .expect("node");
}

fn hold_obj(name: &str, title: &str, release: &str, generation: i64) -> HomeObject {
    HomeObject::new(
        Kind::Hold,
        name,
        Spec::Hold(HoldSpec {
            title_id: title.into(),
            release_id: release.into(),
            decision: HoldDecisionSpec::Empty,
        }),
        StatusBody::Hold(HoldStatus {
            list_generation: generation,
            release: release.into(),
            size: 1000,
            placement: Some(Placement::movie("Hearts", 1991, "mkv")),
            ..HoldStatus::default()
        }),
    )
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
