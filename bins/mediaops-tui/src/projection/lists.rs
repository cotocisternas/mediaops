//! Per-screen row projections.

use mediaops_core::{HomeObject, JobPhase, Kind, Spec, StatusBody, node_is_ready};

use super::{
    ListingKind, NOTHING_HAPPENING, NOTHING_ON_HOLD, NOTHING_ON_THE_BOX, Projection, TableRow, row,
    unavailable,
};
use crate::cache::ObjectCache;
use crate::format::{fmt_age, fmt_bytes, fmt_observed_age, fmt_percent};
use crate::inventory::{committed_inventory_generation, current_remote_files, open_holds};

pub(crate) fn wants(cache: &ObjectCache, now_unix: i64) -> Projection {
    let names = super::names::Names::from_cache(cache, now_unix);
    let mut rows: Vec<TableRow> = cache
        .live_kind(Kind::Want)
        .map(|obj| {
            let phase = match &obj.status {
                StatusBody::Want(st) => st.phase.as_str(),
                _ => "",
            };
            let title = match &obj.spec {
                Spec::Want(spec) => names.get(&spec.title_id),
                _ => obj.metadata.name.clone(),
            };
            row(obj, vec![title, phase.to_string()])
        })
        .collect();
    rows.sort_by(|a, b| a.identity.cmp(&b.identity));
    let listing = if rows.is_empty() {
        ListingKind::KnownEmpty(NOTHING_HAPPENING)
    } else {
        ListingKind::Rows
    };
    Projection {
        listing,
        rows,
        headers: vec!["TITLE", "PHASE"],
        detail: Vec::new(),
        hold_caption: false,
    }
}

pub(crate) fn jobs(cache: &ObjectCache, active_only: bool, now_unix: i64) -> Projection {
    let names = super::names::Names::from_cache(cache, now_unix);
    let mut rows: Vec<TableRow> = cache
        .live_kind(Kind::Job)
        .filter(|obj| match &obj.status {
            StatusBody::Job(st) if active_only => !matches!(st.phase, JobPhase::Installed),
            StatusBody::Job(_) => true,
            _ => false,
        })
        .map(|obj| {
            let (phase, progress, bytes, attempts, node, message) = match (&obj.spec, &obj.status) {
                (Spec::Job(spec), StatusBody::Job(st)) => (
                    st.phase.as_str().to_string(),
                    fmt_percent(st.bytes_done, spec.file_len),
                    fmt_bytes(st.bytes_done),
                    st.attempts.to_string(),
                    if spec.node_name.is_empty() {
                        "unbound".into()
                    } else {
                        spec.node_name.clone()
                    },
                    st.message.clone(),
                ),
                _ => (
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                ),
            };
            let title = match &obj.spec {
                Spec::Job(spec) if !spec.title_id.is_empty() => names.get(&spec.title_id),
                _ => obj.metadata.name.clone(),
            };
            row(
                obj,
                vec![title, phase, progress, bytes, attempts, node, message],
            )
        })
        .collect();
    rows.sort_by(|a, b| a.identity.cmp(&b.identity));
    let listing = if rows.is_empty() {
        ListingKind::KnownEmpty(NOTHING_HAPPENING)
    } else {
        ListingKind::Rows
    };
    Projection {
        listing,
        rows,
        headers: vec![
            "TITLE", "PHASE", "PROGRESS", "BYTES", "ATTEMPTS", "NODE", "MESSAGE",
        ],
        detail: Vec::new(),
        hold_caption: false,
    }
}

pub(crate) fn holds(cache: &ObjectCache, now_unix: i64) -> Projection {
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
    Projection {
        listing,
        rows,
        headers: vec!["TITLE", "SIZE", "AGE"],
        detail: Vec::new(),
        hold_caption: true,
    }
}

pub(crate) fn nodes(cache: &ObjectCache, now_unix: i64) -> Projection {
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
            let heartbeat = match &obj.status {
                StatusBody::Node(st) => fmt_observed_age(st.last_heartbeat_unix, now_unix),
                _ => "never".into(),
            };
            row(
                obj,
                vec![obj.metadata.name.clone(), ready.to_string(), heartbeat],
            )
        })
        .collect();
    rows.sort_by(|a, b| a.identity.cmp(&b.identity));
    let listing = if rows.is_empty() {
        ListingKind::Unavailable
    } else {
        ListingKind::Rows
    };
    Projection {
        listing,
        rows,
        headers: vec!["NODE", "READY", "HEARTBEAT"],
        detail: Vec::new(),
        hold_caption: false,
    }
}

pub(crate) fn box_listing(cache: &ObjectCache, now_unix: i64) -> Projection {
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
    Projection {
        listing,
        rows,
        headers: vec!["ROOT", "PATH", "BYTES"],
        detail: Vec::new(),
        hold_caption: false,
    }
}
