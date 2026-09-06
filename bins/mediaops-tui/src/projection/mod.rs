//! Project cache facts onto screens. No invented ETA.

mod detail;
mod facts;
mod lists;
mod names;
mod overview;

use mediaops_core::{HomeObject, Kind, StatusBody, node_is_ready};

use crate::cache::ObjectCache;
use crate::model::Screen;
use crate::sanitize::sanitize;

pub const NOTHING_HAPPENING: &str = "nothing happening";
pub const NOTHING_ON_HOLD: &str = "nothing on hold";
pub const NOTHING_ON_THE_BOX: &str = "nothing on the box";
pub const HOLD_CAPTION: &str = "Approve records a decision; it does not install.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingKind {
    KnownEmpty(&'static str),
    Unavailable,
    Rows,
}

#[derive(Debug, Clone)]
pub struct TableRow {
    pub identity: String,
    pub cells: Vec<String>,
    pub uid: String,
    pub rv: i64,
    pub kind: Kind,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct DetailLine {
    pub label: &'static str,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct Projection {
    pub listing: ListingKind,
    pub rows: Vec<TableRow>,
    pub headers: Vec<&'static str>,
    pub detail: Vec<DetailLine>,
    pub hold_caption: bool,
}

pub fn project(cache: &ObjectCache, screen: Screen, selected: usize, now_unix: i64) -> Projection {
    match screen {
        Screen::Overview => overview::overview(cache, selected, now_unix),
        Screen::Wants => lists::wants(cache, selected),
        Screen::Jobs => lists::jobs(cache, selected, false),
        Screen::Holds => lists::holds(cache, selected, now_unix),
        Screen::Titles => overview::titles(cache, selected, now_unix),
        Screen::Nodes => lists::nodes(cache, selected, now_unix),
        Screen::BoxListing => lists::box_listing(cache, selected, now_unix),
    }
}

pub(crate) fn unavailable(headers: Vec<&'static str>) -> Projection {
    Projection {
        listing: ListingKind::Unavailable,
        rows: Vec::new(),
        headers,
        detail: Vec::new(),
        hold_caption: false,
    }
}

pub(crate) fn row(obj: &HomeObject, cells: Vec<String>) -> TableRow {
    TableRow {
        identity: obj.metadata.name.clone(),
        cells,
        uid: obj.metadata.uid.clone(),
        rv: obj.metadata.resource_version,
        kind: obj.kind,
        name: obj.metadata.name.clone(),
    }
}

pub(crate) fn line(label: &'static str, value: &str) -> DetailLine {
    DetailLine {
        label,
        value: sanitize(value),
    }
}

pub fn worker_readiness(cache: &ObjectCache, now_unix: i64) -> Vec<(&'static str, bool)> {
    ["scheduler", "inventory", "pull"]
        .into_iter()
        .map(|name| {
            let ready = cache.live_kind(Kind::Node).any(|obj| {
                obj.metadata.name == name
                    && matches!(&obj.status, StatusBody::Node(st) if node_is_ready(st.ready, st.last_heartbeat_unix, now_unix))
            });
            (name, ready)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use mediaops_core::{
        HoldSpec, HoldStatus, HomeObject, Kind, NodeSpec, NodeStatus, Spec, StatusBody, WorkerKind,
    };

    use super::*;
    use crate::cache::ObjectCache;
    use crate::model::Screen;

    #[test]
    fn clock_past_freshness_makes_inbox_unavailable() {
        let mut cache = ObjectCache::default();
        let epoch = cache.bump_epoch();
        let listed = 10i64;
        let node = HomeObject::new(
            Kind::Node,
            "inventory",
            Spec::Node(NodeSpec {
                worker_kind: WorkerKind::Inventory,
            }),
            StatusBody::Node(NodeStatus {
                list_generation: 1,
                list_completed_unix: listed,
                ready: true,
                last_heartbeat_unix: listed,
            }),
        );
        cache.install_baseline(
            epoch,
            vec![node, hold_obj("movie:tmdb:1-a", "movie:tmdb:1", "a")],
        );
        let fresh = project(&cache, Screen::Holds, 0, listed);
        assert_eq!(fresh.rows.len(), 1);
        let stale = project(
            &cache,
            Screen::Holds,
            0,
            listed + mediaops_core::NODE_NOTREADY_SECS as i64,
        );
        assert_eq!(stale.listing, ListingKind::Unavailable);
    }

    #[test]
    fn known_empty_wants_use_exact_english() {
        let mut cache = ObjectCache::default();
        let epoch = cache.bump_epoch();
        cache.install_baseline(epoch, Vec::new());
        let p = project(&cache, Screen::Wants, 0, 0);
        assert_eq!(p.listing, ListingKind::KnownEmpty(NOTHING_HAPPENING));
    }

    #[test]
    fn two_holds_same_title_keep_distinct_names() {
        let mut cache = ObjectCache::default();
        let epoch = cache.bump_epoch();
        let now = 20i64;
        let node = HomeObject::new(
            Kind::Node,
            "inventory",
            Spec::Node(NodeSpec {
                worker_kind: WorkerKind::Inventory,
            }),
            StatusBody::Node(NodeStatus {
                list_generation: 1,
                list_completed_unix: now,
                ready: true,
                last_heartbeat_unix: now,
            }),
        );
        let a = hold_obj("movie:tmdb:1-one", "movie:tmdb:1", "one");
        let b = hold_obj("movie:tmdb:1-two", "movie:tmdb:1", "two");
        cache.install_baseline(epoch, vec![node, a, b]);
        let p = project(&cache, Screen::Holds, 0, now);
        assert_eq!(p.rows.len(), 2);
        assert_ne!(p.rows[0].name, p.rows[1].name);
    }

    fn hold_obj(name: &str, title: &str, release: &str) -> HomeObject {
        HomeObject::new(
            Kind::Hold,
            name,
            Spec::Hold(HoldSpec {
                title_id: title.into(),
                release_id: release.into(),
                decision: mediaops_core::HoldDecisionSpec::Empty,
            }),
            StatusBody::Hold(HoldStatus {
                list_generation: 1,
                release: release.into(),
                ..HoldStatus::default()
            }),
        )
    }
}
