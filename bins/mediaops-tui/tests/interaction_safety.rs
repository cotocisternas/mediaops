use mediaops_core::Kind;
use mediaops_tui::actions::Mutation;
use mediaops_tui::cache::ObjectKey;
use mediaops_tui::keys::Command;
use mediaops_tui::model::{Screen, SyncState, UiModel};
use mediaops_tui::update::{Update, UpdateEffect, apply};

#[test]
fn help_blocks_hidden_detail_mutation() {
    let mut ui = UiModel {
        screen: Screen::Wants,
        in_detail: true,
        help: true,
        selected_key: Some(ObjectKey::new(Kind::Want, "movie:tmdb:1")),
        ..UiModel::default()
    };
    let result = apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Current,
            row_count: 1,
            page: 10,
        },
        Command::Mutate(Mutation::DeleteWant),
    );
    assert!(matches!(result, UpdateEffect::None));
}

#[test]
fn write_bindings_are_exactly_the_approved_screen_allowlist() {
    for screen in Screen::ALL {
        assert_eq!(
            Mutation::ApplyWant.allowed_on(screen),
            matches!(screen, Screen::Wants | Screen::Titles)
        );
        assert_eq!(
            Mutation::DeleteWant.allowed_on(screen),
            screen == Screen::Wants
        );
        assert_eq!(
            Mutation::ApproveHold.allowed_on(screen),
            screen == Screen::Holds
        );
        assert_eq!(
            Mutation::RejectHold.allowed_on(screen),
            screen == Screen::Holds
        );
    }
}

fn want(name: &str, uid: &str, rv: i64) -> mediaops_core::HomeObject {
    let mut obj = mediaops_core::HomeObject::new(
        Kind::Want,
        name,
        mediaops_core::Spec::Want(mediaops_core::WantSpec {
            title_id: name.into(),
        }),
        mediaops_core::StatusBody::empty(Kind::Want),
    );
    obj.metadata.uid = uid.into();
    obj.metadata.resource_version = rv;
    obj
}

#[test]
fn insertion_before_selection_preserves_identity_and_deletion_disarms_detail() {
    let mut session = mediaops_tui::Session::new(None);
    let epoch = session.cache.bump_epoch();
    session
        .cache
        .install_baseline(epoch, vec![want("movie:tmdb:2", "two", 1)]);
    session.sync = SyncState::Current;
    let mut ui = UiModel {
        screen: Screen::Wants,
        in_detail: true,
        ..UiModel::default()
    };
    mediaops_tui::interaction::project_ui(&session, &mut ui);
    session.cache.apply_event(
        epoch,
        mediaops_home_client::WatchEvent::Added(want("movie:tmdb:1", "one", 2)),
    );
    mediaops_tui::interaction::project_ui(&session, &mut ui);
    assert_eq!(ui.selected, 1);
    assert_eq!(ui.rendered_target.as_ref().expect("target").uid, "two");
    session.cache.apply_event(
        epoch,
        mediaops_home_client::WatchEvent::Deleted(want("movie:tmdb:2", "two", 3)),
    );
    mediaops_tui::interaction::project_ui(&session, &mut ui);
    assert!(!ui.in_detail);
    assert!(ui.rendered_target.is_none());
}

#[test]
fn rapid_navigation_invalidates_last_rendered_target() {
    let mut session = mediaops_tui::Session::new(None);
    let epoch = session.cache.bump_epoch();
    session
        .cache
        .install_baseline(epoch, vec![want("movie:tmdb:1", "one", 1)]);
    session.sync = SyncState::Current;
    let mut ui = UiModel {
        screen: Screen::Wants,
        in_detail: true,
        ..UiModel::default()
    };
    mediaops_tui::interaction::project_ui(&session, &mut ui);
    assert!(ui.rendered_target.is_some());
    apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Current,
            row_count: 1,
            page: 10,
        },
        Command::Screen(Screen::Titles),
    );
    apply(
        Update {
            ui: &mut ui,
            sync: SyncState::Current,
            row_count: 1,
            page: 10,
        },
        Command::EnterDetail,
    );
    assert!(ui.rendered_target.is_none());
}

#[test]
fn titles_capture_existing_want_revision_instead_of_synthetic_title_uid() {
    let mut session = mediaops_tui::Session::new(None);
    let epoch = session.cache.bump_epoch();
    session
        .cache
        .install_baseline(epoch, vec![want("movie:tmdb:1", "one", 42)]);
    session.sync = SyncState::Current;
    let mut ui = UiModel {
        screen: Screen::Titles,
        in_detail: true,
        ..UiModel::default()
    };
    mediaops_tui::interaction::project_ui(&session, &mut ui);
    let target = ui.rendered_target.expect("target");
    assert_eq!(target.key.kind, Kind::Want);
    assert_eq!(target.uid, "one");
    assert_eq!(target.resource_version, 42);
}
