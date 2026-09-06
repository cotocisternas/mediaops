//! Wire ↔ domain conversions for `mediaops.home.v1`. Status construction lives here.

use mediaops_core::{
    Blake3Hex, Bytes, CLUSTER_NAME, ClusterSpec, ClusterStatus, EventStatus, ExitCode, Grabber,
    HOME_API_VERSION, HoldDecisionSpec, HoldSpec, HoldStatus, HomeError, HomeJobKind, HomeObject,
    JobPhase, JobSpec, JobStatus, Kind, NodeSpec, NodeStatus, ObjectMeta, PathRoot,
    RemoteFileStatus, SECRET_NAME, SecretSpec, Spec, StatusBody, SyncDisposition, SyncEntry,
    SyncPhase, SyncScope, SyncSpec, SyncStatus, TitleFileStatus, TitleId, TitleKind, TitleSpec,
    TitleStatus, WantPhase, WantSpec, WantStatus, WorkerKind,
};

use crate::home::{
    Cluster, ClusterSpec as WireClusterSpec, ClusterStatus as WireClusterStatus, Event,
    EventStatus as WireEventStatus, Hold, HoldSpec as WireHoldSpec, HoldStatus as WireHoldStatus,
    Job, JobSpec as WireJobSpec, JobStatus as WireJobStatus, Metadata, Node,
    NodeSpec as WireNodeSpec, NodeStatus as WireNodeStatus, Object, PathRoot as WirePathRoot,
    RemoteFile, RemoteFileStatus as WireRemoteFileStatus, Secret, SecretSpec as WireSecretSpec,
    Sync, SyncEntry as WireSyncEntry, SyncSpec as WireSyncSpec, SyncStatus as WireSyncStatus,
    Title, TitleSpec as WireTitleSpec, TitleStatus as WireTitleStatus, Want,
    WantSpec as WireWantSpec, WantStatus as WireWantStatus, object::Body,
};
use crate::{ErrorDetail, status_from_error_detail};

/// Map a Home domain error to a gRPC Status (ADV-8: construction only here).
pub fn home_status(err: HomeError) -> tonic::Status {
    let (reason, exit) = match &err {
        HomeError::NotFound { .. } => ("not_found", ExitCode::Usage),
        HomeError::AlreadyExists { .. } => ("already_exists", ExitCode::Usage),
        HomeError::Conflict { .. } => ("conflict", ExitCode::Usage),
        HomeError::Expired { .. } => ("expired", ExitCode::Usage),
        HomeError::Invalid(_) => ("invalid", ExitCode::Usage),
        HomeError::Denied(_) => ("denied", ExitCode::PolicyRefusal),
    };
    let detail = ErrorDetail {
        exit_code: i32::from(exit),
        reason: reason.to_string(),
        message: err.to_string(),
    };
    let mut status = status_from_error_detail(&detail);
    status = match &err {
        HomeError::NotFound { .. } => tonic::Status::with_details(
            tonic::Code::NotFound,
            status.message(),
            status.details().to_vec().into(),
        ),
        HomeError::AlreadyExists { .. } => tonic::Status::with_details(
            tonic::Code::AlreadyExists,
            status.message(),
            status.details().to_vec().into(),
        ),
        HomeError::Conflict { .. } => tonic::Status::with_details(
            tonic::Code::FailedPrecondition,
            status.message(),
            status.details().to_vec().into(),
        ),
        HomeError::Expired { .. } => tonic::Status::with_details(
            tonic::Code::OutOfRange,
            status.message(),
            status.details().to_vec().into(),
        ),
        HomeError::Denied(_) => tonic::Status::with_details(
            tonic::Code::PermissionDenied,
            status.message(),
            status.details().to_vec().into(),
        ),
        HomeError::Invalid(_) => tonic::Status::with_details(
            tonic::Code::InvalidArgument,
            status.message(),
            status.details().to_vec().into(),
        ),
    };
    status
}

