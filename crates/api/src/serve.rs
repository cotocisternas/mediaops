//! UDS Home service. No mTLS. Actor comes from `x-mediaops-actor`.

use std::pin::Pin;
use std::sync::Arc;

use mediaops_core::{
    ACTOR_HEADER, Actor, CLUSTER_NAME, HomeError, HomeObject, HomeOp, Kind, Spec, StatusBody, admit,
};
use mediaops_proto::home::home_service_server::{HomeService, HomeServiceServer};
use mediaops_proto::home::{
    ApplyRequest, ApplyResponse, DeleteRequest, DeleteResponse, GetRequest, GetResponse,
    ListRequest, ListResponse, PatchRequest, PatchResponse, ReconcileRequest, ReconcileResponse,
    WatchRequest, WatchResponse, WatchType as WireWatchType,
};
use mediaops_proto::{home_object_from_wire, home_object_to_wire, home_status};
use mediaops_store::{ApiStore, StoreError, WatchType};
use tokio::net::UnixListener;
use tokio::sync::{Mutex, Notify};
use tokio_stream::Stream;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{Request, Response, Status};

use crate::ApiConfig;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("io: {0}")]
    Io(String),
    #[error("serve: {0}")]
    Serve(String),
}

pub(crate) struct Inner {
    pub(crate) store: ApiStore,
    pub(crate) mutation: Mutex<()>,
    pub(crate) wake: Notify,
}

#[derive(Clone)]
struct HomeSvc {
    inner: Arc<Inner>,
}

/// Bind the Home service on `config.socket` and open `config.api_db`.
pub async fn serve_api(config: ApiConfig) -> Result<(), ApiError> {
    let store = ApiStore::open(&config.api_db).await?;
    let inner = Arc::new(Inner {
        store,
        mutation: Mutex::new(()),
        wake: Notify::new(),
    });
    if let Some(parent) = config.socket.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ApiError::Io(e.to_string()))?;
    }
    let listener = bind_socket(&config.socket).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config.socket, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| ApiError::Io(e.to_string()))?;
    }
    let _controllers = AbortTask(crate::controllers::spawn(inner.clone()));
    let incoming = UnixListenerStream::new(listener);
    tonic::transport::Server::builder()
        .add_service(HomeServiceServer::new(HomeSvc { inner }))
        .serve_with_incoming(incoming)
        .await
        .map_err(|e| ApiError::Serve(e.to_string()))
}

struct AbortTask(tokio::task::JoinHandle<()>);
impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn actor_of<T>(req: &Request<T>) -> Result<Actor, Status> {
    match req.metadata().get(ACTOR_HEADER) {
        Some(value) => {
            let raw = value
                .to_str()
                .map_err(|_| home_status(HomeError::Invalid("actor header is not utf-8".into())))?;
            Actor::parse(raw).map_err(home_status)
        }
        None => Ok(Actor::Cli),
    }
}

fn map_store(err: StoreError) -> Status {
    match err {
        StoreError::Home(home) => home_status(home),
        other => home_status(HomeError::Invalid(other.to_string())),
    }
}

fn maybe_redact(actor: Actor, mut obj: HomeObject) -> HomeObject {
    if actor == Actor::Cli {
        obj.redact();
    }
    obj
}

#[tonic::async_trait]
impl HomeService for HomeSvc {
    async fn get(&self, req: Request<GetRequest>) -> Result<Response<GetResponse>, Status> {
        let actor = actor_of(&req)?;
        let inner = req.into_inner();
        let kind = Kind::parse(&inner.kind).map_err(home_status)?;
        admit(actor, HomeOp::Get, kind).map_err(home_status)?;
        if inner.name.is_empty() {
            return Err(home_status(HomeError::Invalid("name is required".into())));
        }
        let obj = self
            .inner
            .store
            .get(kind, &inner.name)
            .await
            .map_err(map_store)?
            .ok_or_else(|| {
                home_status(HomeError::NotFound {
                    kind,
                    name: inner.name,
                })
            })?;
        Ok(Response::new(GetResponse {
            object: Some(home_object_to_wire(&maybe_redact(actor, obj))),
        }))
    }

    async fn list(&self, req: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        let actor = actor_of(&req)?;
        let inner = req.into_inner();
        let kind = if inner.kind.is_empty() {
            None
        } else {
            Some(Kind::parse(&inner.kind).map_err(home_status)?)
        };
        if let Some(kind) = kind {
            admit(actor, HomeOp::List, kind).map_err(home_status)?;
        }
        let items = self
            .inner
            .store
            .list(kind)
            .await
            .map_err(map_store)?
            .into_iter()
            .map(|obj| home_object_to_wire(&maybe_redact(actor, obj)))
            .collect();
        Ok(Response::new(ListResponse { items }))
    }

    type WatchStream = Pin<Box<dyn Stream<Item = Result<WatchResponse, Status>> + Send>>;

