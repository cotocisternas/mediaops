//! In-process Want, Hold, drift and Job controllers.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::serve::Inner;
use mediaops_core::{
    CLUSTER_NAME, ClusterSpec, FileKey, HoldDecisionSpec, HomeObject, JobPhase, JobSpec, JobStatus,
    Kind, Placement, RemoteFileStatus, Spec, StatusBody, TitleId, TitleSpec, TitleStatus,
    WantPhase, WorkerKind, node_is_ready,
};
use mediaops_store::{ApiStore, StoreError};

pub(crate) fn spawn(inner: Arc<Inner>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(err) = reconcile_once(&inner).await {
                tracing::warn!(error = %err, "controller pass failed");
            }
            tokio::select! {
                _ = inner.wake.notified() => {},
                _ = tokio::time::sleep(Duration::from_secs(2)) => {},
            }
        }
    })
}

async fn reconcile_once(inner: &Inner) -> Result<(), StoreError> {
    {
        let _guard = inner.mutation.lock().await;
        refuse_revoked_jobs(&inner.store).await?;
    }
    // The Node commit marker and its rows must come from one sqlite snapshot.
    let (objects, _) = inner.store.snapshot(None).await?;
    let objects_of = |kind| objects.iter().filter(move |o| o.kind == kind);
    let mut titles: Vec<_> = objects_of(Kind::Title).cloned().collect();
    let cluster = objects
        .iter()
        .find(|o| o.kind == Kind::Cluster && o.metadata.name == CLUSTER_NAME);
    let root = match cluster.map(|c| &c.spec) {
        Some(Spec::Cluster(c)) => c.library_root.as_str(),
        _ => "",
    };
    flag_drift(&inner.store, &titles, root).await?;
    titles = inner.store.list(Some(Kind::Title)).await?;
    let Some(cluster) = cluster else {
        return Ok(());
    };
    let Spec::Cluster(cs) = &cluster.spec else {
        return Ok(());
    };
    if cs.lock || cs.library_root.is_empty() {
        return Ok(());
    }
    let Some(StatusBody::Node(inventory)) = objects_of(Kind::Node)
        .find(|n| n.metadata.name == WorkerKind::Inventory.node_name())
        .map(|n| &n.status)
    else {
        return Ok(());
    };
    if !node_is_ready(inventory.ready, inventory.last_heartbeat_unix, unix_now())
        || inventory.list_generation <= 0
        || !node_is_ready(true, inventory.list_completed_unix, unix_now())
    {
        return Ok(());
    }
    let remotes: Vec<_> = objects_of(Kind::RemoteFile)
        .filter_map(|o| match &o.status {
            StatusBody::RemoteFile(s) if s.list_generation == inventory.list_generation => Some(s),
            _ => None,
        })
        .collect();
    let holds: Vec<_> = objects_of(Kind::Hold)
        .filter(|h| {
            matches!(&h.status,
        StatusBody::Hold(s) if s.list_generation == inventory.list_generation)
        })
        .collect();
    for want in objects_of(Kind::Want) {
        let (Spec::Want(ws), StatusBody::Want(status)) = (&want.spec, &want.status) else {
            continue;
        };
        if status.phase == WantPhase::Dropped {
            continue;
        }
        let id = TitleId::parse(&ws.title_id).map_err(|e| StoreError::Sqlite(e.to_string()))?;
        let mut found = false;
        let mut complete = true;
        for remote in &remotes {
            if !remote.parse_ok || remote.title_id != ws.title_id || remote.len == 0 {
                continue;
            }
            let (_, placement) =
                match mediaops_core::parse_remote(Some(id.kind()), Path::new(&remote.rel_path)) {
                    Ok(parsed) => parsed,
                    Err(_) => continue,
                };
            found = true;
            let installed = has_placement(&titles, &ws.title_id, placement.file_key());
            complete &= installed && healthy_placement(&titles, &ws.title_id, placement.file_key());
            if installed || held_remote(&holds, remote, &ws.title_id) {
                continue;
            }
            create_pull_job(inner, cs, &id, remote, &placement, &objects, want).await?;
        }
        let phase = if found && complete {
            WantPhase::Satisfied
        } else {
            WantPhase::Open
        };
        if phase != status.phase {
            inner
                .store
                .patch_status(
                    Kind::Want,
                    &want.metadata.name,
                    StatusBody::Want(mediaops_core::WantStatus { phase }),
                    want.metadata.resource_version,
                )
                .await?;
        }
    }
    for hold in holds {
        let (Spec::Hold(hs), StatusBody::Hold(status)) = (&hold.spec, &hold.status) else {
            continue;
        };
        if hs.decision != HoldDecisionSpec::Approved {
            continue;
        }
        // Approval is bound to HoldKey's exact release file and authoritative placement.
        let Some(placement) = &status.placement else {
            continue;
        };
        let Some(remote) = remotes
            .iter()
            .find(|r| r.root_id == status.remote_root && r.rel_path == status.remote_path)
        else {
            continue;
        };
        let authority_id =
            TitleId::parse(&hs.title_id).map_err(|e| StoreError::Sqlite(e.to_string()))?;
        let placement = mediaops_core::preflight_approve_placement(&authority_id, placement)
            .map_err(|e| StoreError::Sqlite(e.to_string()))?;
        let canonical = mediaops_core::render(&authority_id, &placement)
            .map_err(|e| StoreError::Sqlite(e.to_string()))?;
        let id = mediaops_core::parse(&canonical).map_err(|e| StoreError::Sqlite(e.to_string()))?;
        if has_placement(&titles, &id.render(), placement.file_key()) {
            continue;
        }
        create_pull_job(inner, cs, &id, remote, &placement, &objects, hold).await?;
    }
    if let StatusBody::Cluster(status) = &cluster.status
        && status.accepted_generation != cluster.metadata.generation
    {
        let mut next = status.clone();
        next.accepted_generation = cluster.metadata.generation;
        inner
            .store
            .patch_status(
                Kind::Cluster,
                CLUSTER_NAME,
                StatusBody::Cluster(next),
                cluster.metadata.resource_version,
            )
            .await?;
    }
    Ok(())
}