/// Domain object → wire Object.
pub fn home_object_to_wire(obj: &HomeObject) -> Object {
    let metadata = Metadata {
        name: obj.metadata.name.clone(),
        uid: obj.metadata.uid.clone(),
        generation: obj.metadata.generation,
        resource_version: obj.metadata.resource_version,
    };
    let body = match (&obj.spec, &obj.status) {
        (Spec::Cluster(spec), StatusBody::Cluster(status)) => Some(Body::Cluster(Cluster {
            spec: Some(WireClusterSpec {
                max_copy: spec.max_copy.get(),
                min_free: spec.min_free.get(),
                range_len: spec.range_len.get(),
                range_concurrency: spec.range_concurrency.unwrap_or(0),
                grabber: grabber_wire(spec.grabber),
                lock: spec.lock,
                encode_pause: spec.encode_pause,
                library_root: spec.library_root.clone(),
                roots: spec.roots.iter().map(path_root_to_wire).collect(),
            }),
            status: Some(WireClusterStatus {
                accepted_generation: status.accepted_generation,
                reconcile_generation: status.reconcile_generation,
            }),
        })),
        (Spec::Secret(spec), _) => Some(Body::Secret(Secret {
            spec: Some(WireSecretSpec {
                seedbox_address: spec.seedbox_address.clone(),
                ca_sha256: spec.ca_sha256.clone(),
                server_sha256: spec.server_sha256.clone(),
                client_sha256: spec.client_sha256.clone(),
            }),
        })),
        (Spec::Title(spec), StatusBody::Title(status)) => Some(Body::Title(Title {
            spec: Some(WireTitleSpec {
                title_id: spec.title_id.clone(),
                desired_present: spec.desired_present,
            }),
            status: Some(WireTitleStatus {
                files: status
                    .files
                    .iter()
                    .map(|f| crate::home::TitleFileStatus {
                        path: f.path.clone(),
                        install_b3: f.install_b3.to_string(),
                        current_b3: f.current_b3.to_string(),
                        drifted: f.drifted,
                    })
                    .collect(),
                path: status.path.clone(),
                install_b3: status
                    .install_b3
                    .as_ref()
                    .map(|d| d.as_str().to_string())
                    .unwrap_or_default(),
                current_b3: status
                    .current_b3
                    .as_ref()
                    .map(|d| d.as_str().to_string())
                    .unwrap_or_default(),
                drifted: status.drifted,
            }),
        })),
        (Spec::Want(spec), StatusBody::Want(status)) => Some(Body::Want(Want {
            spec: Some(WireWantSpec {
                title_id: spec.title_id.clone(),
            }),
            status: Some(WireWantStatus {
                phase: status.phase.as_str().to_string(),
            }),
        })),
        (Spec::Job(spec), StatusBody::Job(status)) => Some(Body::Job(Job {
            spec: Some(job_spec_to_wire(spec)),
            status: Some(WireJobStatus {
                started_unix: status.started_unix,
                verified_b3: status
                    .verified_b3
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                phase: status.phase.as_str().to_string(),
                bytes_done: status.bytes_done,
                attempts: status.attempts,
                message: status.message.clone(),
            }),
        })),
        (Spec::Hold(spec), StatusBody::Hold(status)) => Some(Body::Hold(Hold {
            spec: Some(WireHoldSpec {
                title_id: spec.title_id.clone(),
                release_id: spec.release_id.clone(),
                decision: spec.decision.as_str().to_string(),
            }),
            status: Some(WireHoldStatus {
                list_generation: status.list_generation,
                rejection_observed: status.rejection_observed,
                remote_root: status.remote_root.clone(),
                remote_path: status.remote_path.clone(),
                placement: status.placement.as_ref().map(crate::Placement::from),
                added_unix: status.added_unix,
                reason: status.reason.clone(),
                size: status.size,
                release: status.release.clone(),
            }),
        })),
        (Spec::RemoteFile, StatusBody::RemoteFile(status)) => Some(Body::RemoteFile(RemoteFile {
            status: Some(WireRemoteFileStatus {
                root_id: status.root_id.clone(),
                rel_path: status.rel_path.clone(),
                len: status.len,
                parse_ok: status.parse_ok,
                title_id: status.title_id.clone(),
                list_generation: status.list_generation,
            }),
        })),
        (Spec::Node(spec), StatusBody::Node(status)) => Some(Body::Node(Node {
            spec: Some(WireNodeSpec {
                worker_kind: spec.worker_kind.as_str().to_string(),
            }),
            status: Some(node_status_to_wire(status)),
        })),
        (Spec::Sync(spec), StatusBody::Sync(status)) => Some(Body::Sync(Sync {
            spec: Some(WireSyncSpec {
                scope: spec.scope.as_str().to_string(),
            }),
            status: Some(sync_status_to_wire(status)),
        })),
        (Spec::Event, StatusBody::Event(status)) => Some(Body::Event(Event {
            status: Some(WireEventStatus {
                involved_kind: status.involved_kind.clone(),
                involved_name: status.involved_name.clone(),
                reason: status.reason.clone(),
                message: status.message.clone(),
                ts: status.ts,
            }),
        })),
        _ => None,
    };
    Object {
        api_version: obj.api_version.clone(),
        kind: obj.kind.as_str().to_string(),
        metadata: Some(metadata),
        body,
    }
}

