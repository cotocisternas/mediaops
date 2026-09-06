mod support;

use mediaops_core::{HomeObject, Kind, Spec, StatusBody, WantSpec};
use mediaops_tui::actions::{Mutation, MutationOutcome, MutationTarget, execute, prepare, submit};
use mediaops_tui::cache::ObjectKey;
use mediaops_tui::{Session, SessionEvent};
use support::{TestApi, pump_until_current};

fn new_want() -> HomeObject {
    HomeObject::new(
        Kind::Want,
        "movie:tmdb:603",
        Spec::Want(WantSpec {
            title_id: "movie:tmdb:603".into(),
        }),
        StatusBody::empty(Kind::Want),
    )
}

fn target(obj: &HomeObject, session: &Session) -> MutationTarget {
    MutationTarget {
        key: ObjectKey::from_object(obj),
        uid: obj.metadata.uid.clone(),
        resource_version: obj.metadata.resource_version,
        epoch: session.cache.epoch(),
    }
}

#[tokio::test]
async fn existing_want_removed_during_preflight_is_not_recreated() {
    let home = TestApi::start("want-removed").await;
    let want = home.api.apply(new_want()).await.expect("want");
    let mut session = Session::new(Some(&home.socket));
    session.bootstrap().await;
    pump_until_current(&mut session).await;
    let chosen = target(&want, &session);
    home.api
        .delete_at_version(
            Kind::Want,
            &want.metadata.name,
            want.metadata.resource_version,
        )
        .await
        .expect("concurrent delete");
    assert_eq!(
        execute(&home.api, Mutation::ApplyWant, &chosen, &session.cache, 0).await,
        MutationOutcome::Conflict
    );
    assert!(
        home.api
            .get(Kind::Want, &want.metadata.name)
            .await
            .expect_err("not recreated")
            .is_not_found()
    );
}

#[tokio::test]
async fn versioned_delete_does_not_substitute_a_newer_revision() {
    let home = TestApi::start("version-delete").await;
    let want = home.api.apply(new_want()).await.expect("want");
    let newer = home
        .api
        .apply(want.clone())
        .await
        .expect("advance revision");
    let err = home
        .api
        .delete_at_version(
            Kind::Want,
            &want.metadata.name,
            want.metadata.resource_version,
        )
        .await
        .expect_err("conflict");
    assert!(err.is_conflict());
    assert_eq!(
        home.api
            .get(Kind::Want, &want.metadata.name)
            .await
            .expect("preserved")
            .metadata
            .resource_version,
        newer.metadata.resource_version
    );
}

#[tokio::test]
async fn preflight_read_failure_is_not_an_uncertain_write() {
    let home = TestApi::start("bad-target").await;
    let mut session = Session::new(Some(&home.socket));
    session.bootstrap().await;
    pump_until_current(&mut session).await;
    let chosen = MutationTarget {
        key: ObjectKey::new(Kind::Job, "pull-task"),
        uid: String::new(),
        resource_version: 0,
        epoch: session.cache.epoch(),
    };
    assert!(matches!(
        prepare(&home.api, Mutation::ApplyWant, &chosen).await,
        Err(MutationOutcome::Unavailable)
    ));
}

#[tokio::test]
async fn prepared_write_does_not_force_over_concurrent_change() {
    let home = TestApi::start("prepared-conflict").await;
    let want = home.api.apply(new_want()).await.expect("want");
    let mut session = Session::new(Some(&home.socket));
    session.bootstrap().await;
    pump_until_current(&mut session).await;
    let prepared = prepare(&home.api, Mutation::DeleteWant, &target(&want, &session))
        .await
        .expect("prepare");
    home.api.apply(want).await.expect("concurrent update");
    assert_eq!(submit(&home.api, prepared).await, MutationOutcome::Conflict);
}

#[tokio::test]
async fn disconnected_epoch_invalidates_rendered_preflight_authorization() {
    let home = TestApi::start("epoch-action").await;
    home.api.apply(new_want()).await.expect("want");
    let mut session = Session::new(Some(&home.socket));
    session.bootstrap().await;
    pump_until_current(&mut session).await;
    let mut ui = mediaops_tui::UiModel {
        screen: mediaops_tui::Screen::Wants,
        in_detail: true,
        ..Default::default()
    };
    mediaops_tui::interaction::project_ui(&session, &mut ui);
    let chosen = ui.rendered_target.clone().expect("rendered target");
    assert!(mediaops_tui::interaction::can_submit(
        &session, &ui, &chosen
    ));
    session.apply_event(SessionEvent::WatchEnded {
        epoch: chosen.epoch,
    });
    assert!(!mediaops_tui::interaction::can_submit(
        &session, &ui, &chosen
    ));
}