fn held_remote(holds: &[&HomeObject], remote: &RemoteFileStatus, title_id: &str) -> bool {
    holds.iter().any(|h| match (&h.spec, &h.status) {
        (Spec::Hold(spec), StatusBody::Hold(status)) => {
            (status.remote_root == remote.root_id && status.remote_path == remote.rel_path)
                || (spec.title_id == title_id
                    && status.remote_path.is_empty()
                    && status.placement.is_none())
                || status.placement.as_ref().is_some_and(|placement| {
                    TitleId::parse(&spec.title_id)
                        .ok()
                        .and_then(|id| mediaops_core::render(&id, placement).ok())
                        .and_then(|path| mediaops_core::parse(&path).ok())
                        .is_some_and(|id| {
                            id.render() == title_id
                                && mediaops_core::parse_remote(
                                    Some(id.kind()),
                                    Path::new(&remote.rel_path),
                                )
                                .is_ok_and(
                                    |(_, remote_placement)| {
                                        remote_placement.file_key() == placement.file_key()
                                    },
                                )
                        })
                })
        }
        _ => false,
    })
}

fn has_placement(titles: &[HomeObject], title_id: &str, key: FileKey) -> bool {
    titles.iter().any(|t| match (&t.spec, &t.status) {
        (Spec::Title(spec), StatusBody::Title(status)) => {
            status.observed_files().iter().any(|f| {
                mediaops_core::parse_placement(Path::new(&f.path)).is_ok_and(|(id, placement)| {
                    placement.file_key() == key
                        && (spec.title_id == title_id || id.render() == title_id)
                })
            }) || (!status.path.is_empty()
                && mediaops_core::parse_placement(Path::new(&status.path)).is_ok_and(
                    |(id, placement)| {
                        placement.file_key() == key
                            && (spec.title_id == title_id || id.render() == title_id)
                    },
                ))
        }
        _ => false,
    })
}

fn healthy_placement(titles: &[HomeObject], title_id: &str, key: FileKey) -> bool {
    titles.iter().any(|t| match (&t.spec, &t.status) {
        (Spec::Title(spec), StatusBody::Title(status)) => status.observed_files().iter().any(|f| {
            !f.drifted
                && mediaops_core::parse_placement(Path::new(&f.path)).is_ok_and(|(id, p)| {
                    p.file_key() == key && (spec.title_id == title_id || id.render() == title_id)
                })
        }),
        _ => false,
    })
}

async fn flag_drift(store: &ApiStore, titles: &[HomeObject], root: &str) -> Result<(), StoreError> {
    for title in titles {
        let (Spec::Title(spec), StatusBody::Title(status)) = (&title.spec, &title.status) else {
            continue;
        };
        if !spec.desired_present {
            continue;
        }
        let mut next = status.clone();
        for file in &mut next.files {
            let path = Path::new(root).join(&file.path);
            // A vanished proof stays drifted until a verified maintenance write.
            if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
                file.drifted = true;
            }
        }
        if status.install_b3.is_some()
            && (status.path.is_empty()
                || !std::fs::symlink_metadata(Path::new(root).join(&status.path))
                    .is_ok_and(|m| m.is_file()))
        {
            next.drifted = true;
        }
        next.drifted |= next.files.iter().any(|f| f.drifted);
        if next != *status {
            store
                .patch_status(
                    Kind::Title,
                    &title.metadata.name,
                    StatusBody::Title(next),
                    title.metadata.resource_version,
                )
                .await?;
        }
    }
    Ok(())
}

