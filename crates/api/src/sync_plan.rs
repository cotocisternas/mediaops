use std::collections::HashMap;
use std::path::Path;

use mediaops_core::{
    ClusterSpec, FileKey, HoldDecisionSpec, HomeObject, JobSpec, Placement, RemoteFileStatus,
    RemoteRef, Spec, StatusBody, SyncDisposition, SyncEntry, TitleId, WorkerKind,
};

pub(crate) fn job_name(id: &TitleId, placement: &Placement) -> String {
    let key = match placement.file_key() {
        FileKey::Whole => "whole".into(),
        FileKey::Episode { season, episode } => format!("s{season}-e{episode}"),
        FileKey::Track { disc, track } => format!("d{disc}-t{track}"),
    };
    format!("pull-{}-{key}", id.staging_token())
}

pub(crate) fn capture(objects: &[HomeObject], request: &HomeObject) -> Vec<SyncEntry> {
    let StatusBody::Sync(status) = &request.status else {
        return Vec::new();
    };
    let Some(cluster) = &status.cluster else {
        return Vec::new();
    };
    let holds: Vec<_> = objects.iter().filter(|obj| matches!(&obj.status, StatusBody::Hold(st) if st.list_generation == status.list_generation)).collect();
    let mut entries: Vec<_> = objects
        .iter()
        .filter_map(|obj| match &obj.status {
            StatusBody::RemoteFile(remote) if remote.list_generation == status.list_generation => {
                Some(candidate(remote, &holds, cluster, &request.metadata.name))
            }
            _ => None,
        })
        .collect();
    entries.sort_by(|a, b| (&a.remote_root, &a.remote_path).cmp(&(&b.remote_root, &b.remote_path)));
    let mut destinations = HashMap::new();
    for entry in &entries {
        if entry.job.is_some() {
            *destinations.entry(entry.job_name.clone()).or_insert(0usize) += 1;
        }
    }
    for entry in &mut entries {
        if destinations
            .get(&entry.job_name)
            .is_some_and(|count| *count > 1)
        {
            block(entry, "multiple sources target the same library placement");
        }
    }
    entries
}

fn candidate(
    remote: &RemoteFileStatus,
    holds: &[&HomeObject],
    cluster: &ClusterSpec,
    name: &str,
) -> SyncEntry {
    let mut entry = SyncEntry {
        remote_root: remote.root_id.clone(),
        remote_path: remote.rel_path.clone(),
        file_len: remote.len,
        disposition: SyncDisposition::Ineligible,
        ..Default::default()
    };
    let Some(root) = cluster.roots.iter().find(|root| root.id == remote.root_id) else {
        entry.reason = "root is not configured in Cluster".into();
        return entry;
    };
    let Ok(reference) =
        RemoteRef::from_wire_parts(remote.root_id.clone(), remote.rel_path.clone().into())
    else {
        entry.reason = "unsafe remote path".into();
        return entry;
    };
    if remote.len == 0 || !mediaops_core::is_media_file(&reference) {
        entry.reason = "not completed nonempty library media".into();
        return entry;
    }
    if holds
        .iter()
        .filter(|hold| {
            matches!(&hold.status, StatusBody::Hold(status)
        if status.remote_root == remote.root_id && status.remote_path == remote.rel_path)
        })
        .count()
        > 1
    {
        block(
            &mut entry,
            "conflicting Hold records refer to the same source",
        );
        return entry;
    }
    let approved: Vec<_> = holds.iter().filter(|hold| matches!((&hold.spec, &hold.status), (Spec::Hold(spec), StatusBody::Hold(status))
        if spec.decision == HoldDecisionSpec::Approved && status.remote_root == remote.root_id && status.remote_path == remote.rel_path)).collect();
    let classified = if approved.len() == 1 {
        let hold = approved[0];
        match (&hold.spec, &hold.status) {
            (Spec::Hold(spec), StatusBody::Hold(status)) => {
                TitleId::parse(&spec.title_id).ok().and_then(|id| {
                    status
                        .placement
                        .as_ref()
                        .and_then(|placement| {
                            mediaops_core::preflight_approve_placement(&id, placement).ok()
                        })
                        .map(|placement| (id, placement, hold.metadata.name.clone()))
                })
            }
            _ => None,
        }
    } else if approved.is_empty()
        && remote.parse_ok
        && !crate::controllers::held_remote(holds, remote, &remote.title_id)
    {
        mediaops_core::parse_remote(root.kind, Path::new(&remote.rel_path))
            .ok()
            .map(|(id, placement)| (id, placement, String::new()))
    } else {
        block(
            &mut entry,
            "source requires an explicit, unambiguous Hold decision",
        );
        return entry;
    };
    let Some((authority, placement, hold_name)) = classified else {
        entry.reason = "no safe canonical placement".into();
        return entry;
    };
    let Ok(dest) = mediaops_core::render(&authority, &placement) else {
        entry.reason = "placement cannot be rendered safely".into();
        return entry;
    };
    let Ok(id) = mediaops_core::parse(&dest) else {
        entry.reason = "destination identity is invalid".into();
        return entry;
    };
    entry.title_id = id.render();
    entry.job_name = job_name(&id, &placement);
    entry.placement = Some(placement);
    entry.job = Some(JobSpec {
        sync_name: name.into(),
        hold_name,
        title_id: entry.title_id.clone(),
        remote_root: remote.root_id.clone(),
        remote_path: remote.rel_path.clone(),
        file_len: remote.len,
        dest_rel: dest.to_string_lossy().into_owned(),
        library_root: cluster.library_root.clone(),
        range_len: cluster.range_len.get(),
        range_concurrency: cluster.range_concurrency.unwrap_or(1),
        max_copy: cluster.max_copy.get(),
        min_free: cluster.min_free.get(),
        worker_kind: WorkerKind::Pull.as_str().into(),
        ..Default::default()
    });
    entry.disposition = SyncDisposition::WouldQueue;
    entry
}

