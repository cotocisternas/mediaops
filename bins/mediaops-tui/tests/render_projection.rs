use mediaops_core::{
    HoldDecisionSpec, HoldSpec, HoldStatus, HomeObject, JobPhase, JobSpec, JobStatus, Kind,
    NodeSpec, NodeStatus, RemoteFileStatus, Spec, StatusBody, WantSpec, WantStatus, WorkerKind,
};
use mediaops_tui::cache::ObjectCache;
use mediaops_tui::model::Screen;
use mediaops_tui::projection::{ListingKind, project};

fn epoch_cache(objects: Vec<HomeObject>) -> ObjectCache {
    let mut cache = ObjectCache::default();
    let epoch = cache.bump_epoch();
    cache.install_baseline(epoch, objects);
    cache
}

fn inventory(now: i64, generation: i64) -> HomeObject {
    HomeObject::new(
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
    )
}

#[test]
fn overview_lists_open_wants_not_empty_string() {
    let cache = epoch_cache(vec![HomeObject::new(
        Kind::Want,
        "movie:tmdb:603",
        Spec::Want(WantSpec {
            title_id: "movie:tmdb:603".into(),
        }),
        StatusBody::Want(WantStatus::default()),
    )]);
    let p = project(&cache, Screen::Overview, 0, 0);
    assert_eq!(p.listing, ListingKind::Rows);
    assert!(p.rows.iter().any(|r| r.cells[0] == "want"));
    assert!(p.headers.contains(&"KIND"));
}

#[test]
fn overview_includes_readiness_when_nodes_exist() {
    let cache = epoch_cache(vec![inventory(20, 1)]);
    let p = project(&cache, Screen::Overview, 0, 20);
    assert!(p.rows.iter().any(|r| r.cells[0] == "node"));
}

#[test]
fn titles_exclude_archived_hold_and_stale_or_unparsed_files() {
    let now = 30i64;
    let archived = HomeObject::new(
        Kind::Hold,
        "movie:tmdb:9-old",
        Spec::Hold(HoldSpec {
            title_id: "movie:tmdb:9".into(),
            release_id: "old".into(),
            decision: HoldDecisionSpec::Approved,
        }),
        StatusBody::Hold(HoldStatus {
            list_generation: 1,
            ..HoldStatus::default()
        }),
    );
    let stale = HomeObject::new(
        Kind::RemoteFile,
        "movies/stale.mkv",
        Spec::RemoteFile,
        StatusBody::RemoteFile(RemoteFileStatus {
            title_id: "movie:tmdb:8".into(),
            parse_ok: true,
            list_generation: 99,
            ..RemoteFileStatus::default()
        }),
    );
    let unparsed = HomeObject::new(
        Kind::RemoteFile,
        "movies/bad.mkv",
        Spec::RemoteFile,
        StatusBody::RemoteFile(RemoteFileStatus {
            title_id: "movie:tmdb:7".into(),
            parse_ok: false,
            list_generation: 1,
            ..RemoteFileStatus::default()
        }),
    );
    let want = HomeObject::new(
        Kind::Want,
        "movie:tmdb:603",
        Spec::Want(WantSpec {
            title_id: "movie:tmdb:603".into(),
        }),
        StatusBody::Want(WantStatus::default()),
    );
    let cache = epoch_cache(vec![inventory(now, 1), archived, stale, unparsed, want]);
    let p = project(&cache, Screen::Titles, 0, now);
    let ids: Vec<_> = p.rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(ids, vec!["movie:tmdb:603"]);
}

#[test]
fn why_facts_omit_grab_when_inventory_unavailable() {
    let cache = epoch_cache(vec![HomeObject::new(
        Kind::Want,
        "movie:tmdb:603",
        Spec::Want(WantSpec {
            title_id: "movie:tmdb:603".into(),
        }),
        StatusBody::Want(WantStatus::default()),
    )]);
    let p = project(&cache, Screen::Titles, 0, 0);
    assert!(!p.detail.iter().any(|l| l.label == "grab"));
}

#[test]
fn job_detail_shows_title_and_progress() {
    let job = HomeObject::new(
        Kind::Job,
        "pull-1",
        Spec::Job(JobSpec {
            title_id: "movie:tmdb:603".into(),
            file_len: 6_800_000_000,
            ..JobSpec::default()
        }),
        StatusBody::Job(JobStatus {
            phase: JobPhase::Pulling,
            bytes_done: 1_200_000_000,
            ..JobStatus::default()
        }),
    );
    let cache = epoch_cache(vec![job]);
    let p = project(&cache, Screen::Jobs, 0, 0);
    let title = p
        .detail
        .iter()
        .find(|l| l.label == "title_id")
        .expect("title");
    assert_eq!(title.value, "movie:tmdb:603");
    let bytes = p.detail.iter().find(|l| l.label == "bytes").expect("bytes");
    assert!(bytes.value.contains('/'), "{}", bytes.value);
}