async fn create_pull_job(
    inner: &Inner,
    cluster: &ClusterSpec,
    id: &TitleId,
    remote: &RemoteFileStatus,
    placement: &Placement,
    snapshot: &[HomeObject],
    authority: &HomeObject,
) -> Result<(), StoreError> {
    let _guard = inner.mutation.lock().await;
    // Maintenance serializes with creation; a stale pass cannot snapshot a
    // library root or budget that has already been replaced.
    if !matches!(inner.store.get(Kind::Cluster, CLUSTER_NAME).await?.map(|o| o.spec), Some(Spec::Cluster(ref now)) if now == cluster && !now.lock)
    {
        return Ok(());
    }
    // A snapshot is evidence, not a lease: admission must still see the same
    // authorizer, committed listing and source row after taking the write lock.
    let (current, _) = inner.store.snapshot(None).await?;
    let remote_name = mediaops_core::remote_file_name(&remote.root_id, &remote.rel_path);
    for (kind, name) in [
        (authority.kind, authority.metadata.name.as_str()),
        (Kind::Node, WorkerKind::Inventory.node_name()),
        (Kind::RemoteFile, remote_name.as_str()),
    ] {
        let previous = snapshot
            .iter()
            .find(|o| o.kind == kind && o.metadata.name == name);
        let latest = current
            .iter()
            .find(|o| o.kind == kind && o.metadata.name == name);
        if !matches!((previous, latest), (Some(a), Some(b)) if a.metadata.resource_version == b.metadata.resource_version)
        {
            return Ok(());
        }
    }
    let dest_rel = mediaops_core::render(id, placement)
        .map_err(|e| StoreError::Sqlite(e.to_string()))?
        .to_string_lossy()
        .into_owned();
    let key = match placement.file_key() {
        FileKey::Whole => "whole".into(),
        FileKey::Episode { season, episode } => format!("s{season}-e{episode}"),
        FileKey::Track { disc, track } => format!("d{disc}-t{track}"),
    };
    let name = format!("pull-{}-{key}", id.staging_token());
    if inner.store.get(Kind::Job, &name).await?.is_some() {
        return Ok(());
    }
    let title_id = id.render();
    let job = HomeObject::new(
        Kind::Job,
        name,
        Spec::Job(JobSpec {
            hold_name: if authority.kind == Kind::Hold {
                authority.metadata.name.clone()
            } else {
                String::new()
            },
            title_id: title_id.clone(),
            remote_root: remote.root_id.clone(),
            remote_path: remote.rel_path.clone(),
            dest_rel,
            file_len: remote.len,
            range_len: cluster.range_len.get(),
            range_concurrency: cluster.range_concurrency.unwrap_or(1),
            max_copy: cluster.max_copy.get(),
            min_free: cluster.min_free.get(),
            library_root: cluster.library_root.clone(),
            node_name: String::new(),
            worker_kind: WorkerKind::Pull.as_str().to_string(),
            ..JobSpec::default()
        }),
        StatusBody::Job(JobStatus::default()),
    );
    let Spec::Job(spec) = &job.spec else {
        unreachable!();
    };
    if authorization_refusal(&current, spec, unix_now()).is_some() {
        return Ok(());
    }
    if inner.store.get(Kind::Title, &title_id).await?.is_none() {
        inner
            .store
            .apply(HomeObject::new(
                Kind::Title,
                &title_id,
                Spec::Title(TitleSpec {
                    title_id: title_id.clone(),
                    desired_present: true,
                }),
                StatusBody::Title(TitleStatus::default()),
            ))
            .await?;
    }
    job.validate()?;
    inner.store.apply(job).await?;
    Ok(())
}

/// Shared by creation and binding. The caller holds the API mutation lock.
pub(crate) fn authorization_refusal(
    objects: &[HomeObject],
    job: &JobSpec,
    now: i64,
) -> Option<&'static str> {
    let Some(inventory) = objects.iter().find_map(|o| match &o.status {
        StatusBody::Node(s) if o.metadata.name == WorkerKind::Inventory.node_name() => Some(s),
        _ => None,
    }) else {
        return Some("inventory Node is missing");
    };
    if !node_is_ready(inventory.ready, inventory.last_heartbeat_unix, now)
        || inventory.list_generation <= 0
        || !node_is_ready(true, inventory.list_completed_unix, now)
    {
        return Some("inventory listing is not ready and fresh");
    }
    let Some(remote) = objects.iter().find_map(|o| match &o.status {
        StatusBody::RemoteFile(s)
            if s.root_id == job.remote_root
                && s.rel_path == job.remote_path
                && s.list_generation == inventory.list_generation
                && s.len == job.file_len =>
        {
            Some(s)
        }
        _ => None,
    }) else {
        return Some("Job source is absent or changed in the current listing");
    };
    let holds: Vec<_> = objects
        .iter()
        .filter(|o| {
            matches!(&o.status,
        StatusBody::Hold(s) if s.list_generation == inventory.list_generation)
        })
        .collect();
    if job.hold_name.is_empty() {
        if !objects.iter().any(|o| {
            matches!((&o.spec, &o.status), (Spec::Want(s), StatusBody::Want(st))
            if s.title_id == job.title_id && st.phase != WantPhase::Dropped)
        }) {
            return Some("Job Want is absent or dropped");
        }
        if !remote.parse_ok
            || remote.title_id != job.title_id
            || held_remote(&holds, remote, &job.title_id)
        {
            return Some("Job source requires an approved Hold");
        }
        let destination = TitleId::parse(&job.title_id).ok().and_then(|id| {
            mediaops_core::parse_remote(Some(id.kind()), Path::new(&remote.rel_path))
                .ok()
                .and_then(|(_, placement)| mediaops_core::render(&id, &placement).ok())
        });
        if destination.as_deref() != Some(Path::new(&job.dest_rel)) {
            return Some("Job source placement changed");
        }
    } else {
        let Some(hold) = holds.iter().find(|h| h.metadata.name == job.hold_name) else {
            return Some("authorizing Hold is no longer live");
        };
        let (Spec::Hold(spec), StatusBody::Hold(status)) = (&hold.spec, &hold.status) else {
            return Some("invalid Hold");
        };
        if spec.decision != HoldDecisionSpec::Approved
            || status.remote_root != job.remote_root
            || status.remote_path != job.remote_path
        {
            return Some("Hold no longer authorizes this source");
        }
        let destination = TitleId::parse(&spec.title_id).ok().and_then(|id| {
            status
                .placement
                .as_ref()
                .and_then(|placement| {
                    mediaops_core::preflight_approve_placement(&id, placement).ok()
                })
                .and_then(|placement| mediaops_core::render(&id, &placement).ok())
        });
        if destination.as_deref() != Some(Path::new(&job.dest_rel)) {
            return Some("Hold placement changed");
        }
    }
    let Ok((_, placement)) = mediaops_core::parse_placement(Path::new(&job.dest_rel)) else {
        return Some("invalid Job placement");
    };
    if has_placement(objects, &job.title_id, placement.file_key()) {
        return Some("Title already has an observation for this placement");
    }
    None
}

