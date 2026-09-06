//! Field ownership and lifecycle checks above the transactional object store.

use mediaops_core::{
    Actor, CLUSTER_NAME, HomeError, HomeObject, HomeOp, JobPhase, Kind, Spec, StatusBody,
    WorkerKind, node_is_ready,
};
use mediaops_store::ApiStore;

fn invalid(message: impl Into<String>) -> HomeError {
    HomeError::Invalid(message.into())
}
fn denied(message: impl Into<String>) -> HomeError {
    HomeError::Denied(message.into())
}

async fn current(store: &ApiStore, obj: &HomeObject) -> Result<HomeObject, HomeError> {
    let old = store
        .get(obj.kind, &obj.metadata.name)
        .await
        .map_err(|e| invalid(e.to_string()))?
        .ok_or_else(|| HomeError::NotFound {
            kind: obj.kind,
            name: obj.metadata.name.clone(),
        })?;
    if old.metadata.resource_version != obj.metadata.resource_version {
        return Err(HomeError::Conflict {
            kind: obj.kind,
            name: obj.metadata.name.clone(),
        });
    }
    Ok(old)
}

fn owns_node(actor: Actor, obj: &HomeObject) -> Result<(), HomeError> {
    if let Spec::Node(spec) = &obj.spec
        && actor.as_str() != spec.worker_kind.as_str()
    {
        return Err(denied("a role may only heartbeat its own Node"));
    }
    Ok(())
}

pub(crate) async fn validate_apply(
    store: &ApiStore,
    actor: Actor,
    obj: &HomeObject,
    verify_disk: bool,
) -> Result<(), HomeError> {
    obj.validate()?;
    owns_node(actor, obj)?;
    let previous = store
        .get(obj.kind, &obj.metadata.name)
        .await
        .map_err(|e| invalid(e.to_string()))?;
    if let Some(old) = previous {
        validate_root_change(store, &old.spec, &obj.spec).await?;
        if obj.metadata.resource_version != old.metadata.resource_version {
            return Err(HomeError::Conflict {
                kind: obj.kind,
                name: obj.metadata.name.clone(),
            });
        }
        if obj.kind == Kind::Job && obj.spec != old.spec {
            return Err(denied("Job snapshot is immutable"));
        }
        if obj.kind == Kind::Hold && obj.spec != old.spec {
            return Err(denied("inventory cannot change a Hold decision"));
        }
        if !obj.spec.is_status_only()
            && obj.status != old.status
            && obj.status != StatusBody::empty(obj.kind)
        {
            return Err(denied(
                "Apply cannot write observed status; use its owned subresource",
            ));
        }
    } else {
        if obj.metadata.resource_version != 0 {
            return Err(HomeError::Conflict {
                kind: obj.kind,
                name: obj.metadata.name.clone(),
            });
        }
        match (&obj.spec, &obj.status) {
            (Spec::Node(_), _) => {}
            (Spec::Hold(spec), _) if actor == Actor::Inventory => {
                if spec.decision != mediaops_core::HoldDecisionSpec::Empty {
                    return Err(denied("inventory cannot decide a Hold"));
                }
            }
            (Spec::RemoteFile | Spec::Event, _) => {}
            (Spec::Title(_), StatusBody::Title(_)) if actor == Actor::Import => {
                verify_title(store, actor, obj, None, verify_disk).await?
            }
            (Spec::Job(spec), _) if !spec.node_name.is_empty() => {
                return Err(denied("new Jobs must be unbound"));
            }
            _ => {
                if obj.status != StatusBody::empty(obj.kind) {
                    return Err(denied("Apply cannot initialize observed status"));
                }
            }
        }
    }
    Ok(())
}

