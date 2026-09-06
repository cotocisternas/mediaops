//! Detail panes.

use mediaops_core::{HomeObject, Kind, Spec, StatusBody, node_is_ready};

use super::{DetailLine, TableRow, line};
use crate::cache::ObjectCache;
use crate::format::fmt_bytes;

pub(crate) fn want_detail(obj: &HomeObject) -> Vec<DetailLine> {
    let title = match &obj.spec {
        Spec::Want(spec) => spec.title_id.as_str(),
        _ => "",
    };
    let phase = match &obj.status {
        StatusBody::Want(st) => st.phase.as_str(),
        _ => "",
    };
    vec![
        line("name", &obj.metadata.name),
        line("title_id", title),
        line("phase", phase),
        line("uid", &obj.metadata.uid),
        line(
            "resourceVersion",
            &obj.metadata.resource_version.to_string(),
        ),
    ]
}

pub(crate) fn job_detail_for(
    cache: &ObjectCache,
    rows: &[TableRow],
    selected: usize,
) -> Vec<DetailLine> {
    rows.get(selected)
        .and_then(|r| {
            cache
                .live_kind(Kind::Job)
                .find(|o| o.metadata.name == r.name)
        })
        .map(|obj| {
            let (title, node, dest, total) = match &obj.spec {
                Spec::Job(spec) => (
                    spec.title_id.as_str(),
                    spec.node_name.as_str(),
                    spec.dest_rel.as_str(),
                    spec.file_len,
                ),
                _ => ("", "", "", 0),
            };
            let (phase, done, attempts, failure) = match &obj.status {
                StatusBody::Job(st) => (
                    st.phase.as_str(),
                    st.bytes_done,
                    st.attempts.to_string(),
                    st.message.as_str(),
                ),
                _ => ("", 0, String::new(), ""),
            };
            let bytes = format!("{} / {}", fmt_bytes(done), fmt_bytes(total));
            vec![
                line("name", &obj.metadata.name),
                line("title_id", title),
                line("phase", phase),
                line("bytes", &bytes),
                line("attempts", &attempts),
                line("node", node),
                line("failure", failure),
                line("dest", dest),
            ]
        })
        .unwrap_or_default()
}

pub(crate) fn hold_detail_for(
    cache: &ObjectCache,
    rows: &[TableRow],
    selected: usize,
) -> Vec<DetailLine> {
    rows.get(selected)
        .and_then(|r| {
            cache
                .live_kind(Kind::Hold)
                .find(|o| o.metadata.name == r.name)
        })
        .map(|obj| {
            let (title, release_id) = match &obj.spec {
                Spec::Hold(spec) => (spec.title_id.as_str(), spec.release_id.as_str()),
                _ => ("", ""),
            };
            let (size, reason, release, generation) = match &obj.status {
                StatusBody::Hold(st) => (
                    fmt_bytes(st.size),
                    st.reason.as_str(),
                    st.release.as_str(),
                    st.list_generation.to_string(),
                ),
                _ => (String::new(), "", "", String::new()),
            };
            vec![
                line("name", &obj.metadata.name),
                line("title_id", title),
                line("release_id", release_id),
                line("size", &size),
                line("reason", reason),
                line("release", release),
                line("generation", &generation),
            ]
        })
        .unwrap_or_default()
}

pub(crate) fn node_detail_for(
    cache: &ObjectCache,
    rows: &[TableRow],
    selected: usize,
    now_unix: i64,
) -> Vec<DetailLine> {
    rows.get(selected)
        .and_then(|r| {
            cache
                .live_kind(Kind::Node)
                .find(|o| o.metadata.name == r.name)
        })
        .map(|obj| {
            let worker = match &obj.spec {
                Spec::Node(spec) => spec.worker_kind.as_str(),
                _ => "",
            };
            let (ready, generation, beat) = match &obj.status {
                StatusBody::Node(st) => (
                    if node_is_ready(st.ready, st.last_heartbeat_unix, now_unix) {
                        "ready"
                    } else {
                        "not-ready"
                    },
                    st.list_generation.to_string(),
                    st.last_heartbeat_unix.to_string(),
                ),
                _ => ("", String::new(), String::new()),
            };
            vec![
                line("name", &obj.metadata.name),
                line("worker", worker),
                line("ready", ready),
                line("list_generation", &generation),
                line("heartbeat", &beat),
            ]
        })
        .unwrap_or_default()
}

pub(crate) fn remotefile_detail_for(
    cache: &ObjectCache,
    rows: &[TableRow],
    selected: usize,
) -> Vec<DetailLine> {
    rows.get(selected)
        .and_then(|r| {
            cache
                .live_kind(Kind::RemoteFile)
                .find(|o| o.metadata.name == r.name)
        })
        .map(|obj| {
            let st = match &obj.status {
                StatusBody::RemoteFile(st) => st,
                _ => {
                    return vec![line("name", &obj.metadata.name)];
                }
            };
            vec![
                line("name", &obj.metadata.name),
                line("root", &st.root_id),
                line("path", &st.rel_path),
                line("bytes", &fmt_bytes(st.len)),
                line("title_id", &st.title_id),
                line("generation", &st.list_generation.to_string()),
            ]
        })
        .unwrap_or_default()
}
