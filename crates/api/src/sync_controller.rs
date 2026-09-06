use mediaops_core::{
    CLUSTER_NAME, HomeError, HomeObject, Kind, Spec, StatusBody, SyncPhase, SyncStatus, WorkerKind,
    node_is_ready,
};
use mediaops_store::StoreError;

use crate::serve::Inner;
use crate::sync::{now, secret_revision};

pub(crate) fn inventory_generation(objects: &[HomeObject]) -> Option<i64> {
    objects.iter().find_map(|obj| match &obj.status {
        StatusBody::Node(st)
            if obj.kind == Kind::Node
                && obj.metadata.name == WorkerKind::Inventory.node_name()
                && st.list_generation > 0
                && node_is_ready(st.ready, st.last_heartbeat_unix, now())
                && node_is_ready(true, st.list_completed_unix, now()) =>
        {
            Some(st.list_generation)
        }
        _ => None,
    })
}

pub(crate) fn qualifying_generation(
    objects: &[HomeObject],
    request: &SyncStatus,
) -> Result<Option<i64>, HomeError> {
    let current = objects
        .iter()
        .find(|o| o.kind == Kind::Cluster && o.metadata.name == CLUSTER_NAME);
    if !matches!(current, Some(obj) if obj.metadata.generation == request.cluster_generation && matches!(&obj.spec, Spec::Cluster(config) if Some(config) == request.cluster.as_ref() && !config.lock))
        || secret_revision(objects) != request.secret_resource_version
    {
        return Err(HomeError::Invalid(
            "Cluster or seedbox configuration changed; issue a new sync request".into(),
        ));
    }
    let Some(generation) = inventory_generation(objects) else {
        return Ok(None);
    };
    Ok(objects.iter().find_map(|obj| match &obj.status {
        StatusBody::Node(st)
            if obj.kind == Kind::Node
                && obj.metadata.name == WorkerKind::Inventory.node_name()
                && generation > request.baseline_generation
                && st.scan_started_rv > request.acceptance_rv
                && st.scan_cluster_generation == request.cluster_generation
                && st.scan_secret_resource_version == request.secret_resource_version =>
        {
            Some(generation)
        }
        _ => None,
    }))
}

pub(crate) async fn reconcile(inner: &Inner) -> Result<(), StoreError> {
    let requests = inner.store.list(Some(Kind::Sync)).await?;
    for request in requests {
        let StatusBody::Sync(status) = &request.status else {
            continue;
        };
        if matches!(status.phase, SyncPhase::Scheduled | SyncPhase::Failed) {
            continue;
        }
        if let Err(err) = advance(inner, request.clone()).await {
            match err {
                StoreError::Home(HomeError::Conflict { .. } | HomeError::NotFound { .. }) => {}
                other => fail(inner, &request, &other.to_string()).await?,
            }
        }
    }
    Ok(())
}

async fn advance(inner: &Inner, mut request: HomeObject) -> Result<(), StoreError> {
    let (objects, revision) = inner.store.snapshot(None).await?;
    let StatusBody::Sync(status) = &request.status else {
        return Ok(());
    };
    if now() >= status.deadline_unix {
        return fail(
            inner,
            &request,
            "planning deadline expired without scheduling",
        )
        .await;
    }
    let fresh = qualifying_generation(&objects, status)?;
    match status.phase {
        SyncPhase::WaitingInventory => {
            let Some(generation) = fresh else {
                return Ok(());
            };
            if let StatusBody::Sync(st) = &mut request.status {
                st.list_generation = generation;
                st.phase = SyncPhase::Captured;
            }
            let entries = crate::sync_plan::capture(&objects, &request);
            if let StatusBody::Sync(st) = &mut request.status {
                st.entries = entries;
            }
            let _guard = inner.mutation.lock().await;
            let (latest, _) = inner.store.snapshot(None).await?;
            let StatusBody::Sync(st) = &request.status else {
                return Ok(());
            };
            if qualifying_generation(&latest, st)? != Some(generation) {
                return Ok(());
            }
            if latest
                .iter()
                .filter(|o| matches!(o.kind, Kind::RemoteFile | Kind::Hold))
                .ne(objects
                    .iter()
                    .filter(|o| matches!(o.kind, Kind::RemoteFile | Kind::Hold)))
            {
                return Ok(());
            }
            inner
                .store
                .patch_status(
                    Kind::Sync,
                    &request.metadata.name,
                    request.status,
                    request.metadata.resource_version,
                )
                .await?;
            inner.wake.notify_one();
        }
        SyncPhase::Captured => {
            if fresh.is_none() {
                return Ok(());
            }
            let planned = tokio::task::spawn_blocking(move || {
                if let StatusBody::Sync(st) = &mut request.status {
                    crate::sync_plan::evaluate(&mut st.entries, &objects, now());
                }
                request
            })
            .await
            .map_err(|err| StoreError::Join(err.to_string()))?;
            let _guard = inner.mutation.lock().await;
            let StatusBody::Sync(st) = &planned.status else {
                return Ok(());
            };
            if now() >= st.deadline_unix {
                drop(_guard);
                return fail(
                    inner,
                    &planned,
                    "planning deadline expired without scheduling",
                )
                .await;
            }
            inner.store.schedule_sync(revision, planned).await?;
            inner.wake.notify_one();
        }
        SyncPhase::Scheduled | SyncPhase::Failed => {}
    }
    Ok(())
}

async fn fail(inner: &Inner, request: &HomeObject, message: &str) -> Result<(), StoreError> {
    let _guard = inner.mutation.lock().await;
    let Some(mut latest) = inner.store.get(Kind::Sync, &request.metadata.name).await? else {
        return Ok(());
    };
    let StatusBody::Sync(status) = &mut latest.status else {
        return Ok(());
    };
    if matches!(status.phase, SyncPhase::Scheduled | SyncPhase::Failed) {
        return Ok(());
    }
    status.phase = SyncPhase::Failed;
    status.message = message.into();
    inner
        .store
        .patch_status(
            Kind::Sync,
            &latest.metadata.name,
            latest.status,
            latest.metadata.resource_version,
        )
        .await?;
    Ok(())
}
