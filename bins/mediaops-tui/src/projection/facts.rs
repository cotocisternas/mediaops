//! Title union and why-facts. Current inventory generation only.

use mediaops_core::{HoldDecisionSpec, HomeObject, Kind, Spec, StatusBody, TitleId, WantPhase};

use super::DetailLine;
use crate::cache::ObjectCache;
use crate::format::fmt_bytes;
use crate::inventory::{committed_inventory_generation, current_remote_files, open_holds};

pub(crate) fn title_id_of(obj: &HomeObject) -> Option<String> {
    let raw = match (&obj.spec, &obj.status) {
        (Spec::Want(spec), _) => spec.title_id.as_str(),
        (Spec::Hold(spec), _) => spec.title_id.as_str(),
        (Spec::Job(spec), _) => spec.title_id.as_str(),
        (Spec::Title(spec), _) => spec.title_id.as_str(),
        (_, StatusBody::RemoteFile(st)) => st.title_id.as_str(),
        _ => "",
    };
    if raw.is_empty() {
        return None;
    }
    TitleId::parse(raw).ok().map(|id| id.render())
}

pub(crate) fn title_union(cache: &ObjectCache, now_unix: i64) -> Vec<String> {
    let objects: Vec<&HomeObject> = cache.live().collect();
    let generation = committed_inventory_generation(objects.iter().copied(), now_unix);
    let mut ids = Vec::new();
    for obj in objects {
        if let Some(id) = union_id(obj, generation) {
            ids.push(id);
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

fn union_id(obj: &HomeObject, generation: Option<i64>) -> Option<String> {
    match (&obj.spec, &obj.status) {
        (Spec::Want(_), _) | (Spec::Job(_), _) | (Spec::Title(_), _) => title_id_of(obj),
        (Spec::Hold(spec), StatusBody::Hold(st)) => {
            let generation = generation?;
            if spec.decision != HoldDecisionSpec::Empty || st.list_generation != generation {
                return None;
            }
            title_id_of(obj)
        }
        (_, StatusBody::RemoteFile(st)) => {
            let generation = generation?;
            if !st.parse_ok || st.list_generation != generation {
                return None;
            }
            title_id_of(obj)
        }
        _ => None,
    }
}

pub(crate) fn why_facts(cache: &ObjectCache, title_id: &str, now_unix: i64) -> Vec<DetailLine> {
    let mut lines = vec![super::line("title_id", title_id)];
    let objects: Vec<&HomeObject> = cache.live().collect();
    let generation = committed_inventory_generation(objects.iter().copied(), now_unix);
    if let Some(generation) = generation {
        for hold in open_holds(objects.iter().copied(), generation) {
            if let (Spec::Hold(spec), StatusBody::Hold(st)) = (&hold.spec, &hold.status)
                && spec.title_id == title_id
            {
                lines.push(super::line(
                    "hold",
                    &format!(
                        "{}  {}",
                        crate::sanitize::sanitize(&st.reason),
                        fmt_bytes(st.size)
                    ),
                ));
            }
        }
    }
    let open_want = objects.iter().any(|obj| {
        matches!((&obj.spec, &obj.status), (Spec::Want(spec), StatusBody::Want(st))
            if spec.title_id == title_id && st.phase == WantPhase::Open)
    });
    if let Some(generation) = generation {
        let listed = current_remote_files(objects.iter().copied(), generation)
            .into_iter()
            .any(|obj| match &obj.status {
                StatusBody::RemoteFile(st) => {
                    st.parse_ok && st.title_id == title_id && TitleId::parse(&st.title_id).is_ok()
                }
                _ => false,
            });
        if open_want && listed {
            lines.push(super::line("want", "open, listed on the box"));
        }
        if open_want && !listed {
            lines.push(super::line("grab", "wanted, not on the box"));
        }
    }
    for obj in cache.live_kind(Kind::Job) {
        if let (Spec::Job(spec), StatusBody::Job(st)) = (&obj.spec, &obj.status)
            && spec.title_id == title_id
        {
            lines.push(super::line("pull", st.phase.as_str()));
        }
    }
    for obj in cache.live_kind(Kind::Title) {
        if let (Spec::Title(spec), StatusBody::Title(st)) = (&obj.spec, &obj.status)
            && spec.title_id == title_id
        {
            let files = st.observed_files();
            if files.iter().any(|f| f.drifted) {
                lines.push(super::line("library", "drifted"));
            } else if let Some(file) = files.first() {
                lines.push(super::line("library", &file.path));
            }
        }
    }
    if lines.len() == 1 {
        lines.push(super::line("quiet", ""));
    }
    lines
}