/// Wire Object → domain. Missing body or kind mismatch is Invalid.
pub fn home_object_from_wire(obj: Object) -> Result<HomeObject, HomeError> {
    let kind = Kind::parse(&obj.kind)?;
    let meta = obj.metadata.unwrap_or_default();
    let (spec, status) = match obj.body {
        Some(Body::Cluster(c)) => {
            require_kind(kind, Kind::Cluster)?;
            let spec = c.spec.unwrap_or_default();
            let st = c.status.unwrap_or_default();
            (
                Spec::Cluster(ClusterSpec {
                    max_copy: Bytes::new(spec.max_copy),
                    min_free: Bytes::new(spec.min_free),
                    range_len: Bytes::new(spec.range_len),
                    range_concurrency: if spec.range_concurrency == 0 {
                        None
                    } else {
                        Some(spec.range_concurrency)
                    },
                    grabber: parse_grabber(&spec.grabber)?,
                    lock: spec.lock,
                    encode_pause: spec.encode_pause,
                    library_root: spec.library_root,
                    roots: spec
                        .roots
                        .into_iter()
                        .map(path_root_from_wire)
                        .collect::<Result<Vec<_>, _>>()?,
                }),
                StatusBody::Cluster(ClusterStatus {
                    accepted_generation: st.accepted_generation,
                    reconcile_generation: st.reconcile_generation,
                }),
            )
        }
        Some(Body::Secret(s)) => {
            require_kind(kind, Kind::Secret)?;
            let spec = s.spec.unwrap_or_default();
            (
                Spec::Secret(SecretSpec {
                    seedbox_address: spec.seedbox_address,
                    ca_sha256: spec.ca_sha256,
                    server_sha256: spec.server_sha256,
                    client_sha256: spec.client_sha256,
                }),
                StatusBody::Secret,
            )
        }
        Some(Body::Title(t)) => {
            require_kind(kind, Kind::Title)?;
            let spec = t.spec.unwrap_or_default();
            let st = t.status.unwrap_or_default();
            if !spec.title_id.is_empty() {
                TitleId::parse(&spec.title_id).map_err(|e| HomeError::Invalid(e.to_string()))?;
            }
            (
                Spec::Title(TitleSpec {
                    title_id: spec.title_id,
                    desired_present: spec.desired_present,
                }),
                StatusBody::Title(TitleStatus {
                    files: st
                        .files
                        .into_iter()
                        .map(|f| {
                            Ok(TitleFileStatus {
                                path: f.path,
                                install_b3: Blake3Hex::parse(&f.install_b3)
                                    .map_err(|e| HomeError::Invalid(e.to_string()))?,
                                current_b3: Blake3Hex::parse(&f.current_b3)
                                    .map_err(|e| HomeError::Invalid(e.to_string()))?,
                                drifted: f.drifted,
                            })
                        })
                        .collect::<Result<Vec<_>, HomeError>>()?,
                    path: st.path,
                    install_b3: parse_digest_opt(&st.install_b3)?,
                    current_b3: parse_digest_opt(&st.current_b3)?,
                    drifted: st.drifted,
                }),
            )
        }
        Some(Body::Want(w)) => {
            require_kind(kind, Kind::Want)?;
            let spec = w.spec.unwrap_or_default();
            let st = w.status.unwrap_or_default();
            if spec.title_id.is_empty() {
                return Err(HomeError::Invalid("Want.spec.title_id is required".into()));
            }
            TitleId::parse(&spec.title_id).map_err(|e| HomeError::Invalid(e.to_string()))?;
            (
                Spec::Want(WantSpec {
                    title_id: spec.title_id,
                }),
                StatusBody::Want(WantStatus {
                    phase: if st.phase.is_empty() {
                        WantPhase::Open
                    } else {
                        WantPhase::parse(&st.phase)?
                    },
                }),
            )
        }
        Some(Body::Job(j)) => {
            require_kind(kind, Kind::Job)?;
            let spec = j.spec.unwrap_or_default();
            let st = j.status.unwrap_or_default();
            (
                Spec::Job(job_spec_from_wire(spec)?),
                StatusBody::Job(JobStatus {
                    started_unix: st.started_unix,
                    verified_b3: parse_digest_opt(&st.verified_b3)?,
                    phase: if st.phase.is_empty() {
                        JobPhase::Pending
                    } else {
                        JobPhase::parse(&st.phase)?
                    },
                    bytes_done: st.bytes_done,
                    attempts: st.attempts,
                    message: st.message,
                }),
            )
        }
        Some(Body::Hold(h)) => {
            require_kind(kind, Kind::Hold)?;
            let spec = h.spec.unwrap_or_default();
            let st = h.status.unwrap_or_default();
            (
                Spec::Hold(HoldSpec {
                    title_id: spec.title_id,
                    release_id: spec.release_id,
                    decision: HoldDecisionSpec::parse(&spec.decision)?,
                }),
                StatusBody::Hold(HoldStatus {
                    list_generation: st.list_generation,
                    rejection_observed: st.rejection_observed,
                    remote_root: st.remote_root,
                    remote_path: st.remote_path,
                    placement: st
                        .placement
                        .map(mediaops_core::Placement::try_from)
                        .transpose()
                        .map_err(|e| HomeError::Invalid(e.to_string()))?,
                    added_unix: st.added_unix,
                    reason: st.reason,
                    size: st.size,
                    release: st.release,
                }),
            )
        }
        Some(Body::RemoteFile(r)) => {
            require_kind(kind, Kind::RemoteFile)?;
            let st = r.status.unwrap_or_default();
            (
                Spec::RemoteFile,
                StatusBody::RemoteFile(RemoteFileStatus {
                    root_id: st.root_id,
                    rel_path: st.rel_path,
                    len: st.len,
                    parse_ok: st.parse_ok,
                    title_id: st.title_id,
                    list_generation: st.list_generation,
                }),
            )
        }
        Some(Body::Node(n)) => {
            require_kind(kind, Kind::Node)?;
            let spec = n.spec.unwrap_or_default();
            let st = n.status.unwrap_or_default();
            (
                Spec::Node(NodeSpec {
                    worker_kind: WorkerKind::parse(&spec.worker_kind)?,
                }),
                StatusBody::Node(node_status_from_wire(st)),
            )
        }
        Some(Body::Sync(s)) => {
            require_kind(kind, Kind::Sync)?;
            let spec = s.spec.unwrap_or_default();
            let st = s.status.unwrap_or_default();
            (
                Spec::Sync(SyncSpec {
                    scope: SyncScope::parse(&spec.scope)?,
                }),
                StatusBody::Sync(sync_status_from_wire(st)?),
            )
        }
        Some(Body::Event(e)) => {
            require_kind(kind, Kind::Event)?;
            let st = e.status.unwrap_or_default();
            (
                Spec::Event,
                StatusBody::Event(EventStatus {
                    involved_kind: st.involved_kind,
                    involved_name: st.involved_name,
                    reason: st.reason,
                    message: st.message,
                    ts: st.ts,
                }),
            )
        }
        // Defaulting here would let `apply` with an empty body silently reset a
        // stored object -- a Cluster back to zero budgets, for instance.
        None => {
            return Err(HomeError::Invalid(format!("{kind} object has no body")));
        }
    };
    let name = if meta.name.is_empty() {
        default_name(kind, &spec)
    } else {
        meta.name
    };
    Ok(HomeObject {
        api_version: if obj.api_version.is_empty() {
            HOME_API_VERSION.to_string()
        } else {
            obj.api_version
        },
        kind,
        metadata: ObjectMeta {
            name,
            uid: meta.uid,
            generation: meta.generation,
            resource_version: meta.resource_version,
        },
        spec,
        status,
    })
}