    async fn watch(
        &self,
        req: Request<WatchRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        let actor = actor_of(&req)?;
        let inner = req.into_inner();
        let kind = if inner.kind.is_empty() {
            None
        } else {
            Some(Kind::parse(&inner.kind).map_err(home_status)?)
        };
        if let Some(kind) = kind {
            admit(actor, HomeOp::Watch, kind).map_err(home_status)?;
        }
        // Subscribe before snapshotting: a write landing between the two is
        // otherwise in neither the snapshot nor the backlog. Overlap is fine,
        // the rv filter and the client dedupe on it.
        if inner.resource_version < 0 {
            return Err(home_status(HomeError::Invalid(
                "resourceVersion must be nonnegative".into(),
            )));
        }
        let (snapshot, after) = if inner.resource_version == 0 {
            self.inner.store.snapshot(kind).await.map_err(map_store)?
        } else {
            (Vec::new(), inner.resource_version)
        };
        let stream = async_stream(snapshot, kind, after, actor, self.inner.store.clone());
        Ok(Response::new(Box::pin(stream)))
    }

    async fn apply(&self, req: Request<ApplyRequest>) -> Result<Response<ApplyResponse>, Status> {
        let actor = actor_of(&req)?;
        let inner = req.into_inner();
        let obj = inner
            .object
            .ok_or_else(|| home_status(HomeError::Invalid("apply requires an object".into())))?;
        let obj = home_object_from_wire(obj).map_err(home_status)?;
        admit(actor, HomeOp::Apply, obj.kind).map_err(home_status)?;
        if obj.kind == Kind::RemoteFile && !matches!(actor, Actor::Inventory) {
            return Err(home_status(HomeError::Denied(
                "only inventory may apply RemoteFile".into(),
            )));
        }
        let import_proof = actor == Actor::Import && obj.kind == Kind::Title;
        let proof_cluster = if import_proof {
            let cluster = cluster_spec(&self.inner.store).await?;
            crate::admission::validate_apply(&self.inner.store, actor, &obj, true)
                .await
                .map_err(home_status)?;
            cluster
        } else {
            None
        };
        let _guard = self.inner.mutation.lock().await;
        if import_proof && proof_cluster != cluster_spec(&self.inner.store).await? {
            return Err(home_status(HomeError::Conflict {
                kind: Kind::Cluster,
                name: CLUSTER_NAME.into(),
            }));
        }
        crate::admission::validate_apply(&self.inner.store, actor, &obj, false)
            .await
            .map_err(home_status)?;
        let (written, _) = self.inner.store.apply(obj).await.map_err(map_store)?;
        self.inner.wake.notify_one();
        Ok(Response::new(ApplyResponse {
            object: Some(home_object_to_wire(&maybe_redact(actor, written))),
        }))
    }

    async fn patch(&self, req: Request<PatchRequest>) -> Result<Response<PatchResponse>, Status> {
        let actor = actor_of(&req)?;
        let inner = req.into_inner();
        let obj = inner
            .object
            .ok_or_else(|| home_status(HomeError::Invalid("patch requires an object".into())))?;
        let obj = home_object_from_wire(obj).map_err(home_status)?;
        let sub = inner.subresource.as_str();
        let op = match sub {
            "" | "spec" => HomeOp::PatchSpec,
            "status" => HomeOp::PatchStatus,
            "bind" => HomeOp::PatchBind,
            other => {
                return Err(home_status(HomeError::Invalid(format!(
                    "unknown subresource `{other}`"
                ))));
            }
        };
        admit(actor, op, obj.kind).map_err(home_status)?;
        let import_proof =
            actor == Actor::Import && obj.kind == Kind::Title && op == HomeOp::PatchStatus;
        let proof_cluster = if import_proof {
            let cluster = cluster_spec(&self.inner.store).await?;
            crate::admission::validate_patch(&self.inner.store, actor, op, &obj, true)
                .await
                .map_err(home_status)?;
            cluster
        } else {
            None
        };
        let _guard = self.inner.mutation.lock().await;
        if import_proof && proof_cluster != cluster_spec(&self.inner.store).await? {
            return Err(home_status(HomeError::Conflict {
                kind: Kind::Cluster,
                name: CLUSTER_NAME.into(),
            }));
        }
        crate::admission::validate_patch(&self.inner.store, actor, op, &obj, false)
            .await
            .map_err(home_status)?;
        let expected = obj.metadata.resource_version;
        let written = match op {
            HomeOp::PatchStatus => self
                .inner
                .store
                .patch_status(obj.kind, &obj.metadata.name, obj.status, expected)
                .await
                .map_err(map_store)?,
            HomeOp::PatchBind => {
                if obj.kind != Kind::Job {
                    return Err(home_status(HomeError::Invalid(
                        "bind is only valid on Job".into(),
                    )));
                }
                // Bind assigns a worker. It must not rewrite the budget the
                // controller snapshotted into the Job at create time, so take
                // only nodeName / workerKind and keep the stored spec.
                let Spec::Job(patch) = obj.spec else {
                    return Err(home_status(HomeError::Invalid(
                        "bind requires a Job body".into(),
                    )));
                };
                let current = self
                    .inner
                    .store
                    .get(Kind::Job, &obj.metadata.name)
                    .await
                    .map_err(map_store)?
                    .ok_or_else(|| {
                        home_status(HomeError::NotFound {
                            kind: Kind::Job,
                            name: obj.metadata.name.clone(),
                        })
                    })?;
                let Spec::Job(mut spec) = current.spec else {
                    return Err(home_status(HomeError::Invalid(
                        "stored Job has no Job body".into(),
                    )));
                };
                spec.node_name = patch.node_name;
                spec.worker_kind = patch.worker_kind;
                self.inner
                    .store
                    .patch_spec(Kind::Job, &obj.metadata.name, Spec::Job(spec), expected)
                    .await
                    .map_err(map_store)?
            }
            _ => self
                .inner
                .store
                .patch_spec(obj.kind, &obj.metadata.name, obj.spec, expected)
                .await
                .map_err(map_store)?,
        };
        if written.kind == Kind::Hold {
            crate::controllers::refuse_revoked_jobs(&self.inner.store)
                .await
                .map_err(map_store)?;
        }
        self.inner.wake.notify_one();
        Ok(Response::new(PatchResponse {
            object: Some(home_object_to_wire(&maybe_redact(actor, written))),
        }))
    }