pub(crate) async fn validate_patch(
    store: &ApiStore,
    actor: Actor,
    op: HomeOp,
    obj: &HomeObject,
    verify_disk: bool,
) -> Result<(), HomeError> {
    let old = current(store, obj).await?;
    let mut next = old.clone();
    match op {
        HomeOp::PatchStatus => {
            next.status = obj.status.clone();
            next.validate()?;
            owns_node(actor, &next)?;
            match (&old.spec, &old.status, &next.status) {
                (Spec::Job(spec), StatusBody::Job(before), StatusBody::Job(after)) => {
                    if spec.node_name != WorkerKind::Pull.node_name() || actor != Actor::Pull {
                        return Err(denied("only the bound pull worker may update a Job"));
                    }
                    if before.phase.is_terminal() {
                        return Err(denied("terminal Job status is immutable"));
                    }
                    if after.attempts < before.attempts
                        || after.attempts > before.attempts + 1
                        || (before.started_unix > 0 && after.started_unix != before.started_unix)
                        || (before.verified_b3.is_some() && after.verified_b3 != before.verified_b3)
                    {
                        return Err(denied(
                            "retry deadline and verification proof must be preserved",
                        ));
                    }
                    let transition = before.phase == after.phase
                        || matches!(
                            (before.phase, after.phase),
                            (
                                JobPhase::Pending,
                                JobPhase::Pulling | JobPhase::Refused | JobPhase::Failed
                            ) | (
                                JobPhase::Pulling,
                                JobPhase::Pending
                                    | JobPhase::Verifying
                                    | JobPhase::Failed
                                    | JobPhase::Refused
                            ) | (
                                JobPhase::Verifying,
                                JobPhase::Installed | JobPhase::Failed | JobPhase::Refused
                            )
                        );
                    if !transition {
                        return Err(denied("invalid Job phase transition"));
                    }
                    if matches!(
                        after.phase,
                        JobPhase::Pulling | JobPhase::Verifying | JobPhase::Installed
                    ) && (after.started_unix <= 0 || after.attempts == 0)
                    {
                        return Err(invalid(
                            "active Job requires a persisted attempt and start time",
                        ));
                    }
                    if matches!(after.phase, JobPhase::Verifying | JobPhase::Installed)
                        && after.verified_b3.is_none()
                    {
                        return Err(invalid("verifying Job requires durable whole-file proof"));
                    }
                    if after.phase == JobPhase::Installed {
                        let title = store
                            .get(Kind::Title, &spec.title_id)
                            .await
                            .map_err(|e| invalid(e.to_string()))?;
                        let proved = title.is_some_and(|t| match t.status {
                            StatusBody::Title(st) => st.observed_files().iter().any(|f| {
                                f.path == spec.dest_rel
                                    && !f.drifted
                                    && Some(&f.install_b3) == after.verified_b3.as_ref()
                            }),
                            _ => false,
                        });
                        if !proved || after.bytes_done != spec.file_len {
                            return Err(denied(
                                "Title proof must be durable before Job completion",
                            ));
                        }
                    }
                }
                (Spec::Title(_), _, _) => {
                    verify_title(store, actor, &next, Some(&old), verify_disk).await?
                }
                (Spec::Node(_), StatusBody::Node(before), StatusBody::Node(after))
                    if after.list_generation < before.list_generation
                        || after.list_completed_unix < before.list_completed_unix =>
                {
                    return Err(invalid("inventory publication cannot move backwards"));
                }
                _ => {}
            }
        }
        HomeOp::PatchBind => {
            let (Spec::Job(before), Spec::Job(patch), StatusBody::Job(status)) =
                (&old.spec, &obj.spec, &old.status)
            else {
                return Err(invalid("bind requires Job"));
            };
            if status.phase != JobPhase::Pending
                || !before.node_name.is_empty()
                || patch.node_name != WorkerKind::Pull.node_name()
                || patch.worker_kind != WorkerKind::Pull.as_str()
            {
                return Err(denied(
                    "bind requires an unbound Pending Job and the pull Node",
                ));
            }
            let cluster = store
                .get(Kind::Cluster, CLUSTER_NAME)
                .await
                .map_err(|e| invalid(e.to_string()))?;
            if !matches!(cluster.map(|c| c.spec), Some(Spec::Cluster(c)) if !c.lock && c.library_root == before.library_root)
            {
                return Err(denied(
                    "Cluster is locked, missing, or has another library root",
                ));
            }
            let (objects, _) = store
                .snapshot(None)
                .await
                .map_err(|e| invalid(e.to_string()))?;
            if let Some(reason) =
                crate::controllers::authorization_refusal(&objects, before, unix_now())
            {
                return Err(denied(reason));
            }
            let node = store
                .get(Kind::Node, WorkerKind::Pull.node_name())
                .await
                .map_err(|e| invalid(e.to_string()))?;
            if !matches!(node.map(|n| n.status), Some(StatusBody::Node(s)) if node_is_ready(s.ready, s.last_heartbeat_unix, unix_now()))
            {
                return Err(denied("pull Node is not ready"));
            }
            let jobs = store
                .list(Some(Kind::Job))
                .await
                .map_err(|e| invalid(e.to_string()))?;
            let reserved = jobs
                .iter()
                .filter_map(|j| match (&j.spec, &j.status) {
                    (Spec::Job(s), StatusBody::Job(t))
                        if !s.node_name.is_empty() && !t.phase.is_terminal() =>
                    {
                        Some(s.file_len)
                    }
                    _ => None,
                })
                .try_fold(0u64, |sum, len| sum.checked_add(len))
                .ok_or_else(|| invalid("bound byte budget overflow"))?;
            let free = mediaops_core::free_bytes(std::path::Path::new(&before.library_root))
                .map_err(|e| invalid(e.to_string()))?;
            let remaining =
                mediaops_core::pull_remaining_bytes(before).map_err(|e| invalid(e.to_string()))?;
            let mut reserved_disk = 0u64;
            for j in &jobs {
                if let (Spec::Job(s), StatusBody::Job(st)) = (&j.spec, &j.status)
                    && !s.node_name.is_empty()
                    && !st.phase.is_terminal()
                {
                    reserved_disk = reserved_disk
                        .checked_add(
                            mediaops_core::pull_remaining_bytes(s)
                                .map_err(|e| invalid(e.to_string()))?,
                        )
                        .ok_or_else(|| invalid("staging byte budget overflow"))?;
                }
            }
            if !mediaops_core::pull_fits(u64::MAX, 0, before.max_copy, reserved, before.file_len)
                || !mediaops_core::pull_fits(free, before.min_free, 0, reserved_disk, remaining)
                || !mediaops_core::install_fits(before).map_err(|e| invalid(e.to_string()))?
            {
                return Err(denied(
                    "snapshotted copy budget or disk watermark would be exceeded",
                ));
            }
        }
        HomeOp::PatchSpec => {
            next.spec = obj.spec.clone();
            next.validate()?;
            validate_root_change(store, &old.spec, &next.spec).await?;
            if let (Spec::Hold(before), Spec::Hold(after)) = (&old.spec, &next.spec)
                && (before.title_id != after.title_id || before.release_id != after.release_id)
            {
                return Err(denied("Hold title and release identity are immutable"));
            }
            if matches!((&old.spec, &next.spec), (Spec::Hold(before), Spec::Hold(after))
                if before.decision == mediaops_core::HoldDecisionSpec::Approved && after.decision != before.decision)
            {
                refuse_active_hold_change(store, &old.metadata.name).await?;
            }
            if let (Spec::Hold(before), Spec::Hold(after), StatusBody::Hold(status)) =
                (&old.spec, &next.spec, &old.status)
                && before.decision != after.decision
                && after.decision == mediaops_core::HoldDecisionSpec::Approved
            {
                let id = mediaops_core::TitleId::parse(&after.title_id)
                    .map_err(|e| invalid(e.to_string()))?;
                let placement = status
                    .placement
                    .as_ref()
                    .ok_or_else(|| denied("approval requires an authoritative placement"))?;
                mediaops_core::preflight_approve_placement(&id, placement)
                    .map_err(|e| denied(e.to_string()))?;
                let node = store
                    .get(Kind::Node, WorkerKind::Inventory.node_name())
                    .await
                    .map_err(|e| invalid(e.to_string()))?;
                if !matches!(node.map(|o| o.status), Some(StatusBody::Node(s)) if s.list_generation > 0
                    && s.list_generation == status.list_generation && node_is_ready(s.ready, s.last_heartbeat_unix, unix_now())
                    && node_is_ready(true, s.list_completed_unix, unix_now()))
                {
                    return Err(denied(
                        "approval requires a Hold in the fresh committed inventory",
                    ));
                }
            }
        }
        _ => return Err(invalid("unsupported patch")),
    }
    Ok(())
}