fn job_spec_to_wire(spec: &JobSpec) -> WireJobSpec {
    WireJobSpec {
        hold_name: spec.hold_name.clone(),
        sync_name: spec.sync_name.clone(),
        library_root: spec.library_root.clone(),
        range_concurrency: spec.range_concurrency,
        kind: spec.kind.as_str().to_string(),
        title_id: spec.title_id.clone(),
        remote_root: spec.remote_root.clone(),
        remote_path: spec.remote_path.clone(),
        dest_rel: spec.dest_rel.clone(),
        file_len: spec.file_len,
        range_len: spec.range_len,
        max_copy: spec.max_copy,
        min_free: spec.min_free,
        node_name: spec.node_name.clone(),
        worker_kind: spec.worker_kind.clone(),
    }
}

fn job_spec_from_wire(spec: WireJobSpec) -> Result<JobSpec, HomeError> {
    Ok(JobSpec {
        hold_name: spec.hold_name,
        sync_name: spec.sync_name,
        library_root: spec.library_root,
        range_concurrency: spec.range_concurrency,
        kind: if spec.kind.is_empty() {
            HomeJobKind::Pull
        } else {
            HomeJobKind::parse(&spec.kind)?
        },
        title_id: spec.title_id,
        remote_root: spec.remote_root,
        remote_path: spec.remote_path,
        dest_rel: spec.dest_rel,
        file_len: spec.file_len,
        range_len: spec.range_len,
        max_copy: spec.max_copy,
        min_free: spec.min_free,
        node_name: spec.node_name,
        worker_kind: spec.worker_kind,
    })
}

