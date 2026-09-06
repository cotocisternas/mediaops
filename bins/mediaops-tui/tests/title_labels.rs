use mediaops_core::{
    HoldSpec, HoldStatus, HomeObject, Kind, NodeSpec, NodeStatus, Placement, Spec, StatusBody,
    TitleSpec, TitleStatus, WorkerKind,
};
use mediaops_tui::{Screen, cache::ObjectCache, projection::project};

fn cache() -> ObjectCache {
    let mut cache = ObjectCache::default();
    let epoch = cache.bump_epoch();
    cache.install_baseline(
        epoch,
        vec![HomeObject::new(
            Kind::Node,
            "inventory",
            Spec::Node(NodeSpec {
                worker_kind: WorkerKind::Inventory,
            }),
            StatusBody::Node(NodeStatus {
                ready: true,
                list_generation: 1,
                last_heartbeat_unix: 100,
                list_completed_unix: 100,
            }),
        )],
    );
    cache
}

fn hold(name: &str, title_id: &str, title: &str) -> HomeObject {
    HomeObject::new(
        Kind::Hold,
        name,
        Spec::Hold(HoldSpec {
            title_id: title_id.into(),
            release_id: name.into(),
            ..Default::default()
        }),
        StatusBody::Hold(HoldStatus {
            list_generation: 1,
            placement: Some(Placement::movie(title, 1991, "mkv")),
            ..Default::default()
        }),
    )
}

#[test]
fn holds_display_names_but_keep_exact_release_identity() {
    let mut cache = cache();
    for name in ["movie:tmdb:4539-a", "movie:tmdb:4539-b"] {
        cache.apply_event(
            cache.epoch(),
            mediaops_home_client::WatchEvent::Added(hold(
                name,
                "movie:tmdb:4539",
                "Hearts.of.Darkness",
            )),
        );
    }
    let view = project(&cache, Screen::Holds, 0, 100);
    assert_eq!(view.rows.len(), 2);
    assert!(
        view.rows
            .iter()
            .all(|row| row.cells[0] == "Hearts of Darkness (1991)")
    );
    assert_eq!(view.rows[0].name, "movie:tmdb:4539-a");
    assert_eq!(view.rows[1].name, "movie:tmdb:4539-b");
    assert_eq!(view.rows[0].identity, "movie:tmdb:4539-a");
    assert!(
        view.detail
            .iter()
            .any(|line| line.label == "title_id" && line.value == "movie:tmdb:4539")
    );
}

#[test]
fn titles_display_library_names_and_years_without_changing_ids() {
    let mut cache = cache();
    for (id, path) in [
        (
            "album:key:radiohead.amnesiac",
            "music/Radiohead/Amnesiac.(2001)/Disc.01/Amnesiac.(2001).01.Packt.Like.Sardines.in.a.Crushd.Tin.Box.flac",
        ),
        (
            "series:key:mrrobot.2015",
            "series/Mr.Robot.(2015)/Season.01/Mr.Robot.(2015).S01E01.eps1.0.hellofriend.mov.mkv",
        ),
    ] {
        cache.apply_event(
            cache.epoch(),
            mediaops_home_client::WatchEvent::Added(HomeObject::new(
                Kind::Title,
                id,
                Spec::Title(TitleSpec {
                    title_id: id.into(),
                    desired_present: true,
                }),
                StatusBody::Title(TitleStatus {
                    path: path.into(),
                    ..Default::default()
                }),
            )),
        );
    }
    let view = project(&cache, Screen::Titles, 0, 100);
    assert_eq!(view.rows[0].cells[0], "Radiohead / Amnesiac (2001)");
    assert_eq!(view.rows[0].name, "album:key:radiohead.amnesiac");
    assert_eq!(view.rows[1].cells[0], "Mr Robot (2015)");
    assert_eq!(view.rows[1].name, "series:key:mrrobot.2015");
}

#[test]
fn titles_can_use_hold_metadata_for_authority_ids() {
    let mut cache = cache();
    cache.apply_event(
        cache.epoch(),
        mediaops_home_client::WatchEvent::Added(hold(
            "held",
            "movie:tmdb:4539",
            "Hearts.of.Darkness",
        )),
    );
    let view = project(&cache, Screen::Titles, 0, 100);
    assert_eq!(view.rows[0].cells[0], "Hearts of Darkness (1991)");
    assert_eq!(view.rows[0].name, "movie:tmdb:4539");
}

#[test]
fn missing_name_metadata_keeps_the_real_id_instead_of_guessing() {
    let mut cache = cache();
    cache.apply_event(
        cache.epoch(),
        mediaops_home_client::WatchEvent::Added(HomeObject::new(
            Kind::Title,
            "movie:tmdb:999",
            Spec::Title(TitleSpec {
                title_id: "movie:tmdb:999".into(),
                desired_present: true,
            }),
            StatusBody::Title(TitleStatus::default()),
        )),
    );
    let view = project(&cache, Screen::Titles, 0, 100);
    assert_eq!(view.rows[0].cells[0], "movie:tmdb:999");
}

#[test]
fn unplaced_hold_uses_release_name_without_terminal_controls() {
    let mut cache = cache();
    let mut obj = hold("held", "movie:tmdb:999", "unused");
    if let StatusBody::Hold(status) = &mut obj.status {
        status.placement = None;
        status.release = "A.Real.Release.1080p\u{1b}".into();
    }
    cache.apply_event(cache.epoch(), mediaops_home_client::WatchEvent::Added(obj));
    let view = project(&cache, Screen::Holds, 0, 100);
    assert_eq!(view.rows[0].cells[0], "A Real Release 1080p ");
    assert_eq!(view.rows[0].name, "held");
}
