//! Per-screen row projections.

use mediaops_core::{HomeObject, JobPhase, Kind, Spec, StatusBody, node_is_ready};

use super::{
    ListingKind, NOTHING_HAPPENING, NOTHING_ON_HOLD, NOTHING_ON_THE_BOX, Projection, TableRow, row,
    unavailable,
};
use crate::cache::ObjectCache;
use crate::format::{fmt_age, fmt_bytes};
use crate::inventory::{committed_inventory_generation, current_remote_files, open_holds};

pub(crate) fn wants(cache: &ObjectCache, selected: usize) -> Projection {
    let mut rows: Vec<TableRow> = cache
        .live_kind(Kind::Want)
        .map(|obj| {
            let phase = match &obj.status {
                StatusBody::Want(st) => st.phase.as_str(),
                _ => "",
            };
            row(obj, vec![obj.metadata.name.clone(), phase.to_string()])
        })
        .collect();
    rows.sort_by(|a, b| a.identity.cmp(&b.identity));
    let listing = if rows.is_empty() {
        ListingKind::KnownEmpty(NOTHING_HAPPENING)
    } else {
        ListingKind::Rows
    };
    let detail = rows
        .get(selected)
        .and_then(|r| {
            cache
                .get(&crate::cache::ObjectKey::new(r.kind, r.name.clone()))
                .and_then(|e| e.object.as_ref())
                .map(super::detail::want_detail)
        })
        .unwrap_or_default();
    Projection {
        listing,
        rows,
        headers: vec!["TITLE", "PHASE"],
        detail,
        hold_caption: false,
    }
}

pub(crate) fn jobs(cache: &ObjectCache, selected: usize, active_only: bool) -> Projection {
    let mut rows: Vec<TableRow> = cache
        .live_kind(Kind::Job)
        .filter(|obj| match &obj.status {
            StatusBody::Job(st) if active_only => !matches!(st.phase, JobPhase::Installed),
            StatusBody::Job(_) => true,
            _ => false,
        })
        .map(|obj| {
            let (phase, bytes, attempts, node, failure) = match (&obj.spec, &obj.status) {
                (Spec::Job(spec), StatusBody::Job(st)) => (
                    st.phase.as_str().to_string(),
                    fmt_bytes(st.bytes_done),
                    st.attempts.to_string(),
                    spec.node_name.clone(),
                    st.message.clone(),
                ),
                _ => (
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                ),
            };
            let title = match &obj.spec {
                Spec::Job(spec) if !spec.title_id.is_empty() => spec.title_id.clone(),
                _ => obj.metadata.name.clone(),
            };
            row(obj, vec![title, phase, bytes, attempts, node, failure])
        })
        .collect();
    rows.sort_by(|a, b| a.identity.cmp(&b.identity));
    let listing = if rows.is_empty() {
        ListingKind::KnownEmpty(NOTHING_HAPPENING)
    } else {
        ListingKind::Rows
    };
    let detail = super::detail::job_detail_for(cache, &rows, selected);
    Projection {
        listing,
        rows,
        headers: vec!["TITLE", "PHASE", "BYTES", "ATTEMPTS", "NODE", "FAILURE"],
        detail,
        hold_caption: false,
    }
}

pub(crate) fn holds(cache: &ObjectCache, selected: usize, now_unix: i64) -> Projection {
    let names = super::names::Names::from_cache(cache, now_unix);
    let objects: Vec<&HomeObject> = cache.live().collect();
    let Some(generation) = committed_inventory_generation(objects.iter().copied(), now_unix) else {
        return unavailable(vec!["TITLE", "SIZE", "AGE"]);
    };
    let mut rows: Vec<TableRow> = open_holds(objects.iter().copied(), generation)
        .into_iter()
        .map(|obj| {
            let (size, age) = match &obj.status {
                StatusBody::Hold(st) => (
                    fmt_bytes(st.size),
                    if st.added_unix > 0 && now_unix >= st.added_unix {
                        fmt_age((now_unix - st.added_unix) as u64)
                    } else {
                        String::new()
                    },
                ),
                _ => (String::new(), String::new()),
            };
            let title = match (&obj.spec, &obj.status) {
                (Spec::Hold(spec), StatusBody::Hold(status)) => {
                    super::names::hold_name(status, &spec.title_id, &names)
                }
                _ => obj.metadata.name.clone(),
            };
            row(obj, vec![title, size, age])
        })
        .collect();
    rows.sort_by(|a, b| a.identity.cmp(&b.identity));
    let listing = if rows.is_empty() {
        ListingKind::KnownEmpty(NOTHING_ON_HOLD)
    } else {
        ListingKind::Rows
    };
    let mut detail = super::detail::hold_detail_for(cache, &rows, selected);
    if let Some(row) = rows.get(selected) {
        detail.insert(1, super::line("title", &row.cells[0]));
    }
    Projection {
        listing,
        rows,
        headers: vec!["TITLE", "SIZE", "AGE"],
        detail,
        hold_caption: true,
    }
}

pub(crate) fn nodes(cache: &ObjectCache, selected: usize, now_unix: i64) -> Projection {
    let mut rows: Vec<TableRow> = cache
        .live_kind(Kind::Node)
        .map(|obj| {
            let ready = match (&obj.spec, &obj.status) {
                (Spec::Node(_), StatusBody::Node(st)) => {
                    if node_is_ready(st.ready, st.last_heartbeat_unix, now_unix) {
                        "ready"
                    } else {
                        "not-ready"
                    }
                }
                _ => "",
            };
            row(obj, vec![obj.metadata.name.clone(), ready.to_string()])
        })
        .collect();
    rows.sort_by(|a, b| a.identity.cmp(&b.identity));
    let listing = if rows.is_empty() {
        ListingKind::Unavailable
    } else {
        ListingKind::Rows
    };
    let detail = super::detail::node_detail_for(cache, &rows, selected, now_unix);
    Projection {
        listing,
        rows,
        headers: vec!["NODE", "READY"],
        detail,
        hold_caption: false,
    }
}

pub(crate) fn box_listing(cache: &ObjectCache, selected: usize, now_unix: i64) -> Projection {
    let objects: Vec<&HomeObject> = cache.live().collect();
    let Some(generation) = committed_inventory_generation(objects.iter().copied(), now_unix) else {
        return unavailable(vec!["ROOT", "PATH", "BYTES"]);
    };
    let mut rows: Vec<TableRow> = current_remote_files(objects.iter().copied(), generation)
        .into_iter()
        .map(|obj| {
            let (root, path, len) = match &obj.status {
                StatusBody::RemoteFile(st) => {
                    (st.root_id.clone(), st.rel_path.clone(), fmt_bytes(st.len))
                }
                _ => (String::new(), String::new(), String::new()),
            };
            row(obj, vec![root, path, len])
        })
        .collect();
    rows.sort_by(|a, b| a.identity.cmp(&b.identity));
    let listing = if rows.is_empty() {
        ListingKind::KnownEmpty(NOTHING_ON_THE_BOX)
    } else {
        ListingKind::Rows
    };
    let detail = super::detail::remotefile_detail_for(cache, &rows, selected);
    Projection {
        listing,
        rows,
        headers: vec!["ROOT", "PATH", "BYTES"],
        detail,
        hold_caption: false,
    }
}