    async fn delete(
        &self,
        req: Request<DeleteRequest>,
    ) -> Result<Response<DeleteResponse>, Status> {
        let actor = actor_of(&req)?;
        let inner = req.into_inner();
        let kind = Kind::parse(&inner.kind).map_err(home_status)?;
        admit(actor, HomeOp::Delete, kind).map_err(home_status)?;
        let _guard = self.inner.mutation.lock().await;
        crate::admission::validate_delete(&self.inner.store, kind, &inner.name)
            .await
            .map_err(home_status)?;
        let written = self
            .inner
            .store
            .delete(kind, &inner.name, inner.resource_version)
            .await
            .map_err(map_store)?;
        if kind == Kind::Want || kind == Kind::Hold {
            crate::controllers::refuse_revoked_jobs(&self.inner.store)
                .await
                .map_err(map_store)?;
        }
        self.inner.wake.notify_one();
        Ok(Response::new(DeleteResponse {
            object: Some(home_object_to_wire(&maybe_redact(actor, written))),
        }))
    }

    async fn reconcile(
        &self,
        req: Request<ReconcileRequest>,
    ) -> Result<Response<ReconcileResponse>, Status> {
        let actor = actor_of(&req)?;
        admit(actor, HomeOp::Reconcile, Kind::Cluster).map_err(home_status)?;
        let _guard = self.inner.mutation.lock().await;
        let cluster = self
            .inner
            .store
            .get(Kind::Cluster, CLUSTER_NAME)
            .await
            .map_err(map_store)?
            .ok_or_else(|| {
                home_status(HomeError::NotFound {
                    kind: Kind::Cluster,
                    name: CLUSTER_NAME.into(),
                })
            })?;
        let StatusBody::Cluster(mut st) = cluster.status.clone() else {
            return Err(home_status(HomeError::Invalid(
                "Cluster status missing".into(),
            )));
        };
        st.reconcile_generation += 1;
        self.inner
            .store
            .patch_status(
                Kind::Cluster,
                CLUSTER_NAME,
                StatusBody::Cluster(st.clone()),
                cluster.metadata.resource_version,
            )
            .await
            .map_err(map_store)?;
        self.inner.wake.notify_one();
        Ok(Response::new(ReconcileResponse {
            reconcile_generation: st.reconcile_generation,
        }))
    }
}

fn async_stream(
    snapshot: Vec<HomeObject>,
    kind: Option<Kind>,
    mut after: i64,
    actor: Actor,
    store: ApiStore,
) -> impl Stream<Item = Result<WatchResponse, Status>> {
    let (tx, out) = tokio::sync::mpsc::channel(64);
    tokio::spawn(async move {
        for obj in snapshot {
            if tx
                .send(Ok(to_watch(WatchType::Added, maybe_redact(actor, obj))))
                .await
                .is_err()
            {
                return;
            }
        }
        loop {
            if tx.is_closed() {
                return;
            }
            match store.events_after(after).await {
                Ok(events) if !events.is_empty() => {
                    for ev in events {
                        after = ev.object.metadata.resource_version;
                        if kind.is_none_or(|k| k == ev.object.kind)
                            && tx
                                .send(Ok(to_watch(ev.watch, maybe_redact(actor, ev.object))))
                                .await
                                .is_err()
                        {
                            return;
                        }
                    }
                }
                Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                Err(err) => {
                    let _ = tx.send(Err(map_store(err))).await;
                    return;
                }
            }
        }
    });
    tokio_stream::wrappers::ReceiverStream::new(out)
}