fn node_status_to_wire(status: &NodeStatus) -> WireNodeStatus {
    WireNodeStatus {
        list_generation: status.list_generation,
        list_completed_unix: status.list_completed_unix,
        ready: status.ready,
        last_heartbeat_unix: status.last_heartbeat_unix,
        scan_started_rv: status.scan_started_rv,
        scan_cluster_generation: status.scan_cluster_generation,
        scan_secret_resource_version: status.scan_secret_resource_version,
    }
}

fn node_status_from_wire(status: WireNodeStatus) -> NodeStatus {
    NodeStatus {
        list_generation: status.list_generation,
        list_completed_unix: status.list_completed_unix,
        ready: status.ready,
        last_heartbeat_unix: status.last_heartbeat_unix,
        scan_started_rv: status.scan_started_rv,
        scan_cluster_generation: status.scan_cluster_generation,
        scan_secret_resource_version: status.scan_secret_resource_version,
    }
}

fn cluster_spec_to_wire(spec: &ClusterSpec) -> WireClusterSpec {
    WireClusterSpec {
        max_copy: spec.max_copy.get(),
        min_free: spec.min_free.get(),
        range_len: spec.range_len.get(),
        range_concurrency: spec.range_concurrency.unwrap_or(0),
        grabber: grabber_wire(spec.grabber),
        lock: spec.lock,
        encode_pause: spec.encode_pause,
        library_root: spec.library_root.clone(),
        roots: spec.roots.iter().map(path_root_to_wire).collect(),
    }
}

