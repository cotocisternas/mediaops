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
    NoMatches,
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
    let mut projection = project_rows(cache, screen, now_unix);
    select_detail(cache, screen, &mut projection, selected, now_unix);
    projection
}

fn project_rows(cache: &ObjectCache, screen: Screen, now_unix: i64) -> Projection {
    match screen {
        Screen::Overview => overview::overview(cache, now_unix),
        Screen::Wants => lists::wants(cache, now_unix),
        Screen::Jobs => lists::jobs(cache, false, now_unix),
        Screen::Holds => lists::holds(cache, now_unix),
        Screen::Titles => overview::titles(cache, now_unix),
        Screen::Nodes => lists::nodes(cache, now_unix),
        Screen::BoxListing => lists::box_listing(cache, now_unix),
    }
}

/// Build rows once, then select detail by the retained row's exact object key.
/// Filtering never turns a displayed row index into an unfiltered source index.
pub fn project_filtered(
    cache: &ObjectCache,
    screen: Screen,
    selected: usize,
    now_unix: i64,
    query: &str,
) -> Projection {
    let mut projection = filtered_rows(cache, screen, now_unix, query);
    let selected = selected.min(projection.rows.len().saturating_sub(1));
    select_detail(cache, screen, &mut projection, selected, now_unix);
    projection
}

pub(crate) fn filtered_rows(
    cache: &ObjectCache,
    screen: Screen,
    now_unix: i64,
    query: &str,
) -> Projection {
    let mut projection = project_rows(cache, screen, now_unix);
    if !query.is_empty() {
        // Uppercase maps both Greek sigma forms to Σ, independent of whether
        // the query contains the whole word or only its final letter.
        let query = sanitize(query).to_uppercase();
        projection.rows.retain(|row| {
            sanitize(&row.identity).to_uppercase().contains(&query)
                || row
                    .cells
                    .iter()
                    .any(|cell| sanitize(cell).to_uppercase().contains(&query))
        });
        if projection.rows.is_empty() && projection.listing != ListingKind::Unavailable {
            projection.listing = ListingKind::NoMatches;
        }
    }
    projection
}

pub(crate) fn select_detail(
    cache: &ObjectCache,
    screen: Screen,
    projection: &mut Projection,
    selected: usize,
    now_unix: i64,
) {
    let rows = &projection.rows;
    projection.detail = match screen {
        Screen::Overview => overview::overview_detail(cache, rows, selected, now_unix),
        Screen::Wants => rows
            .get(selected)
            .and_then(|row| {
                cache
                    .get(&crate::cache::ObjectKey::new(row.kind, &row.name))
                    .and_then(|entry| entry.object.as_ref())
                    .map(detail::want_detail)
            })
            .unwrap_or_default(),
        Screen::Jobs => detail::job_detail_for(cache, rows, selected),
        Screen::Holds => detail::hold_detail_for(cache, rows, selected),
        Screen::Titles => rows
            .get(selected)
            .map(|row| facts::why_facts(cache, &row.name, now_unix))
            .unwrap_or_default(),
        Screen::Nodes => detail::node_detail_for(cache, rows, selected, now_unix),
        Screen::BoxListing => detail::remotefile_detail_for(cache, rows, selected),
    };
    if matches!(screen, Screen::Titles | Screen::Holds)
        && let Some(row) = rows.get(selected)
    {
        projection.detail.insert(1, line("title", &row.cells[0]));
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
                ..NodeStatus::default()
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
                ..NodeStatus::default()
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
