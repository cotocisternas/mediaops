//! Overview mix and Titles union.

use mediaops_core::{JobPhase, Kind, Spec, StatusBody, WantPhase};

use super::{ListingKind, NOTHING_HAPPENING, Projection, TableRow, row};
use crate::cache::ObjectCache;

pub(crate) fn overview(cache: &ObjectCache, now_unix: i64) -> Projection {
    let names = super::names::Names::from_cache(cache, now_unix);
    let mut rows = Vec::new();
    for obj in cache.live_kind(Kind::Cluster) {
        if let Spec::Cluster(spec) = &obj.spec {
            let fact = match (spec.lock, spec.encode_pause) {
                (true, true) => "scheduling/encode paused",
                (true, false) => "scheduling paused",
                (false, true) => "encoding paused",
                (false, false) => "scheduling enabled",
            };
            rows.push(row(
                obj,
                vec!["home".into(), obj.metadata.name.clone(), fact.into()],
            ));
        }
    }
    for obj in cache.live_kind(Kind::Want) {
        if matches!(&obj.status, StatusBody::Want(st) if st.phase == WantPhase::Open) {
            let title = match &obj.spec {
                Spec::Want(spec) => names.get(&spec.title_id),
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
            Spec::Job(spec) if !spec.title_id.is_empty() => names.get(&spec.title_id),
            _ => obj.metadata.name.clone(),
        };
        let kind = if matches!(st.phase, JobPhase::Failed | JobPhase::Refused) {
            "fail"
        } else {
            "job"
        };
        let fact = match &obj.spec {
            Spec::Job(spec) if st.phase == JobPhase::Pulling => format!(
                "pulling {}",
                crate::format::fmt_percent(st.bytes_done, spec.file_len)
            ),
            _ => st.phase.as_str().to_string(),
        };
        rows.push(row(obj, vec![kind.into(), title, fact]));
    }
    for (name, ready) in super::worker_readiness(cache, now_unix) {
        let fact = if ready { "ready" } else { "not-ready" };
        if let Some(obj) = cache
            .live_kind(Kind::Node)
            .find(|o| o.metadata.name == name)
        {
            rows.push(row(obj, vec!["node".into(), name.to_string(), fact.into()]));
        } else if cache.live_kind(Kind::Cluster).next().is_some()
            || cache.live_kind(Kind::Node).next().is_some()
        {
            rows.push(TableRow {
                identity: name.into(),
                cells: vec!["node".into(), name.into(), "missing".into()],
                uid: String::new(),
                rv: 0,
                kind: Kind::Node,
                name: name.into(),
            });
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
    Projection {
        listing,
        rows,
        headers: vec!["KIND", "TITLE", "FACT"],
        detail: Vec::new(),
        hold_caption: false,
    }
}

pub(super) fn overview_detail(
    cache: &ObjectCache,
    rows: &[TableRow],
    selected: usize,
    now_unix: i64,
) -> Vec<super::DetailLine> {
    let Some(row) = rows.get(selected) else {
        return Vec::new();
    };
    match row.kind {
        Kind::Cluster => cache
            .live_kind(Kind::Cluster)
            .find(|obj| obj.metadata.name == row.name)
            .map(super::detail::cluster_detail)
            .unwrap_or_default(),
        Kind::Want => cache
            .get(&crate::cache::ObjectKey::new(row.kind, row.name.clone()))
            .and_then(|e| e.object.as_ref())
            .map(super::detail::want_detail)
            .unwrap_or_default(),
        Kind::Job => super::detail::job_detail_for(cache, rows, selected),
        Kind::Node => {
            let detail = super::detail::node_detail_for(cache, rows, selected, now_unix);
            if detail.is_empty() {
                vec![
                    super::line("name", &row.name),
                    super::line("ready", "missing; no worker registered"),
                    super::line("inspect", "systemctl --user status mediaops-home.service"),
                ]
            } else {
                detail
            }
        }
        _ => Vec::new(),
    }
}

pub(crate) fn titles(cache: &ObjectCache, now_unix: i64) -> Projection {
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
    Projection {
        listing,
        rows,
        headers: vec!["TITLE"],
        detail: Vec::new(),
        hold_caption: false,
    }
}
