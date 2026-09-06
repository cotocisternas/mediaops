//! Overview mix and Titles union.

use mediaops_core::{JobPhase, Kind, Spec, StatusBody, WantPhase};

use super::{ListingKind, NOTHING_HAPPENING, Projection, TableRow, row};
use crate::cache::ObjectCache;

pub(crate) fn overview(cache: &ObjectCache, selected: usize, now_unix: i64) -> Projection {
    let mut rows = Vec::new();
    for obj in cache.live_kind(Kind::Want) {
        if matches!(&obj.status, StatusBody::Want(st) if st.phase == WantPhase::Open) {
            let title = match &obj.spec {
                Spec::Want(spec) => spec.title_id.clone(),
                _ => obj.metadata.name.clone(),
            };
            rows.push(row(obj, vec!["want".into(), title, "open".into()]));
        }
    }
    for obj in cache.live_kind(Kind::Job) {
        let StatusBody::Job(st) = &obj.status else {
            continue;
        };
        if matches!(st.phase, JobPhase::Installed) {
            continue;
        }
        let title = match &obj.spec {
            Spec::Job(spec) if !spec.title_id.is_empty() => spec.title_id.clone(),
            _ => obj.metadata.name.clone(),
        };
        let kind = if matches!(st.phase, JobPhase::Failed | JobPhase::Refused) {
            "fail"
        } else {
            "job"
        };
        rows.push(row(
            obj,
            vec![kind.into(), title, st.phase.as_str().to_string()],
        ));
    }
    for (name, ready) in super::worker_readiness(cache, now_unix) {
        let fact = if ready { "ready" } else { "not-ready" };
        if let Some(obj) = cache
            .live_kind(Kind::Node)
            .find(|o| o.metadata.name == name)
        {
            rows.push(row(obj, vec!["node".into(), name.to_string(), fact.into()]));
        }
    }
    let work = rows
        .iter()
        .any(|r| r.kind == Kind::Want || r.kind == Kind::Job);
    let listing = if work || !rows.is_empty() {
        ListingKind::Rows
    } else {
        ListingKind::KnownEmpty(NOTHING_HAPPENING)
    };
    let detail = overview_detail(cache, &rows, selected, now_unix);
    Projection {
        listing,
        rows,
        headers: vec!["KIND", "TITLE", "FACT"],
        detail,
        hold_caption: false,
    }
}

fn overview_detail(
    cache: &ObjectCache,
    rows: &[TableRow],
    selected: usize,
    now_unix: i64,
) -> Vec<super::DetailLine> {
    let Some(row) = rows.get(selected) else {
        return Vec::new();
    };
    match row.kind {
        Kind::Want => cache
            .get(&crate::cache::ObjectKey::new(row.kind, row.name.clone()))
            .and_then(|e| e.object.as_ref())
            .map(super::detail::want_detail)
            .unwrap_or_default(),
        Kind::Job => super::detail::job_detail_for(cache, rows, selected),
        Kind::Node => super::detail::node_detail_for(cache, rows, selected, now_unix),
        _ => Vec::new(),
    }
}

pub(crate) fn titles(cache: &ObjectCache, selected: usize, now_unix: i64) -> Projection {
    let ids = super::facts::title_union(cache, now_unix);
    let names = super::names::Names::from_cache(cache, now_unix);
    let rows: Vec<TableRow> = ids
        .into_iter()
        .map(|id| TableRow {
            identity: id.clone(),
            cells: vec![names.get(&id)],
            uid: String::new(),
            rv: 0,
            kind: Kind::Title,
            name: id,
        })
        .collect();
    let listing = if rows.is_empty() {
        ListingKind::KnownEmpty(NOTHING_HAPPENING)
    } else {
        ListingKind::Rows
    };
    let mut detail = rows
        .get(selected)
        .map(|r| super::facts::why_facts(cache, &r.name, now_unix))
        .unwrap_or_default();
    if let Some(row) = rows.get(selected) {
        detail.insert(1, super::line("title", &row.cells[0]));
    }
    Projection {
        listing,
        rows,
        headers: vec!["TITLE"],
        detail,
        hold_caption: false,
    }
}