async fn validate_root_change(
    store: &ApiStore,
    before: &Spec,
    after: &Spec,
) -> Result<(), HomeError> {
    if let (Spec::Cluster(before), Spec::Cluster(after)) = (before, after)
        && before.library_root != after.library_root
    {
        let jobs = store
            .list(Some(Kind::Job))
            .await
            .map_err(|e| invalid(e.to_string()))?;
        if jobs.iter().any(|job| {
            matches!((&job.spec, &job.status), (Spec::Job(s), StatusBody::Job(st))
            if !st.phase.is_terminal() && s.library_root != after.library_root)
        }) {
            return Err(denied(
                "library root cannot change while incompatible nonterminal Jobs exist",
            ));
        }
    }
    Ok(())
}

async fn refuse_active_hold_change(store: &ApiStore, hold_name: &str) -> Result<(), HomeError> {
    let jobs = store
        .list(Some(Kind::Job))
        .await
        .map_err(|e| invalid(e.to_string()))?;
    if jobs.iter().any(|job| {
        matches!((&job.spec, &job.status), (Spec::Job(s), StatusBody::Job(st))
        if s.hold_name == hold_name && !s.node_name.is_empty() && !st.phase.is_terminal())
    }) {
        return Err(denied(
            "cannot revoke or delete approval while its bound Job is active",
        ));
    }
    Ok(())
}