pub(crate) fn evaluate(entries: &mut [SyncEntry], objects: &[HomeObject], now: i64) {
    for entry in entries {
        if entry.disposition != SyncDisposition::WouldQueue {
            continue;
        }
        let Some(job) = entry.job.clone() else {
            block(entry, "missing Job snapshot");
            continue;
        };
        if let Some((disposition, reason)) = crate::sync_disk::placement_state(objects, &job) {
            entry.disposition = disposition;
            entry.reason = reason;
            continue;
        }
        if let Some(existing) = conflicting_job(objects, &job, &entry.job_name) {
            if let (Spec::Job(spec), StatusBody::Job(status)) = (&existing.spec, &existing.status) {
                if !status.phase.is_terminal()
                    && spec.remote_root == job.remote_root
                    && spec.remote_path == job.remote_path
                    && spec.file_len == job.file_len
                    && spec.library_root == job.library_root
                {
                    entry.disposition = SyncDisposition::AlreadyQueued;
                    entry.job_name.clone_from(&existing.metadata.name);
                    entry.job_uid.clone_from(&existing.metadata.uid);
                    entry.reason = "existing Job retains its original authorization".into();
                } else {
                    block(
                        entry,
                        "existing terminal or conflicting Job; explicit retry is required",
                    );
                }
            }
            continue;
        }
        if let Some(reason) = crate::controllers::authorization_refusal(objects, &job, now) {
            block(entry, reason);
        }
    }
}

fn conflicting_job<'a>(
    objects: &'a [HomeObject],
    candidate: &JobSpec,
    name: &str,
) -> Option<&'a HomeObject> {
    let placement = mediaops_core::parse_placement(Path::new(&candidate.dest_rel)).ok()?;
    objects.iter().find(|obj| match &obj.spec {
        Spec::Job(job) => {
            obj.metadata.name == name
                || mediaops_core::parse_placement(Path::new(&job.dest_rel)).is_ok_and(|(id, p)| {
                    id == placement.0 && p.file_key() == placement.1.file_key()
                })
        }
        _ => false,
    })
}

fn block(entry: &mut SyncEntry, reason: &str) {
    entry.disposition = SyncDisposition::Blocked;
    entry.reason = reason.into();
}
