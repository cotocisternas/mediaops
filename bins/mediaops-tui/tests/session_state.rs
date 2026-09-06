use mediaops_core::{HomeObject, Kind, Spec, StatusBody, WantSpec, WantStatus};
use mediaops_home_client::WatchEvent;
use mediaops_tui::{Session, SessionEvent, SyncState};

fn want(rv: i64) -> HomeObject {
    let mut obj = HomeObject::new(
        Kind::Want,
        "movie:tmdb:1",
        Spec::Want(WantSpec {
            title_id: "movie:tmdb:1".into(),
        }),
        StatusBody::Want(WantStatus::default()),
    );
    obj.metadata.uid = "u".into();
    obj.metadata.resource_version = rv;
    obj
}

#[test]
fn watch_failure_marks_stale_and_keeps_rows() {
    let mut session = Session::new(None);
    let epoch = session.cache.bump_epoch();
    session.cache.install_baseline(epoch, vec![want(1)]);
    session.sync = SyncState::Current;
    session.apply_event(SessionEvent::WatchFailed {
        epoch,
        message: "eof".into(),
    });
    assert_eq!(session.sync, SyncState::Stale);
    assert_eq!(session.cache.live().count(), 1);
}

#[test]
fn failed_list_is_unavailable_not_empty() {
    let mut session = Session::new(None);
    let epoch = session.cache.bump_epoch();
    session.apply_event(SessionEvent::BaselineFailed {
        epoch,
        message: "list".into(),
    });
    assert!(session.list_failed);
    assert_ne!(session.sync, SyncState::Current);
}

#[test]
fn watch_events_merge_after_baseline() {
    let mut session = Session::new(None);
    let epoch = session.cache.bump_epoch();
    session.apply_event(SessionEvent::Baseline {
        epoch,
        objects: vec![want(1)],
    });
    session.apply_event(SessionEvent::Watch {
        epoch,
        event: Box::new(WatchEvent::Modified(want(2))),
    });
    let obj = session.cache.live().next().expect("row");
    assert_eq!(obj.metadata.resource_version, 2);
    assert_eq!(session.sync, SyncState::Current);
}

#[test]
fn late_baseline_cannot_revive_failed_epoch() {
    let mut session = Session::new(None);
    let epoch = session.cache.bump_epoch();
    session.apply_event(SessionEvent::WatchEnded { epoch });
    session.apply_event(SessionEvent::Baseline {
        epoch,
        objects: vec![want(1)],
    });
    assert_eq!(session.sync, SyncState::Stale);
    assert!(session.needs_reconnect);
}

#[test]
fn baseline_failure_schedules_reconnect() {
    let mut session = Session::new(None);
    let epoch = session.cache.bump_epoch();
    session.apply_event(SessionEvent::BaselineFailed {
        epoch,
        message: "list failed".into(),
    });
    assert!(session.needs_reconnect);
}
