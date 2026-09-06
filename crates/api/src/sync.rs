use std::sync::Arc;
use std::time::Duration;

use mediaops_core::{
    CLUSTER_NAME, HomeError, HomeObject, Kind, SECRET_NAME, Spec, StatusBody, SyncPhase, SyncSpec,
    SyncStatus, WorkerKind,
};
use mediaops_store::StoreError;

use crate::serve::Inner;

pub(crate) const PLAN_SECONDS: i64 = 60;

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

pub(crate) fn error(err: StoreError) -> HomeError {
    match err {
        StoreError::Home(error) => error,
        other => HomeError::Invalid(other.to_string()),
    }
}

pub(crate) async fn start(
    inner: &Arc<Inner>,
    request_id: String,
    dry_run: bool,
) -> Result<HomeObject, HomeError> {
    if request_id.is_empty()
        || request_id.len() > 128
        || !request_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(HomeError::Invalid(
            "sync request ID must contain 1-128 ASCII letters, digits, hyphens or underscores"
                .into(),
        ));
    }
    let request = {
        let _guard = inner.mutation.lock().await;
        let existing = if dry_run {
            None
        } else {
            inner
                .store
                .get(Kind::Sync, &request_id)
                .await
                .map_err(error)?
        };
        if let Some(existing) = existing {
            existing
        } else {
            let (objects, revision) = inner.store.snapshot(None).await.map_err(error)?;
            let request = accepted(request_id, &objects, revision)?;
            if dry_run {
                request
            } else {
                inner.store.apply(request).await.map_err(error)?.0
            }
        }
    };
    if dry_run {
        return preview(inner, request).await;
    }
    inner.wake.notify_one();
    loop {
        let current = inner
            .store
            .get(Kind::Sync, &request.metadata.name)
            .await
            .map_err(error)?
            .ok_or_else(|| HomeError::NotFound {
                kind: Kind::Sync,
                name: request.metadata.name.clone(),
            })?;
        let StatusBody::Sync(status) = &current.status else {
            return Err(HomeError::Invalid("Sync status missing".into()));
        };
        match status.phase {
            SyncPhase::Scheduled => return Ok(current),
            SyncPhase::Failed => {
                return Err(HomeError::Invalid(format!(
                    "sync {} failed: {}",
                    current.metadata.name, status.message
                )));
            }
            SyncPhase::WaitingInventory | SyncPhase::Captured => {
                if now() >= status.deadline_unix {
                    return Err(HomeError::Invalid(format!(
                        "sync {} planning deadline reached; inspect with get Sync (no automatic retry)",
                        current.metadata.name
                    )));
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn accepted(name: String, objects: &[HomeObject], revision: i64) -> Result<HomeObject, HomeError> {
    let cluster = objects
        .iter()
        .find(|o| o.kind == Kind::Cluster && o.metadata.name == CLUSTER_NAME)
        .ok_or_else(|| HomeError::Invalid("Cluster is not configured".into()))?;
    let Spec::Cluster(config) = &cluster.spec else {
        return Err(HomeError::Invalid("Cluster spec missing".into()));
    };
    if config.lock || config.library_root.is_empty() {
        return Err(HomeError::Denied(
            "Cluster is locked or library root is missing".into(),
        ));
    }
    let generation = objects
        .iter()
        .find_map(|obj| match &obj.status {
            StatusBody::Node(st) if obj.metadata.name == WorkerKind::Inventory.node_name() => {
                Some(st.list_generation)
            }
            _ => None,
        })
        .unwrap_or(0);
    Ok(HomeObject::new(
        Kind::Sync,
        name,
        Spec::Sync(SyncSpec::default()),
        StatusBody::Sync(SyncStatus {
            phase: SyncPhase::WaitingInventory,
            accepted_unix: now(),
            deadline_unix: now().saturating_add(PLAN_SECONDS),
            baseline_generation: generation,
            acceptance_rv: revision,
            cluster: Some(config.clone()),
            cluster_generation: cluster.metadata.generation,
            secret_resource_version: secret_revision(objects),
            ..Default::default()
        }),
    ))
}

pub(crate) fn secret_revision(objects: &[HomeObject]) -> i64 {
    objects
        .iter()
        .find(|o| o.kind == Kind::Secret && o.metadata.name == SECRET_NAME)
        .map(|o| o.metadata.resource_version)
        .unwrap_or(0)
}

async fn preview(inner: &Arc<Inner>, mut request: HomeObject) -> Result<HomeObject, HomeError> {
    loop {
        let (objects, _) = inner.store.snapshot(None).await.map_err(error)?;
        let StatusBody::Sync(status) = &mut request.status else {
            return Err(HomeError::Invalid("Sync status missing".into()));
        };
        if now() >= status.deadline_unix {
            return Err(HomeError::Invalid(
                "sync preview timed out waiting for a fresh successful inventory".into(),
            ));
        }
        if let Some(generation) = crate::sync_controller::qualifying_generation(&objects, status)? {
            status.list_generation = generation;
            status.phase = SyncPhase::Captured;
            return tokio::task::spawn_blocking(move || {
                let mut entries = crate::sync_plan::capture(&objects, &request);
                crate::sync_plan::evaluate(&mut entries, &objects, now());
                if let StatusBody::Sync(st) = &mut request.status {
                    st.entries = entries;
                    st.message = "Preview only; no objects or copy Jobs were written".into();
                }
                request
            })
            .await
            .map_err(|err| HomeError::Invalid(err.to_string()));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub(crate) async fn begin_inventory(inner: &Inner) -> Result<HomeObject, HomeError> {
    let _guard = inner.mutation.lock().await;
    let (objects, revision) = inner.store.snapshot(None).await.map_err(error)?;
    let previous = objects.iter().find(|obj| {
        obj.kind == Kind::Node && obj.metadata.name == WorkerKind::Inventory.node_name()
    });
    let mut node = previous.cloned().unwrap_or_else(|| {
        HomeObject::new(
            Kind::Node,
            WorkerKind::Inventory.node_name(),
            Spec::Node(mediaops_core::NodeSpec {
                worker_kind: WorkerKind::Inventory,
            }),
            StatusBody::empty(Kind::Node),
        )
    });
    let StatusBody::Node(status) = &mut node.status else {
        return Err(HomeError::Invalid("inventory Node status missing".into()));
    };
    status.ready = false;
    status.last_heartbeat_unix = now();
    status.scan_started_rv = revision.saturating_add(1);
    status.scan_cluster_generation = objects
        .iter()
        .find(|obj| obj.kind == Kind::Cluster && obj.metadata.name == CLUSTER_NAME)
        .map(|o| o.metadata.generation)
        .unwrap_or(0);
    status.scan_secret_resource_version = secret_revision(&objects);
    let result = if previous.is_some() {
        inner
            .store
            .patch_status(
                Kind::Node,
                &node.metadata.name,
                node.status,
                node.metadata.resource_version,
            )
            .await
            .map_err(error)?
    } else {
        inner.store.apply(node).await.map_err(error)?.0
    };
    inner.wake.notify_one();
    Ok(result)
}