fn cluster_spec_from_wire(spec: WireClusterSpec) -> Result<ClusterSpec, HomeError> {
    Ok(ClusterSpec {
        max_copy: Bytes::new(spec.max_copy),
        min_free: Bytes::new(spec.min_free),
        range_len: Bytes::new(spec.range_len),
        range_concurrency: if spec.range_concurrency == 0 {
            None
        } else {
            Some(spec.range_concurrency)
        },
        grabber: parse_grabber(&spec.grabber)?,
        lock: spec.lock,
        encode_pause: spec.encode_pause,
        library_root: spec.library_root,
        roots: spec
            .roots
            .into_iter()
            .map(path_root_from_wire)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn sync_status_to_wire(status: &SyncStatus) -> WireSyncStatus {
    WireSyncStatus {
        phase: status.phase.as_str().to_string(),
        accepted_unix: status.accepted_unix,
        deadline_unix: status.deadline_unix,
        baseline_generation: status.baseline_generation,
        acceptance_rv: status.acceptance_rv,
        list_generation: status.list_generation,
        cluster: status.cluster.as_ref().map(cluster_spec_to_wire),
        cluster_generation: status.cluster_generation,
        secret_resource_version: status.secret_resource_version,
        entries: status.entries.iter().map(sync_entry_to_wire).collect(),
        message: status.message.clone(),
    }
}

fn sync_status_from_wire(status: WireSyncStatus) -> Result<SyncStatus, HomeError> {
    Ok(SyncStatus {
        phase: if status.phase.is_empty() {
            SyncPhase::WaitingInventory
        } else {
            SyncPhase::parse(&status.phase)?
        },
        accepted_unix: status.accepted_unix,
        deadline_unix: status.deadline_unix,
        baseline_generation: status.baseline_generation,
        acceptance_rv: status.acceptance_rv,
        list_generation: status.list_generation,
        cluster: status.cluster.map(cluster_spec_from_wire).transpose()?,
        cluster_generation: status.cluster_generation,
        secret_resource_version: status.secret_resource_version,
        entries: status
            .entries
            .into_iter()
            .map(sync_entry_from_wire)
            .collect::<Result<Vec<_>, _>>()?,
        message: status.message,
    })
}

fn sync_entry_to_wire(entry: &SyncEntry) -> WireSyncEntry {
    WireSyncEntry {
        remote_root: entry.remote_root.clone(),
        remote_path: entry.remote_path.clone(),
        file_len: entry.file_len,
        title_id: entry.title_id.clone(),
        placement: entry.placement.as_ref().map(crate::Placement::from),
        job: entry.job.as_ref().map(job_spec_to_wire),
        disposition: entry.disposition.as_str().to_string(),
        reason: entry.reason.clone(),
        job_name: entry.job_name.clone(),
        job_uid: entry.job_uid.clone(),
    }
}

fn sync_entry_from_wire(entry: WireSyncEntry) -> Result<SyncEntry, HomeError> {
    Ok(SyncEntry {
        remote_root: entry.remote_root,
        remote_path: entry.remote_path,
        file_len: entry.file_len,
        title_id: entry.title_id,
        placement: entry
            .placement
            .map(mediaops_core::Placement::try_from)
            .transpose()
            .map_err(|e| HomeError::Invalid(e.to_string()))?,
        job: entry.job.map(job_spec_from_wire).transpose()?,
        disposition: if entry.disposition.is_empty() {
            SyncDisposition::WouldQueue
        } else {
            SyncDisposition::parse(&entry.disposition)?
        },
        reason: entry.reason,
        job_name: entry.job_name,
        job_uid: entry.job_uid,
    })
}

fn default_name(kind: Kind, spec: &Spec) -> String {
    match (kind, spec) {
        (Kind::Cluster, _) => CLUSTER_NAME.to_string(),
        (Kind::Secret, _) => SECRET_NAME.to_string(),
        (Kind::Want, Spec::Want(w)) => w.title_id.clone(),
        (Kind::Title, Spec::Title(t)) => t.title_id.clone(),
        (Kind::Node, Spec::Node(n)) => n.worker_kind.node_name().to_string(),
        _ => String::new(),
    }
}

fn require_kind(got: Kind, want: Kind) -> Result<(), HomeError> {
    if got == want {
        Ok(())
    } else {
        Err(HomeError::Invalid(format!(
            "kind {got} does not match body {want}"
        )))
    }
}

fn grabber_wire(g: Grabber) -> String {
    match g {
        Grabber::None => "none".into(),
        Grabber::Servarr => "servarr".into(),
    }
}

fn parse_grabber(raw: &str) -> Result<Grabber, HomeError> {
    match raw {
        "" | "none" => Ok(Grabber::None),
        "servarr" => Ok(Grabber::Servarr),
        other => Err(HomeError::Invalid(format!("unknown grabber `{other}`"))),
    }
}

fn path_root_to_wire(root: &PathRoot) -> WirePathRoot {
    WirePathRoot {
        id: root.id.clone(),
        path: root.path.clone(),
        kind: root
            .kind
            .map(|k| k.as_str().to_string())
            .unwrap_or_default(),
    }
}

fn path_root_from_wire(root: WirePathRoot) -> Result<PathRoot, HomeError> {
    let kind = if root.kind.is_empty() {
        None
    } else {
        Some(parse_title_kind(&root.kind)?)
    };
    Ok(PathRoot {
        id: root.id,
        path: root.path,
        kind,
    })
}

fn parse_title_kind(raw: &str) -> Result<TitleKind, HomeError> {
    match raw {
        "movie" => Ok(TitleKind::Movie),
        "series" => Ok(TitleKind::Series),
        "album" => Ok(TitleKind::Album),
        other => Err(HomeError::Invalid(format!("unknown TitleKind `{other}`"))),
    }
}

fn parse_digest_opt(raw: &str) -> Result<Option<Blake3Hex>, HomeError> {
    if raw.is_empty() {
        return Ok(None);
    }
    Blake3Hex::parse(raw)
        .map(Some)
        .map_err(|e| HomeError::Invalid(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn want_round_trip() {
        let obj = HomeObject::new(
            Kind::Want,
            "movie:tmdb:603",
            Spec::Want(WantSpec {
                title_id: "movie:tmdb:603".into(),
            }),
            StatusBody::Want(WantStatus {
                phase: WantPhase::Open,
            }),
        );
        let wire = home_object_to_wire(&obj);
        let back = home_object_from_wire(wire).expect("from");
        assert_eq!(back.kind, Kind::Want);
        match back.spec {
            Spec::Want(w) => assert_eq!(w.title_id, "movie:tmdb:603"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn wire_conversion_does_not_rewrite_an_unsupported_version() {
        let mut object = HomeObject::new(
            Kind::Want,
            "movie:tmdb:603",
            Spec::Want(WantSpec {
                title_id: "movie:tmdb:603".into(),
            }),
            StatusBody::empty(Kind::Want),
        );
        object.api_version = "mediaops.home.v999".into();
        let wire = home_object_to_wire(&object);
        assert_eq!(wire.api_version, object.api_version);
        assert!(
            home_object_from_wire(wire)
                .expect("decode")
                .validate()
                .is_err()
        );
    }

    #[test]
    fn sync_and_scan_fields_round_trip() {
        let obj = HomeObject::new(
            Kind::Sync,
            "req-1",
            Spec::Sync(SyncSpec::default()),
            StatusBody::Sync(SyncStatus {
                phase: SyncPhase::Captured,
                acceptance_rv: 9,
                entries: vec![SyncEntry {
                    remote_root: "movies".into(),
                    disposition: SyncDisposition::WouldQueue,
                    ..SyncEntry::default()
                }],
                ..SyncStatus::default()
            }),
        );
        let back = home_object_from_wire(home_object_to_wire(&obj)).expect("sync");
        assert_eq!(back.kind, Kind::Sync);
        match back.status {
            StatusBody::Sync(status) => {
                assert_eq!(status.phase, SyncPhase::Captured);
                assert_eq!(status.acceptance_rv, 9);
                assert_eq!(status.entries[0].remote_root, "movies");
            }
            other => panic!("{other:?}"),
        }

        let node = HomeObject::new(
            Kind::Node,
            "inventory",
            Spec::Node(NodeSpec {
                worker_kind: WorkerKind::Inventory,
            }),
            StatusBody::Node(NodeStatus {
                scan_started_rv: 4,
                scan_cluster_generation: 2,
                scan_secret_resource_version: 7,
                ..NodeStatus::default()
            }),
        );
        let back = home_object_from_wire(home_object_to_wire(&node)).expect("node");
        match back.status {
            StatusBody::Node(status) => {
                assert_eq!(status.scan_started_rv, 4);
                assert_eq!(status.scan_cluster_generation, 2);
                assert_eq!(status.scan_secret_resource_version, 7);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn empty_sync_name_on_wire_is_old_job_authorization() {
        let spec = JobSpec {
            title_id: "movie:key:thematrix.1999".into(),
            ..JobSpec::default()
        };
        let wire = job_spec_to_wire(&spec);
        assert!(wire.sync_name.is_empty());
        let back = job_spec_from_wire(wire).expect("job");
        assert!(back.sync_name.is_empty());
    }
}