async fn verify_title(
    store: &ApiStore,
    actor: Actor,
    obj: &HomeObject,
    old: Option<&HomeObject>,
    verify_disk: bool,
) -> Result<(), HomeError> {
    let (Spec::Title(spec), StatusBody::Title(status)) = (&obj.spec, &obj.status) else {
        return Err(invalid("Title body required"));
    };
    let files = status.observed_files();
    if status.path.is_empty() && !status.files.is_empty() { /* per-file representation */
    } else if !status.path.is_empty() && files.is_empty() {
        return Err(invalid("Title observation requires digests"));
    }
    if let Some(HomeObject {
        status: StatusBody::Title(before),
        ..
    }) = old
    {
        for previous in before.observed_files() {
            if !files
                .iter()
                .any(|f| f.path == previous.path && f.install_b3 == previous.install_b3)
            {
                return Err(denied("installed proof may not be removed or rewritten"));
            }
        }
    }
    let cluster = store
        .get(Kind::Cluster, CLUSTER_NAME)
        .await
        .map_err(|e| invalid(e.to_string()))?;
    let root = match cluster.map(|c| c.spec) {
        Some(Spec::Cluster(s)) => s.library_root,
        _ if files.is_empty() => return Ok(()),
        _ => return Err(invalid("Cluster required for Title proof")),
    };
    let jobs = if actor == Actor::Pull {
        store
            .list(Some(Kind::Job))
            .await
            .map_err(|e| invalid(e.to_string()))?
    } else {
        Vec::new()
    };
    for file in files {
        let unchanged = old.is_some_and(
            |t| matches!(&t.status, StatusBody::Title(s) if s.observed_files().contains(&file)),
        );
        if unchanged {
            continue;
        }
        if actor == Actor::Pull && !jobs.iter().any(|j| matches!((&j.spec, &j.status), (Spec::Job(s), StatusBody::Job(st))
            if s.title_id == spec.title_id && s.dest_rel == file.path && s.node_name == WorkerKind::Pull.node_name()
                && st.phase == JobPhase::Verifying && st.verified_b3.as_ref() == Some(&file.install_b3)
                && file.install_b3 == file.current_b3 && !file.drifted)) {
            return Err(denied("Title proof requires the bound worker's verifying Job"));
        }
        if actor == Actor::Import && file.drifted {
            continue;
        }
        if actor == Actor::Import && !verify_disk {
            continue;
        }
        let file_root = if actor == Actor::Pull {
            jobs.iter()
                .find_map(|j| match (&j.spec, &j.status) {
                    (Spec::Job(s), StatusBody::Job(st))
                        if s.title_id == spec.title_id
                            && s.dest_rel == file.path
                            && s.node_name == WorkerKind::Pull.node_name()
                            && st.phase == JobPhase::Verifying =>
                    {
                        Some(s.library_root.as_str())
                    }
                    _ => None,
                })
                .unwrap_or(&root)
        } else {
            &root
        };
        let path = std::path::Path::new(file_root).join(&file.path);
        if actor == Actor::Pull {
            // The bound worker owns cryptographic verification and persists its
            // digest before entering this gate. Avoid rehashing gigabytes while
            // serializing control-plane writes and heartbeats.
            let expected_len = jobs.iter().find_map(|j| match (&j.spec, &j.status) {
                (Spec::Job(s), StatusBody::Job(st))
                    if s.title_id == spec.title_id
                        && s.dest_rel == file.path
                        && st.phase == JobPhase::Verifying =>
                {
                    Some(s.file_len)
                }
                _ => None,
            });
            let meta = std::fs::symlink_metadata(&path).map_err(|e| invalid(e.to_string()))?;
            if !meta.is_file() || Some(meta.len()) != expected_len {
                return Err(invalid("installed file does not match the verifying Job"));
            }
            continue;
        }
        let digest = tokio::task::spawn_blocking(move || {
            let meta = std::fs::symlink_metadata(&path).map_err(|e| invalid(e.to_string()))?;
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err(invalid("Title proof must name a regular file"));
            }
            let file = std::fs::File::open(&path).map_err(|e| invalid(e.to_string()))?;
            mediaops_core::Blake3Hex::of_reader(file).map_err(|e| invalid(e.to_string()))
        })
        .await
        .map_err(|e| invalid(e.to_string()))??;
        if digest != file.current_b3 {
            return Err(invalid(
                "Title current digest does not match the library file",
            ));
        }
    }
    Ok(())
}

pub(crate) async fn validate_delete(
    store: &ApiStore,
    kind: Kind,
    name: &str,
) -> Result<(), HomeError> {
    if kind == Kind::Hold {
        refuse_active_hold_change(store, name).await?;
    }
    if let Some(obj) = store
        .get(kind, name)
        .await
        .map_err(|e| invalid(e.to_string()))?
        && matches!((&obj.spec, &obj.status), (Spec::Job(s), StatusBody::Job(st)) if !s.node_name.is_empty() && !st.phase.is_terminal())
    {
        return Err(denied("cannot delete a bound active Job"));
    }
    Ok(())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