async fn bind_socket(path: &std::path::Path) -> Result<UnixListener, ApiError> {
    use std::os::unix::fs::FileTypeExt;
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if !meta.file_type().is_socket() {
                return Err(ApiError::Io(
                    "socket path is not a socket; preserving it".into(),
                ));
            }
            match tokio::net::UnixStream::connect(path).await {
                Ok(_) => return Err(ApiError::Io("socket is already live".into())),
                Err(err) if err.kind() == std::io::ErrorKind::ConnectionRefused => {
                    std::fs::remove_file(path).map_err(|e| ApiError::Io(e.to_string()))?;
                }
                Err(err) => return Err(ApiError::Io(err.to_string())),
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(ApiError::Io(err.to_string())),
    }
    UnixListener::bind(path).map_err(|e| ApiError::Io(e.to_string()))
}

fn to_watch(watch: WatchType, obj: HomeObject) -> WatchResponse {
    WatchResponse {
        r#type: match watch {
            WatchType::Added => WireWatchType::Added as i32,
            WatchType::Modified => WireWatchType::Modified as i32,
            WatchType::Deleted => WireWatchType::Deleted as i32,
        },
        object: Some(home_object_to_wire(&obj)),
    }
}

async fn cluster_spec(store: &ApiStore) -> Result<Option<Spec>, Status> {
    Ok(store
        .get(Kind::Cluster, CLUSTER_NAME)
        .await
        .map_err(map_store)?
        .map(|o| o.spec))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{
        Blake3Hex, ClusterSpec, JobPhase, JobSpec, JobStatus, NodeSpec, NodeStatus,
        TitleFileStatus, TitleSpec, TitleStatus, WantSpec, WorkerKind,
    };

    struct Harness {
        svc: HomeSvc,
        dir: std::path::PathBuf,
    }
    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    async fn harness() -> Harness {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-admission-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let store = ApiStore::open(dir.join("api.db")).await.expect("store");
        store
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
        Harness {
            svc: HomeSvc {
                inner: Arc::new(Inner {
                    store,
                    mutation: Mutex::new(()),
                    wake: Notify::new(),
                }),
            },
            dir,
        }
    }

    fn request<T>(actor: Actor, body: T) -> Request<T> {
        let mut req = Request::new(body);
        req.metadata_mut()
            .insert(ACTOR_HEADER, actor.as_str().parse().expect("actor"));
        req
    }

    async fn apply(svc: &HomeSvc, actor: Actor, obj: &HomeObject) -> Result<HomeObject, Status> {
        let out = svc
            .apply(request(
                actor,
                ApplyRequest {
                    object: Some(home_object_to_wire(obj)),
                },
            ))
            .await?;
        home_object_from_wire(out.into_inner().object.expect("object")).map_err(home_status)
    }

    async fn patch(
        svc: &HomeSvc,
        actor: Actor,
        obj: &HomeObject,
        subresource: &str,
    ) -> Result<HomeObject, Status> {
        let out = svc
            .patch(request(
                actor,
                PatchRequest {
                    object: Some(home_object_to_wire(obj)),
                    subresource: subresource.into(),
                },
            ))
            .await?;
        home_object_from_wire(out.into_inner().object.expect("object")).map_err(home_status)
    }

    fn job(dir: &std::path::Path) -> HomeObject {
        HomeObject::new(
            Kind::Job,
            "pull-matrix",
            Spec::Job(JobSpec {
                title_id: "movie:key:thematrix.1999".into(),
                remote_root: "movies".into(),
                remote_path: "The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
                dest_rel: "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
                file_len: 4,
                range_len: 4,
                range_concurrency: 1,
                library_root: dir.display().to_string(),
                worker_kind: "pull".into(),
                ..JobSpec::default()
            }),
            StatusBody::empty(Kind::Job),
        )
    }

    async fn authorize_job(svc: &HomeSvc, job: &HomeObject) {
        let Spec::Job(spec) = &job.spec else {
            panic!("Job");
        };
        apply(
            svc,
            Actor::Cli,
            &HomeObject::new(
                Kind::Want,
                &spec.title_id,
                Spec::Want(WantSpec {
                    title_id: spec.title_id.clone(),
                }),
                StatusBody::empty(Kind::Want),
            ),
        )
        .await
        .expect("Want");
        apply(
            svc,
            Actor::Inventory,
            &HomeObject::new(
                Kind::Node,
                "inventory",
                Spec::Node(NodeSpec {
                    worker_kind: WorkerKind::Inventory,
                }),
                StatusBody::Node(NodeStatus {
                    ready: true,
                    last_heartbeat_unix: now(),
                    list_generation: 1,
                    list_completed_unix: now(),
                }),
            ),
        )
        .await
        .expect("inventory");
        apply(
            svc,
            Actor::Inventory,
            &HomeObject::new(
                Kind::RemoteFile,
                mediaops_core::remote_file_name(&spec.remote_root, &spec.remote_path),
                Spec::RemoteFile,
                StatusBody::RemoteFile(mediaops_core::RemoteFileStatus {
                    root_id: spec.remote_root.clone(),
                    rel_path: spec.remote_path.clone(),
                    len: spec.file_len,
                    title_id: spec.title_id.clone(),
                    parse_ok: true,
                    list_generation: 1,
                }),
            ),
        )
        .await
        .expect("source");
    }

    async fn ready_pull(svc: &HomeSvc) {
        apply(
            svc,
            Actor::Pull,
            &HomeObject::new(
                Kind::Node,
                "pull",
                Spec::Node(NodeSpec {
                    worker_kind: WorkerKind::Pull,
                }),
                StatusBody::Node(NodeStatus {
                    ready: true,
                    last_heartbeat_unix: now(),
                    ..Default::default()
                }),
            ),
        )
        .await
        .expect("pull");
    }

    fn binding(mut job: HomeObject) -> HomeObject {
        if let Spec::Job(s) = &mut job.spec {
            s.node_name = "pull".into();
        }
        job
    }

    #[tokio::test]
    async fn binding_rechecks_listing_root_and_maintenance_lock() {
        let h = harness().await;
        let stored = apply(&h.svc, Actor::Controller, &job(&h.dir))
            .await
            .expect("job");
        authorize_job(&h.svc, &stored).await;
        ready_pull(&h.svc).await;
        let mut node = h
            .svc
            .inner
            .store
            .get(Kind::Node, "inventory")
            .await
            .expect("get")
            .expect("node");
        if let StatusBody::Node(s) = &mut node.status {
            s.ready = false;
        }
        node = patch(&h.svc, Actor::Inventory, &node, "status")
            .await
            .expect("invalidate");
        let bind = binding(stored.clone());
        assert!(
            patch(&h.svc, Actor::Scheduler, &bind, "bind")
                .await
                .is_err(),
            "stale listing"
        );
        if let StatusBody::Node(s) = &mut node.status {
            s.ready = true;
        }
        patch(&h.svc, Actor::Inventory, &node, "status")
            .await
            .expect("ready");
        let mut cluster = h
            .svc
            .inner
            .store
            .get(Kind::Cluster, CLUSTER_NAME)
            .await
            .expect("get")
            .expect("cluster");
        if let Spec::Cluster(s) = &mut cluster.spec {
            s.lock = true;
        }
        cluster = patch(&h.svc, Actor::Cli, &cluster, "spec")
            .await
            .expect("maintenance");
        assert!(
            patch(&h.svc, Actor::Scheduler, &bind, "bind")
                .await
                .is_err(),
            "locked"
        );
        if let Spec::Cluster(s) = &mut cluster.spec {
            s.library_root = h.dir.join("other").display().to_string();
        }
        assert!(
            apply(&h.svc, Actor::Cli, &cluster).await.is_err(),
            "Apply cannot strand an unbound snapshot"
        );
        assert!(
            patch(&h.svc, Actor::Cli, &cluster, "spec").await.is_err(),
            "Patch cannot strand an unbound snapshot"
        );
        if let Spec::Cluster(s) = &mut cluster.spec {
            s.library_root = h.dir.display().to_string();
            s.lock = false;
        }
        patch(&h.svc, Actor::Cli, &cluster, "spec")
            .await
            .expect("unlock");
        patch(&h.svc, Actor::Scheduler, &bind, "bind")
            .await
            .expect("valid bind");
    }

    #[tokio::test]
    async fn hold_revocation_refuses_queued_jobs_but_cannot_revoke_bound_work() {
        for bound in [false, true] {
            let h = harness().await;
            let mut pending = job(&h.dir);
            if let Spec::Job(s) = &mut pending.spec {
                s.hold_name = "movie:tmdb:603-release".into();
            }
            authorize_job(&h.svc, &pending).await;
            ready_pull(&h.svc).await;
            let hold = HomeObject::new(
                Kind::Hold,
                "movie:tmdb:603-release",
                Spec::Hold(mediaops_core::HoldSpec {
                    title_id: "movie:tmdb:603".into(),
                    release_id: "release".into(),
                    decision: mediaops_core::HoldDecisionSpec::Empty,
                }),
                StatusBody::Hold(mediaops_core::HoldStatus {
                    list_generation: 1,
                    remote_root: "movies".into(),
                    remote_path: "The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
                    placement: Some(mediaops_core::Placement::movie("The.Matrix", 1999, "mkv")),
                    ..Default::default()
                }),
            );
            let mut hold = apply(&h.svc, Actor::Inventory, &hold)
                .await
                .expect("live Hold");
            if let Spec::Hold(s) = &mut hold.spec {
                s.decision = mediaops_core::HoldDecisionSpec::Approved;
            }
            hold = patch(&h.svc, Actor::Cli, &hold, "spec")
                .await
                .expect("approve");
            pending = apply(&h.svc, Actor::Controller, &pending)
                .await
                .expect("job");
            if bound {
                pending = patch(&h.svc, Actor::Scheduler, &binding(pending), "bind")
                    .await
                    .expect("bind");
            }
            if let Spec::Hold(s) = &mut hold.spec {
                s.decision = mediaops_core::HoldDecisionSpec::Rejected;
            }
            let result = patch(&h.svc, Actor::Cli, &hold, "spec").await;
            if bound {
                assert!(
                    result.is_err(),
                    "no false cancellation promise for running work"
                );
                assert!(
                    h.svc
                        .delete(request(
                            Actor::Cli,
                            DeleteRequest {
                                kind: "Hold".into(),
                                name: hold.metadata.name.clone(),
                                resource_version: hold.metadata.resource_version
                            }
                        ))
                        .await
                        .is_err(),
                    "delete cannot revoke running work either"
                );
            } else {
                result.expect("reject");
                let current = h
                    .svc
                    .inner
                    .store
                    .get(Kind::Job, &pending.metadata.name)
                    .await
                    .expect("get")
                    .expect("job");
                assert!(
                    matches!(current.status, StatusBody::Job(s) if s.phase == JobPhase::Refused)
                );
                assert!(
                    patch(&h.svc, Actor::Scheduler, &binding(pending), "bind")
                        .await
                        .is_err()
                );
            }
        }
    }

    #[tokio::test]
    async fn want_delete_refuses_queued_jobs_but_cannot_revoke_bound_work() {
        for bound in [false, true] {
            let h = harness().await;
            let mut pending = job(&h.dir);
            authorize_job(&h.svc, &pending).await;
            ready_pull(&h.svc).await;
            pending = apply(&h.svc, Actor::Controller, &pending)
                .await
                .expect("job");
            if bound {
                pending = patch(&h.svc, Actor::Scheduler, &binding(pending), "bind")
                    .await
                    .expect("bind");
            }
            let want = h
                .svc
                .inner
                .store
                .get(Kind::Want, "movie:key:thematrix.1999")
                .await
                .expect("get")
                .expect("want");
            h.svc
                .delete(request(
                    Actor::Cli,
                    DeleteRequest {
                        kind: "Want".into(),
                        name: want.metadata.name.clone(),
                        resource_version: want.metadata.resource_version,
                    },
                ))
                .await
                .expect("delete want");
            let current = h
                .svc
                .inner
                .store
                .get(Kind::Job, &pending.metadata.name)
                .await
                .expect("get")
                .expect("job");
            if bound {
                assert!(
                    matches!(
                        (&current.spec, &current.status),
                        (Spec::Job(s), StatusBody::Job(st))
                            if !s.node_name.is_empty() && st.phase == JobPhase::Pending
                    ),
                    "bound work is uncancellable"
                );
            } else {
                assert!(
                    matches!(current.status, StatusBody::Job(s) if s.phase == JobPhase::Refused)
                );
                assert!(
                    patch(&h.svc, Actor::Scheduler, &binding(pending), "bind")
                        .await
                        .is_err()
                );
            }
        }
    }

    #[tokio::test]
    async fn stale_source_can_bind_when_it_returns() {
        let h = harness().await;
        let stored = apply(&h.svc, Actor::Controller, &job(&h.dir))
            .await
            .expect("job");
        authorize_job(&h.svc, &stored).await;
        ready_pull(&h.svc).await;
        let Spec::Job(spec) = &stored.spec else {
            panic!("Job");
        };
        let remote_name = mediaops_core::remote_file_name(&spec.remote_root, &spec.remote_path);
        let remote = h
            .svc
            .inner
            .store
            .get(Kind::RemoteFile, &remote_name)
            .await
            .expect("get")
            .expect("source");
        h.svc
            .delete(request(
                Actor::Inventory,
                DeleteRequest {
                    kind: "RemoteFile".into(),
                    name: remote.metadata.name.clone(),
                    resource_version: remote.metadata.resource_version,
                },
            ))
            .await
            .expect("source vanished");
        let bind = binding(stored.clone());
        assert!(
            patch(&h.svc, Actor::Scheduler, &bind, "bind")
                .await
                .is_err(),
            "vanished source"
        );
        let current = h
            .svc
            .inner
            .store
            .get(Kind::Job, &stored.metadata.name)
            .await
            .expect("get")
            .expect("job");
        assert!(
            matches!(current.status, StatusBody::Job(s) if s.phase == JobPhase::Pending),
            "vanished source is not terminal"
        );
        apply(
            &h.svc,
            Actor::Inventory,
            &HomeObject::new(
                Kind::RemoteFile,
                remote_name,
                Spec::RemoteFile,
                StatusBody::RemoteFile(mediaops_core::RemoteFileStatus {
                    root_id: spec.remote_root.clone(),
                    rel_path: spec.remote_path.clone(),
                    len: spec.file_len,
                    title_id: spec.title_id.clone(),
                    parse_ok: true,
                    list_generation: 1,
                }),
            ),
        )
        .await
        .expect("source returned");
        patch(&h.svc, Actor::Scheduler, &bind, "bind")
            .await
            .expect("bind after source returns");
    }

    #[tokio::test]
    async fn approval_requires_live_generation_and_authoritative_placement() {
        let h = harness().await;
        authorize_job(&h.svc, &job(&h.dir)).await;
        for (release, generation, placement) in [
            (
                "stale",
                0,
                Some(mediaops_core::Placement::movie("The.Matrix", 1999, "mkv")),
            ),
            ("unknown", 1, None),
        ] {
            let hold = HomeObject::new(
                Kind::Hold,
                format!("movie:tmdb:603-{release}"),
                Spec::Hold(mediaops_core::HoldSpec {
                    title_id: "movie:tmdb:603".into(),
                    release_id: release.into(),
                    decision: mediaops_core::HoldDecisionSpec::Empty,
                }),
                StatusBody::Hold(mediaops_core::HoldStatus {
                    list_generation: generation,
                    placement,
                    ..Default::default()
                }),
            );
            let mut hold = apply(&h.svc, Actor::Inventory, &hold)
                .await
                .expect("observation");
            if let Spec::Hold(s) = &mut hold.spec {
                s.decision = mediaops_core::HoldDecisionSpec::Approved;
            }
            assert!(patch(&h.svc, Actor::Cli, &hold, "spec").await.is_err());
            assert!(
                matches!(h.svc.inner.store.get(Kind::Hold, &hold.metadata.name).await.expect("get").expect("hold").spec,
                Spec::Hold(s) if s.decision == mediaops_core::HoldDecisionSpec::Empty)
            );
        }
        let before = h.svc.inner.store.current_rv().await.expect("rv");
        let mut unsupported = job(&h.dir);
        unsupported.api_version = "mediaops.home.v999".into();
        assert!(
            apply(&h.svc, Actor::Controller, &unsupported)
                .await
                .is_err()
        );
        assert_eq!(h.svc.inner.store.current_rv().await.expect("rv"), before);
    }

    #[tokio::test]
    async fn cli_apply_cannot_forge_status_and_zero_version_cannot_overwrite() {
        let h = harness().await;
        let mut title = HomeObject::new(
            Kind::Title,
            "movie:key:thematrix.1999",
            Spec::Title(TitleSpec {
                title_id: "movie:key:thematrix.1999".into(),
                desired_present: true,
            }),
            StatusBody::empty(Kind::Title),
        );
        if let StatusBody::Title(s) = &mut title.status {
            s.drifted = true;
        }
        assert_eq!(
            apply(&h.svc, Actor::Cli, &title)
                .await
                .expect_err("forged status")
                .code(),
            tonic::Code::PermissionDenied
        );
        assert!(
            h.svc
                .inner
                .store
                .get(Kind::Title, &title.metadata.name)
                .await
                .expect("get")
                .is_none()
        );
        title.status = StatusBody::empty(Kind::Title);
        let stored = apply(&h.svc, Actor::Cli, &title)
            .await
            .expect("create title");
        assert_eq!(
            apply(&h.svc, Actor::Cli, &title)
                .await
                .expect_err("zero revision")
                .code(),
            tonic::Code::FailedPrecondition
        );
        assert_eq!(
            patch(&h.svc, Actor::Cli, &stored, "status")
                .await
                .expect_err("unowned")
                .code(),
            tonic::Code::PermissionDenied
        );
        assert_eq!(
            h.svc.inner.store.current_rv().await.expect("rv"),
            stored.metadata.resource_version
        );
    }

    #[tokio::test]
    async fn worker_identity_binding_snapshot_and_lifecycle_are_checked() {
        let h = harness().await;
        let stored = apply(&h.svc, Actor::Controller, &job(&h.dir))
            .await
            .expect("job");
        authorize_job(&h.svc, &stored).await;
        let mut node = HomeObject::new(
            Kind::Node,
            "pull",
            Spec::Node(NodeSpec {
                worker_kind: WorkerKind::Pull,
            }),
            StatusBody::Node(NodeStatus {
                ready: true,
                last_heartbeat_unix: now(),
                ..NodeStatus::default()
            }),
        );
        assert!(
            apply(&h.svc, Actor::Inventory, &node).await.is_err(),
            "cannot impersonate another role Node"
        );
        node = apply(&h.svc, Actor::Pull, &node).await.expect("own Node");
        let mut active = stored.clone();
        active.status = StatusBody::Job(JobStatus {
            phase: JobPhase::Pulling,
            attempts: 1,
            started_unix: now(),
            ..JobStatus::default()
        });
        assert!(
            patch(&h.svc, Actor::Pull, &active, "status").await.is_err(),
            "unbound worker refused"
        );
        let mut binding = stored.clone();
        if let Spec::Job(s) = &mut binding.spec {
            s.node_name = "pull".into();
            s.range_len = 99;
        }
        let bound = patch(&h.svc, Actor::Scheduler, &binding, "bind")
            .await
            .expect("bind");
        assert!(
            matches!(&bound.spec, Spec::Job(s) if s.range_len == 4),
            "binding cannot rewrite snapshot"
        );
        assert!(
            patch(&h.svc, Actor::Scheduler, &bound, "bind")
                .await
                .is_err(),
            "cannot rebind"
        );
        active.metadata = bound.metadata.clone();
        active.spec = bound.spec.clone();
        let mut active = patch(&h.svc, Actor::Pull, &active, "status")
            .await
            .expect("start");
        let old = active.clone();
        if let StatusBody::Job(s) = &mut active.status {
            s.phase = JobPhase::Installed;
        }
        assert!(
            patch(&h.svc, Actor::Pull, &active, "status").await.is_err(),
            "cannot skip verifying/proof"
        );
        let mut bad_deadline = old;
        if let StatusBody::Job(s) = &mut bad_deadline.status {
            s.started_unix += 1;
        }
        assert!(
            patch(&h.svc, Actor::Pull, &bad_deadline, "status")
                .await
                .is_err(),
            "cannot reset deadline"
        );
        if let StatusBody::Node(s) = &mut node.status {
            s.last_heartbeat_unix = 0;
        }
        patch(&h.svc, Actor::Pull, &node, "status")
            .await
            .expect("stale node");
        let mut another = job(&h.dir);
        another.metadata.name = "another".into();
        let mut another = apply(&h.svc, Actor::Controller, &another)
            .await
            .expect("new job");
        if let Spec::Job(s) = &mut another.spec {
            s.node_name = "pull".into();
        }
        assert!(
            patch(&h.svc, Actor::Scheduler, &another, "bind")
                .await
                .is_err(),
            "stale Node cannot bind"
        );
    }

    #[tokio::test]
    async fn imported_proof_is_verified_and_install_digest_is_immutable() {
        let h = harness().await;
        let rel = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
        let path = h.dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, b"home").expect("file");
        let proof = TitleFileStatus {
            path: rel.into(),
            install_b3: Blake3Hex::of_bytes(b"home"),
            current_b3: Blake3Hex::of_bytes(b"wrong"),
            drifted: false,
        };
        let mut title = HomeObject::new(
            Kind::Title,
            "movie:key:thematrix.1999",
            Spec::Title(TitleSpec {
                title_id: "movie:key:thematrix.1999".into(),
                desired_present: true,
            }),
            StatusBody::Title(TitleStatus {
                files: vec![proof],
                ..TitleStatus::default()
            }),
        );
        assert!(
            apply(&h.svc, Actor::Import, &title).await.is_err(),
            "false proof denied"
        );
        if let StatusBody::Title(s) = &mut title.status {
            s.files[0].current_b3 = s.files[0].install_b3.clone();
        }
        let mut title = apply(&h.svc, Actor::Import, &title)
            .await
            .expect("verified import");
        if let StatusBody::Title(s) = &mut title.status {
            s.files[0].install_b3 = Blake3Hex::of_bytes(b"rewrite");
        }
        assert!(
            patch(&h.svc, Actor::Import, &title, "status")
                .await
                .is_err(),
            "immutable original proof"
        );
    }

    #[tokio::test]
    async fn socket_regular_file_and_live_listener_are_preserved() {
        let h = harness().await;
        let path = h.dir.join("socket");
        std::fs::write(&path, b"precious").expect("file");
        assert!(bind_socket(&path).await.is_err());
        assert_eq!(std::fs::read(&path).expect("preserved"), b"precious");
        let live_path = h.dir.join("live.sock");
        let live = UnixListener::bind(&live_path).expect("listener");
        assert!(bind_socket(&live_path).await.is_err());
        assert!(tokio::net::UnixStream::connect(&live_path).await.is_ok());
        drop(live);
    }

    #[tokio::test]
    async fn durable_watch_reports_status_and_delete_in_revision_order() {
        use tokio_stream::StreamExt;
        let h = harness().await;
        let obj = HomeObject::new(
            Kind::Want,
            "movie:tmdb:603",
            Spec::Want(WantSpec {
                title_id: "movie:tmdb:603".into(),
            }),
            StatusBody::empty(Kind::Want),
        );
        let stored = apply(&h.svc, Actor::Cli, &obj).await.expect("want");
        let updated = h
            .svc
            .inner
            .store
            .patch_status(
                Kind::Want,
                &stored.metadata.name,
                StatusBody::Want(mediaops_core::WantStatus {
                    phase: mediaops_core::WantPhase::Satisfied,
                }),
                stored.metadata.resource_version,
            )
            .await
            .expect("controller update");
        let deleted = h
            .svc
            .inner
            .store
            .delete(
                Kind::Want,
                &stored.metadata.name,
                updated.metadata.resource_version,
            )
            .await
            .expect("delete");
        let mut stream = h
            .svc
            .watch(request(
                Actor::Cli,
                WatchRequest {
                    kind: "Want".into(),
                    resource_version: stored.metadata.resource_version,
                },
            ))
            .await
            .expect("watch")
            .into_inner();
        for (expected, kind) in [
            (updated, WireWatchType::Modified),
            (deleted, WireWatchType::Deleted),
        ] {
            let event = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .expect("event promptly")
                .expect("event")
                .expect("ok");
            assert_eq!(event.r#type, kind as i32);
            assert_eq!(
                event
                    .object
                    .expect("object")
                    .metadata
                    .expect("meta")
                    .resource_version,
                expected.metadata.resource_version
            );
        }
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_secs() as i64
    }
}