/// Revocation is durable before a queued Job may be bound. Running Jobs instead
/// prevent revocation in admission, since there is no worker cancellation protocol.
pub(crate) async fn refuse_revoked_jobs(store: &ApiStore) -> Result<(), StoreError> {
    let (objects, _) = store.snapshot(None).await?;
    for job in objects.iter().filter(|o| o.kind == Kind::Job) {
        let (Spec::Job(spec), StatusBody::Job(status)) = (&job.spec, &job.status) else {
            continue;
        };
        if !spec.node_name.is_empty() || status.phase != JobPhase::Pending {
            continue;
        }
        let message = if spec.hold_name.is_empty() {
            let live_want = objects.iter().any(|o| {
                matches!((&o.spec, &o.status), (Spec::Want(s), StatusBody::Want(st))
                    if s.title_id == spec.title_id && st.phase != WantPhase::Dropped)
            });
            if live_want {
                continue;
            }
            "Job Want is absent or dropped"
        } else if objects.iter().any(|o| {
            o.kind == Kind::Hold
                && o.metadata.name == spec.hold_name
                && matches!(&o.spec, Spec::Hold(h) if h.decision == HoldDecisionSpec::Approved)
        }) {
            continue;
        } else {
            "authorizing Hold was revoked or deleted"
        };
        let mut next = status.clone();
        next.phase = JobPhase::Refused;
        next.message = message.into();
        store
            .patch_status(
                Kind::Job,
                &job.metadata.name,
                StatusBody::Job(next),
                job.metadata.resource_version,
            )
            .await?;
    }
    Ok(())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{
        Bytes, CLUSTER_NAME, ClusterSpec, ClusterStatus, NodeSpec, NodeStatus, RemoteFileStatus,
        TitleSpec, TitleStatus, WantSpec, WantStatus,
    };
    use tokio::sync::{Mutex, Notify};

    async fn test_inner() -> (Inner, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-api-ctrl-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = ApiStore::open(dir.join("api.db")).await.expect("open");
        (
            Inner {
                store,
                mutation: Mutex::new(()),
                wake: Notify::new(),
            },
            dir,
        )
    }

    async fn configured() -> (Inner, std::path::PathBuf) {
        let (inner, dir) = test_inner().await;
        inner
            .store
            .apply(HomeObject::new(
                Kind::Cluster,
                CLUSTER_NAME,
                Spec::Cluster(ClusterSpec {
                    library_root: dir.display().to_string(),
                    ..ClusterSpec::default()
                }),
                StatusBody::empty(Kind::Cluster),
            ))
            .await
            .expect("cluster");
        inner
            .store
            .apply(ready_node(WorkerKind::Inventory))
            .await
            .expect("inventory");
        (inner, dir)
    }

    async fn observe(inner: &Inner, id: &str, path: &str, generation: i64) {
        inner
            .store
            .apply(HomeObject::new(
                Kind::RemoteFile,
                mediaops_core::remote_file_name("box", path),
                Spec::RemoteFile,
                StatusBody::RemoteFile(RemoteFileStatus {
                    root_id: "box".into(),
                    rel_path: path.into(),
                    len: 4,
                    parse_ok: !id.is_empty(),
                    title_id: id.into(),
                    list_generation: generation,
                }),
            ))
            .await
            .expect("remote");
    }

    async fn want(inner: &Inner, id: &str) -> HomeObject {
        inner
            .store
            .apply(HomeObject::new(
                Kind::Want,
                id,
                Spec::Want(WantSpec {
                    title_id: id.into(),
                }),
                StatusBody::empty(Kind::Want),
            ))
            .await
            .expect("want")
            .0
    }

    #[tokio::test]
    async fn same_size_episodes_have_independent_jobs_and_satisfied_wants_keep_watching() {
        let (inner, dir) = configured().await;
        let id = "series:key:thewire.2002";
        let want = want(&inner, id).await;
        for episode in [1, 2] {
            let placement = Placement::episode("The.Wire", 2002, 1, episode, "mkv");
            let rel = mediaops_core::render(&TitleId::parse(id).expect("id"), &placement)
                .expect("render");
            observe(
                &inner,
                id,
                rel.strip_prefix("series")
                    .expect("remote")
                    .to_str()
                    .expect("utf8"),
                1,
            )
            .await;
        }
        reconcile_once(&inner).await.expect("reconcile");
        let jobs = inner.store.list(Some(Kind::Job)).await.expect("jobs");
        assert_eq!(jobs.len(), 2);
        assert_ne!(jobs[0].metadata.name, jobs[1].metadata.name);
        let mut title = inner
            .store
            .get(Kind::Title, id)
            .await
            .expect("get")
            .expect("title");
        let files = jobs
            .iter()
            .map(|j| {
                let Spec::Job(spec) = &j.spec else {
                    panic!("job");
                };
                let path = dir.join(&spec.dest_rel);
                std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
                std::fs::write(path, b"home").expect("file");
                mediaops_core::TitleFileStatus {
                    path: spec.dest_rel.clone(),
                    install_b3: mediaops_core::Blake3Hex::of_bytes(b"home"),
                    current_b3: mediaops_core::Blake3Hex::of_bytes(b"home"),
                    drifted: false,
                }
            })
            .collect();
        title.status = StatusBody::Title(TitleStatus {
            files,
            ..TitleStatus::default()
        });
        inner
            .store
            .patch_status(
                Kind::Title,
                id,
                title.status,
                title.metadata.resource_version,
            )
            .await
            .expect("proof");
        inner
            .store
            .patch_status(
                Kind::Want,
                id,
                StatusBody::Want(WantStatus {
                    phase: WantPhase::Satisfied,
                }),
                want.metadata.resource_version,
            )
            .await
            .expect("satisfied");
        observe(
            &inner,
            id,
            "The.Wire.(2002)/Season.01/The.Wire.(2002).S01E03.mkv",
            1,
        )
        .await;
        reconcile_once(&inner).await.expect("later episode");
        assert_eq!(
            inner.store.list(Some(Kind::Job)).await.expect("jobs").len(),
            3
        );
        assert!(
            matches!(inner.store.get(Kind::Want, id).await.expect("get").expect("want").status,
            StatusBody::Want(s) if s.phase == WantPhase::Open)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn incomplete_stale_and_empty_inventory_never_schedule_old_files() {
        let (inner, dir) = configured().await;
        let id = "movie:key:thematrix.1999";
        want(&inner, id).await;
        observe(&inner, id, "The.Matrix.(1999)/The.Matrix.(1999).mkv", 2).await;
        reconcile_once(&inner).await.expect("incomplete generation");
        assert!(
            inner
                .store
                .list(Some(Kind::Job))
                .await
                .expect("jobs")
                .is_empty()
        );
        let mut node = inner
            .store
            .get(Kind::Node, "inventory")
            .await
            .expect("get")
            .expect("node");
        if let StatusBody::Node(s) = &mut node.status {
            s.list_generation = 2;
            s.list_completed_unix = 1;
        }
        node = inner
            .store
            .patch_status(
                Kind::Node,
                "inventory",
                node.status,
                node.metadata.resource_version,
            )
            .await
            .expect("stale publication");
        reconcile_once(&inner).await.expect("stale listing");
        assert!(
            inner
                .store
                .list(Some(Kind::Job))
                .await
                .expect("jobs")
                .is_empty()
        );
        if let StatusBody::Node(s) = &mut node.status {
            s.list_generation = 3;
            s.list_completed_unix = unix_now();
        }
        inner
            .store
            .patch_status(
                Kind::Node,
                "inventory",
                node.status,
                node.metadata.resource_version,
            )
            .await
            .expect("successful empty generation");
        reconcile_once(&inner).await.expect("empty listing");
        assert!(
            inner
                .store
                .list(Some(Kind::Job))
                .await
                .expect("jobs")
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn hold_approval_selects_exact_unparseable_release_and_wants_cannot_bypass_it() {
        let (inner, dir) = configured().await;
        let canonical = "movie:key:thematrix.1999";
        want(&inner, canonical).await;
        observe(
            &inner,
            canonical,
            "The.Matrix.(1999)/The.Matrix.(1999).mkv",
            1,
        )
        .await;
        observe(&inner, "", "Scene.Release.A.mkv", 1).await;
        observe(&inner, "", "Scene.Release.B.mkv", 1).await;
        let mut selected = None;
        for (release, decision) in [
            ("a", HoldDecisionSpec::Empty),
            ("b", HoldDecisionSpec::Rejected),
        ] {
            let obj = inner
                .store
                .apply(HomeObject::new(
                    Kind::Hold,
                    format!("movie:tmdb:603-{release}"),
                    Spec::Hold(mediaops_core::HoldSpec {
                        title_id: "movie:tmdb:603".into(),
                        release_id: release.into(),
                        decision,
                    }),
                    StatusBody::Hold(mediaops_core::HoldStatus {
                        list_generation: 1,
                        remote_root: "box".into(),
                        remote_path: format!("Scene.Release.{}.mkv", release.to_uppercase()),
                        placement: Some(Placement::movie("The.Matrix", 1999, "mkv")),
                        ..mediaops_core::HoldStatus::default()
                    }),
                ))
                .await
                .expect("hold")
                .0;
            if release == "a" {
                selected = Some(obj);
            }
        }
        reconcile_once(&inner)
            .await
            .expect("undecided and rejected holds");
        assert!(
            inner
                .store
                .list(Some(Kind::Job))
                .await
                .expect("jobs")
                .is_empty(),
            "Want cannot bypass hold"
        );
        let mut selected = selected.expect("selected release");
        if let Spec::Hold(s) = &mut selected.spec {
            s.decision = HoldDecisionSpec::Approved;
        }
        inner
            .store
            .patch_spec(
                Kind::Hold,
                &selected.metadata.name,
                selected.spec,
                selected.metadata.resource_version,
            )
            .await
            .expect("approve");
        reconcile_once(&inner).await.expect("approved");
        let jobs = inner.store.list(Some(Kind::Job)).await.expect("jobs");
        assert_eq!(jobs.len(), 1);
        assert!(
            matches!(&jobs[0].spec, Spec::Job(s) if s.remote_path == "Scene.Release.A.mkv" && s.title_id == canonical)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_episode_hold_does_not_block_other_episodes_of_the_show() {
        let id = "series:key:thewire.2002";
        let hold = HomeObject::new(
            Kind::Hold,
            "series:tvdb:79126-release",
            Spec::Hold(mediaops_core::HoldSpec {
                title_id: "series:tvdb:79126".into(),
                release_id: "release".into(),
                decision: HoldDecisionSpec::Empty,
            }),
            StatusBody::Hold(mediaops_core::HoldStatus {
                placement: Some(Placement::episode("The.Wire", 2002, 1, 1, "mkv")),
                ..mediaops_core::HoldStatus::default()
            }),
        );
        let mut remote = RemoteFileStatus {
            root_id: "box".into(),
            rel_path: "The.Wire.(2002)/Season.01/The.Wire.(2002).S01E02.mkv".into(),
            title_id: id.into(),
            ..RemoteFileStatus::default()
        };
        assert!(!held_remote(&[&hold], &remote, id));
        remote.rel_path = "The.Wire.(2002)/Season.01/The.Wire.(2002).S01E01.mkv".into();
        assert!(held_remote(&[&hold], &remote, id));
    }

    #[tokio::test]
    async fn stale_creation_snapshot_and_obsolete_approval_cannot_schedule() {
        let (inner, dir) = configured().await;
        let id = TitleId::parse("movie:key:thematrix.1999").expect("id");
        let authority = want(&inner, &id.render()).await;
        let path = "The.Matrix.(1999)/The.Matrix.(1999).mkv";
        observe(&inner, &id.render(), path, 1).await;
        let (snapshot, _) = inner.store.snapshot(None).await.expect("snapshot");
        let cluster = snapshot
            .iter()
            .find_map(|o| match &o.spec {
                Spec::Cluster(s) => Some(s),
                _ => None,
            })
            .expect("cluster");
        let remote = snapshot
            .iter()
            .find_map(|o| match &o.status {
                StatusBody::RemoteFile(s) => Some(s),
                _ => None,
            })
            .expect("remote");
        let mut node = inner
            .store
            .get(Kind::Node, "inventory")
            .await
            .expect("get")
            .expect("node");
        if let StatusBody::Node(s) = &mut node.status {
            s.ready = false;
        }
        node = inner
            .store
            .patch_status(
                Kind::Node,
                "inventory",
                node.status,
                node.metadata.resource_version,
            )
            .await
            .expect("invalidate");
        create_pull_job(
            &inner,
            cluster,
            &id,
            remote,
            &Placement::movie("The.Matrix", 1999, "mkv"),
            &snapshot,
            &authority,
        )
        .await
        .expect("stale pass");
        assert!(
            inner
                .store
                .list(Some(Kind::Job))
                .await
                .expect("jobs")
                .is_empty()
        );
        inner
            .store
            .delete(
                Kind::Want,
                &id.render(),
                authority.metadata.resource_version,
            )
            .await
            .expect("delete want");
        if let StatusBody::Node(s) = &mut node.status {
            s.ready = true;
            s.list_generation = 2;
        }
        inner
            .store
            .patch_status(
                Kind::Node,
                "inventory",
                node.status,
                node.metadata.resource_version,
            )
            .await
            .expect("commit");
        // The same path is now another release, but the old approved Hold was
        // last observed in generation one. No Wanted title permits fallback.
        let old = inner
            .store
            .list(Some(Kind::RemoteFile))
            .await
            .expect("remote")
            .remove(0);
        let mut changed = old.status;
        if let StatusBody::RemoteFile(s) = &mut changed {
            s.list_generation = 2;
        }
        inner
            .store
            .patch_status(
                Kind::RemoteFile,
                &old.metadata.name,
                changed,
                old.metadata.resource_version,
            )
            .await
            .expect("replacement");
        inner
            .store
            .apply(HomeObject::new(
                Kind::Hold,
                "movie:tmdb:603-old",
                Spec::Hold(mediaops_core::HoldSpec {
                    title_id: "movie:tmdb:603".into(),
                    release_id: "old".into(),
                    decision: HoldDecisionSpec::Approved,
                }),
                StatusBody::Hold(mediaops_core::HoldStatus {
                    list_generation: 1,
                    remote_root: "box".into(),
                    remote_path: path.into(),
                    placement: Some(Placement::movie("The.Matrix", 1999, "mkv")),
                    ..Default::default()
                }),
            ))
            .await
            .expect("old approval");
        reconcile_once(&inner).await.expect("reconcile");
        assert!(
            inner
                .store
                .list(Some(Kind::Job))
                .await
                .expect("jobs")
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn canonical_want_recognizes_imported_authority_title_proof() {
        let (inner, dir) = configured().await;
        let id = "movie:key:thematrix.1999";
        want(&inner, id).await;
        observe(&inner, id, "The.Matrix.(1999)/The.Matrix.(1999).mkv", 1).await;
        let path = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
        std::fs::create_dir_all(dir.join(path).parent().expect("parent")).expect("mkdir");
        std::fs::write(dir.join(path), b"home").expect("file");
        let digest = mediaops_core::Blake3Hex::of_bytes(b"home");
        inner
            .store
            .apply(HomeObject::new(
                Kind::Title,
                "movie:tmdb:603",
                Spec::Title(TitleSpec {
                    title_id: "movie:tmdb:603".into(),
                    desired_present: true,
                }),
                StatusBody::Title(TitleStatus {
                    files: vec![mediaops_core::TitleFileStatus {
                        path: path.into(),
                        install_b3: digest.clone(),
                        current_b3: digest,
                        drifted: false,
                    }],
                    ..Default::default()
                }),
            ))
            .await
            .expect("imported proof");
        reconcile_once(&inner).await.expect("reconcile");
        assert!(
            inner
                .store
                .list(Some(Kind::Job))
                .await
                .expect("jobs")
                .is_empty()
        );
        assert!(
            matches!(inner.store.get(Kind::Want, id).await.expect("get").expect("want").status,
            StatusBody::Want(s) if s.phase == WantPhase::Satisfied)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    async fn drifted_empty_title(inner: &Inner, id: &str) {
        inner
            .store
            .apply(HomeObject::new(
                Kind::Title,
                id,
                Spec::Title(TitleSpec {
                    title_id: id.into(),
                    desired_present: true,
                }),
                StatusBody::Title(TitleStatus {
                    drifted: true,
                    ..TitleStatus::default()
                }),
            ))
            .await
            .expect("title");
    }

    async fn observe_placement(inner: &Inner, id: &str, placement: Placement) {
        let parsed = TitleId::parse(id).expect("id");
        let rel = mediaops_core::render(&parsed, &placement).expect("render");
        let prefix = match parsed.kind() {
            mediaops_core::TitleKind::Movie => "movies",
            mediaops_core::TitleKind::Series => "series",
            mediaops_core::TitleKind::Album => "music",
        };
        observe(
            inner,
            id,
            rel.strip_prefix(prefix)
                .expect("kind dir")
                .to_str()
                .expect("utf8"),
            1,
        )
        .await;
    }

    fn job_dests(jobs: &[HomeObject]) -> Vec<&str> {
        jobs.iter()
            .map(|j| match &j.spec {
                Spec::Job(spec) => spec.dest_rel.as_str(),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[tokio::test]
    async fn drifted_empty_series_title_schedules_missing_episode_jobs() {
        let (inner, dir) = configured().await;
        let id = "series:key:thewire.2002";
        want(&inner, id).await;
        drifted_empty_title(&inner, id).await;
        for episode in [1, 2] {
            observe_placement(
                &inner,
                id,
                Placement::episode("The.Wire", 2002, 1, episode, "mkv"),
            )
            .await;
        }
        reconcile_once(&inner).await.expect("reconcile");
        let jobs = inner.store.list(Some(Kind::Job)).await.expect("jobs");
        let dests = job_dests(&jobs);
        assert_eq!(dests.len(), 2, "{dests:?}");
        assert!(dests.iter().any(|d| d.contains("S01E01")), "{dests:?}");
        assert!(dests.iter().any(|d| d.contains("S01E02")), "{dests:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn drifted_empty_album_title_schedules_missing_track_jobs() {
        let (inner, dir) = configured().await;
        let id = "album:key:radiohead.okcomputer";
        want(&inner, id).await;
        drifted_empty_title(&inner, id).await;
        for (track, title) in [(1, "Airbag"), (2, "Paranoid.Android")] {
            observe_placement(
                &inner,
                id,
                Placement::track(
                    "Radiohead",
                    "OK.Computer",
                    1997,
                    Some(1),
                    Some(track),
                    title,
                    "flac",
                ),
            )
            .await;
        }
        reconcile_once(&inner).await.expect("reconcile");
        let jobs = inner.store.list(Some(Kind::Job)).await.expect("jobs");
        let dests = job_dests(&jobs);
        assert_eq!(dests.len(), 2, "{dests:?}");
        assert!(dests.iter().any(|d| d.contains(".01.Airbag.")), "{dests:?}");
        assert!(
            dests.iter().any(|d| d.contains(".02.Paranoid.Android.")),
            "{dests:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn empty_path_movie_without_represented_placement_does_not_suppress_candidate() {
        let (inner, dir) = configured().await;
        let id = "movie:key:thematrix.1999";
        want(&inner, id).await;
        drifted_empty_title(&inner, id).await;
        observe_placement(&inner, id, Placement::movie("The.Matrix", 1999, "mkv")).await;
        reconcile_once(&inner).await.expect("reconcile");
        let jobs = inner.store.list(Some(Kind::Job)).await.expect("jobs");
        assert_eq!(jobs.len(), 1, "{jobs:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn drifted_title_with_episode_path_blocks_only_that_key() {
        let (inner, dir) = configured().await;
        let id = "series:key:thewire.2002";
        want(&inner, id).await;
        inner
            .store
            .apply(HomeObject::new(
                Kind::Title,
                id,
                Spec::Title(TitleSpec {
                    title_id: id.into(),
                    desired_present: true,
                }),
                StatusBody::Title(TitleStatus {
                    path: "series/The.Wire.(2002)/Season.01/The.Wire.(2002).S01E01.mkv".into(),
                    drifted: true,
                    ..TitleStatus::default()
                }),
            ))
            .await
            .expect("title");
        for episode in [1, 2] {
            observe_placement(
                &inner,
                id,
                Placement::episode("The.Wire", 2002, 1, episode, "mkv"),
            )
            .await;
        }
        reconcile_once(&inner).await.expect("reconcile");
        let jobs = inner.store.list(Some(Kind::Job)).await.expect("jobs");
        let dests = job_dests(&jobs);
        assert_eq!(dests.len(), 1, "{dests:?}");
        assert!(dests[0].contains("S01E02"), "{dests:?}");
        assert!(!dests[0].contains("S01E01"), "{dests:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    async fn seed_job(inner: &Inner, dir: &std::path::Path, bound: bool) -> HomeObject {
        inner
            .store
            .apply(HomeObject::new(
                Kind::Job,
                "pull-matrix",
                Spec::Job(JobSpec {
                    title_id: "movie:key:thematrix.1999".into(),
                    remote_root: "box".into(),
                    remote_path: "The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
                    dest_rel: "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
                    file_len: 4,
                    range_len: 4,
                    range_concurrency: 1,
                    library_root: dir.display().to_string(),
                    worker_kind: WorkerKind::Pull.as_str().to_string(),
                    node_name: if bound {
                        WorkerKind::Pull.node_name().to_string()
                    } else {
                        String::new()
                    },
                    ..JobSpec::default()
                }),
                StatusBody::Job(JobStatus::default()),
            ))
            .await
            .expect("job")
            .0
    }

    async fn current_job_phase(inner: &Inner) -> JobPhase {
        match inner
            .store
            .get(Kind::Job, "pull-matrix")
            .await
            .expect("get")
            .expect("job")
            .status
        {
            StatusBody::Job(st) => st.phase,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn want_absent_or_dropped_refuses_unbound_pending_job() {
        for dropped in [true, false] {
            let (inner, dir) = configured().await;
            let id = "movie:key:thematrix.1999";
            let want = want(&inner, id).await;
            seed_job(&inner, &dir, false).await;
            if dropped {
                inner
                    .store
                    .patch_status(
                        Kind::Want,
                        id,
                        StatusBody::Want(WantStatus {
                            phase: WantPhase::Dropped,
                        }),
                        want.metadata.resource_version,
                    )
                    .await
                    .expect("drop");
            } else {
                inner
                    .store
                    .delete(Kind::Want, id, want.metadata.resource_version)
                    .await
                    .expect("delete");
            }
            refuse_revoked_jobs(&inner.store).await.expect("refuse");
            assert_eq!(current_job_phase(&inner).await, JobPhase::Refused);
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[tokio::test]
    async fn bound_job_stays_pending_after_want_drop() {
        let (inner, dir) = configured().await;
        let id = "movie:key:thematrix.1999";
        let want = want(&inner, id).await;
        seed_job(&inner, &dir, true).await;
        inner
            .store
            .patch_status(
                Kind::Want,
                id,
                StatusBody::Want(WantStatus {
                    phase: WantPhase::Dropped,
                }),
                want.metadata.resource_version,
            )
            .await
            .expect("drop");
        refuse_revoked_jobs(&inner.store).await.expect("refuse");
        assert_eq!(current_job_phase(&inner).await, JobPhase::Pending);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn vanished_source_does_not_refuse_pending_job() {
        let (inner, dir) = configured().await;
        let id = "movie:key:thematrix.1999";
        want(&inner, id).await;
        seed_job(&inner, &dir, false).await;
        refuse_revoked_jobs(&inner.store).await.expect("refuse");
        assert_eq!(current_job_phase(&inner).await, JobPhase::Pending);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn ready_node(kind: WorkerKind) -> HomeObject {
        HomeObject::new(
            Kind::Node,
            kind.node_name(),
            Spec::Node(NodeSpec { worker_kind: kind }),
            StatusBody::Node(NodeStatus {
                ready: true,
                last_heartbeat_unix: unix_now(),
                list_generation: 1,
                list_completed_unix: unix_now(),
            }),
        )
    }

    #[tokio::test]
    async fn want_creates_snapshotted_pull_when_inventory_ready() {
        let (inner, dir) = test_inner().await;
        inner
            .store
            .apply(HomeObject::new(
                Kind::Cluster,
                CLUSTER_NAME,
                Spec::Cluster(ClusterSpec {
                    max_copy: Bytes::new(1 << 30),
                    min_free: Bytes::new(0),
                    range_len: Bytes::new(Bytes::MIB),
                    library_root: dir.display().to_string(),
                    ..ClusterSpec::default()
                }),
                StatusBody::Cluster(ClusterStatus::default()),
            ))
            .await
            .expect("cluster");
        inner
            .store
            .apply(ready_node(WorkerKind::Inventory))
            .await
            .expect("node");
        inner
            .store
            .apply(HomeObject::new(
                Kind::RemoteFile,
                "seedbox/The.Matrix.(1999)/The.Matrix.(1999).mkv",
                Spec::RemoteFile,
                StatusBody::RemoteFile(RemoteFileStatus {
                    root_id: "seedbox".into(),
                    rel_path: "The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
                    len: 32,
                    parse_ok: true,
                    title_id: "movie:tmdb:603".into(),
                    list_generation: 1,
                }),
            ))
            .await
            .expect("remote");
        inner
            .store
            .apply(HomeObject::new(
                Kind::Want,
                "movie:tmdb:603",
                Spec::Want(WantSpec {
                    title_id: "movie:tmdb:603".into(),
                }),
                StatusBody::Want(WantStatus {
                    phase: WantPhase::Open,
                }),
            ))
            .await
            .expect("want");
        reconcile_once(&inner).await.expect("reconcile");
        let jobs = inner.store.list(Some(Kind::Job)).await.expect("jobs");
        assert_eq!(jobs.len(), 1, "{jobs:?}");
        match &jobs[0].spec {
            Spec::Job(spec) => {
                assert_eq!(spec.title_id, "movie:tmdb:603");
                assert_eq!(spec.file_len, 32);
                assert_eq!(spec.range_len, Bytes::MIB);
                assert!(spec.node_name.is_empty());
            }
            other => panic!("{other:?}"),
        }
        reconcile_once(&inner).await.expect("second");
        let jobs = inner.store.list(Some(Kind::Job)).await.expect("jobs");
        assert_eq!(jobs.len(), 1, "no duplicate job");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn installed_title_is_not_silently_replaced() {
        let (inner, dir) = configured().await;
        let id = "movie:tmdb:603";
        observe(&inner, id, "The.Matrix.(1999)/The.Matrix.(1999).mkv", 1).await;
        inner
            .store
            .apply(HomeObject::new(
                Kind::Title,
                id,
                Spec::Title(TitleSpec {
                    title_id: id.into(),
                    desired_present: true,
                }),
                StatusBody::Title(TitleStatus {
                    path: "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
                    ..TitleStatus::default()
                }),
            ))
            .await
            .expect("title");
        want(&inner, id).await;
        reconcile_once(&inner).await.expect("reconcile");
        let jobs = inner.store.list(Some(Kind::Job)).await.expect("jobs");
        assert!(jobs.is_empty(), "installed title must not recopy");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn drift_sets_flag_when_install_digest_exists_and_path_is_gone() {
        let (inner, dir) = test_inner().await;
        let digest = mediaops_core::Blake3Hex::parse(&"a".repeat(64)).expect("d");
        inner
            .store
            .apply(HomeObject::new(
                Kind::Title,
                "movie:tmdb:603",
                Spec::Title(TitleSpec {
                    title_id: "movie:tmdb:603".into(),
                    desired_present: true,
                }),
                StatusBody::Title(TitleStatus {
                    files: Vec::new(),
                    path: String::new(),
                    install_b3: Some(digest),
                    current_b3: None,
                    drifted: false,
                }),
            ))
            .await
            .expect("title");
        reconcile_once(&inner).await.expect("reconcile");
        let title = inner
            .store
            .get(Kind::Title, "movie:tmdb:603")
            .await
            .expect("get")
            .expect("row");
        match title.status {
            StatusBody::Title(st) => assert!(st.drifted),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
