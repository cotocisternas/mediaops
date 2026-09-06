use mediaops_core::{HomeObject, JobSpec, Kind, Spec, StatusBody, SyncDisposition, SyncPhase};

pub(crate) fn identity_refusal(objects: &[HomeObject], job: &HomeObject) -> Option<&'static str> {
    let Spec::Job(spec) = &job.spec else {
        return Some("Sync authority requires a Job");
    };
    if spec.sync_name.is_empty() {
        return None;
    }
    let request = objects
        .iter()
        .find(|obj| obj.kind == Kind::Sync && obj.metadata.name == spec.sync_name);
    let Some(StatusBody::Sync(status)) = request.map(|obj| &obj.status) else {
        return Some("authorizing Sync was deleted or is missing");
    };
    if status.phase != SyncPhase::Scheduled {
        return Some("Sync has not committed its Job manifest");
    }
    let current = objects.iter().find_map(|obj| match &obj.spec {
        Spec::Cluster(config) if obj.metadata.name == mediaops_core::CLUSTER_NAME => Some(config),
        _ => None,
    });
    let Some((captured, current)) = status.cluster.as_ref().zip(current) else {
        return Some("Sync configuration is missing");
    };
    if captured.library_root != current.library_root
        || captured
            .roots
            .iter()
            .find(|root| root.id == spec.remote_root)
            != current
                .roots
                .iter()
                .find(|root| root.id == spec.remote_root)
        || crate::sync::secret_revision(objects) != status.secret_resource_version
    {
        return Some("Sync source configuration changed");
    }
    if !status.entries.iter().any(|entry| {
        entry.disposition == SyncDisposition::Queued
            && entry.job_name == job.metadata.name
            && entry.job_uid == job.metadata.uid
            && entry
                .job
                .as_ref()
                .is_some_and(|captured| same_snapshot(captured, spec))
    }) {
        return Some("Job identity or snapshot is not authorized by this Sync");
    }
    None
}

fn same_snapshot(left: &JobSpec, right: &JobSpec) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.node_name.clear();
    right.node_name.clear();
    left.worker_kind.clear();
    right.worker_kind.clear();
    left == right
}

pub(crate) fn hold_conflict(holds: &[&HomeObject], job: &JobSpec, authorizer: &HomeObject) -> bool {
    let Spec::Hold(approved) = &authorizer.spec else {
        return true;
    };
    let target = mediaops_core::parse_placement(std::path::Path::new(&job.dest_rel)).ok();
    holds.iter().any(|hold| {
        if hold.metadata.name == authorizer.metadata.name {
            return false;
        }
        match (&hold.spec, &hold.status) {
            (Spec::Hold(spec), StatusBody::Hold(status)) => {
                (status.remote_root == job.remote_root && status.remote_path == job.remote_path)
                    || (status.remote_path.is_empty()
                        && (spec.title_id == approved.title_id
                            || status.placement.as_ref().is_some_and(|placement| {
                                target.as_ref().is_some_and(|(id, p)| {
                                    placement.file_key() == p.file_key()
                                        && placement.key_title_id().is_ok_and(|key| &key == id)
                                })
                            })))
            }
            _ => false,
        }
    })
}
