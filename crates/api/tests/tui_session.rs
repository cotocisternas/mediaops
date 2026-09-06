mod support;

use mediaops_core::{
    CLUSTER_NAME, ClusterSpec, ClusterStatus, HomeObject, Kind, Spec, StatusBody, WantSpec,
    WantStatus,
};
use mediaops_tui::Session;
use mediaops_tui::model::Screen;
use mediaops_tui::projection::{ListingKind, NOTHING_HAPPENING, project};

use support::{TestApi, pump_until_current};

#[tokio::test]
async fn empty_successful_baseline_is_known_empty() {
    let home = TestApi::start("empty").await;
    seed_cluster(&home).await;
    let mut session = Session::new(Some(&home.socket));
    session.bootstrap().await;
    pump_until_current(&mut session).await;
    let p = project(&session.cache, Screen::Wants, 0, 0);
    assert_eq!(p.listing, ListingKind::KnownEmpty(NOTHING_HAPPENING));
}

#[tokio::test]
async fn watch_then_list_sees_applied_want() {
    let home = TestApi::start("want").await;
    seed_cluster(&home).await;
    home.api
        .apply(HomeObject::new(
            Kind::Want,
            "movie:tmdb:603",
            Spec::Want(WantSpec {
                title_id: "movie:tmdb:603".into(),
            }),
            StatusBody::Want(WantStatus::default()),
        ))
        .await
        .expect("want");
    let mut session = Session::new(Some(&home.socket));
    session.bootstrap().await;
    pump_until_current(&mut session).await;
    let p = project(&session.cache, Screen::Wants, 0, 0);
    assert_eq!(p.rows.len(), 1);
    assert_eq!(p.rows[0].name, "movie:tmdb:603");
}

#[tokio::test]
async fn snapshot_larger_than_channel_capacity_does_not_block_bootstrap() {
    let home = TestApi::start("large").await;
    for n in 0..150 {
        let name = format!("movie:tmdb:{n}");
        home.api
            .apply(HomeObject::new(
                Kind::Want,
                name.clone(),
                Spec::Want(WantSpec { title_id: name }),
                StatusBody::empty(Kind::Want),
            ))
            .await
            .expect("seed");
    }
    let mut session = Session::new(Some(&home.socket));
    tokio::time::timeout(std::time::Duration::from_secs(2), session.bootstrap())
        .await
        .expect("bootstrap must not await its receiver");
    pump_until_current(&mut session).await;
    assert_eq!(session.cache.live_kind(Kind::Want).count(), 150);
}

#[tokio::test]
async fn quiet_objects_older_than_compacted_history_bootstrap_from_zero() {
    let home = TestApi::start("compacted").await;
    let ancient = home
        .api
        .apply(HomeObject::new(
            Kind::Want,
            "movie:tmdb:1",
            Spec::Want(WantSpec {
                title_id: "movie:tmdb:1".into(),
            }),
            StatusBody::empty(Kind::Want),
        ))
        .await
        .expect("seed");
    for n in 2..2060 {
        let name = format!("movie:tmdb:{n}");
        let transient = home
            .api
            .apply(HomeObject::new(
                Kind::Want,
                name.clone(),
                Spec::Want(WantSpec {
                    title_id: name.clone(),
                }),
                StatusBody::empty(Kind::Want),
            ))
            .await
            .expect("transient");
        home.api
            .delete_at_version(Kind::Want, &name, transient.metadata.resource_version)
            .await
            .expect("delete");
    }
    let mut session = Session::new(Some(&home.socket));
    session.bootstrap().await;
    pump_until_current(&mut session).await;
    assert_eq!(
        session
            .cache
            .live_kind(Kind::Want)
            .next()
            .expect("ancient")
            .metadata
            .uid,
        ancient.metadata.uid
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
