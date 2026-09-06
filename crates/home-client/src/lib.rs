//! Typed Home API client. Sets `x-mediaops-actor`. Never opens sqlite.

mod paths;
mod watch;

pub use mediaops_proto::WatchEvent;
pub use paths::{
    default_api_socket, default_config_dir, default_gateway_socket, default_state_dir,
    default_tls_dir,
};
pub use watch::HomeWatch;

/// Independent process ownership, separate from the legacy library flock.
/// Keep the returned file alive for the role's entire lifetime.
pub fn claim_process(socket: &std::path::Path, role: &str) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let path = socket.with_extension(format!("{role}.lock"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("process lock must be a regular file"));
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.try_lock().map_err(std::io::Error::other)?;
    Ok(file)
}

use std::path::{Path, PathBuf};
use std::time::Duration;

use http::Uri;
use hyper_util::rt::TokioIo;
use mediaops_core::{ACTOR_HEADER, Actor, HomeError, HomeObject, Kind};
use mediaops_proto::home::home_service_client::HomeServiceClient;
use mediaops_proto::home::{
    ApplyRequest, DeleteRequest, GetRequest, ListRequest, PatchRequest, ReconcileRequest,
    WatchRequest,
};
use mediaops_proto::{home_object_from_wire, home_object_to_wire};
use tokio::net::UnixStream;
use tonic::Request;
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("{0}")]
    Home(#[from] HomeError),
    #[error("connect: {0}")]
    Connect(String),
    #[error("rpc ({code}): {message}")]
    Rpc { code: tonic::Code, message: String },
}

impl ClientError {
    pub(crate) fn from_status(status: tonic::Status) -> Self {
        Self::Rpc {
            code: status.code(),
            message: mediaops_proto::error_detail_from_status(&status)
                .map(|d| d.message)
                .unwrap_or_else(|_| status.message().to_owned()),
        }
    }

    pub fn is_not_found(&self) -> bool {
        matches!(
            self,
            Self::Home(HomeError::NotFound { .. })
                | Self::Rpc {
                    code: tonic::Code::NotFound,
                    ..
                }
        )
    }

    pub fn is_conflict(&self) -> bool {
        matches!(
            self,
            Self::Home(HomeError::Conflict { .. })
                | Self::Rpc {
                    code: tonic::Code::FailedPrecondition | tonic::Code::AlreadyExists,
                    ..
                }
        )
    }

    pub fn is_denied(&self) -> bool {
        matches!(
            self,
            Self::Home(HomeError::Denied(_))
                | Self::Rpc {
                    code: tonic::Code::PermissionDenied,
                    ..
                }
        )
    }

    /// Transport may have delivered the write; do not resend.
    pub fn is_uncertain(&self) -> bool {
        matches!(
            self,
            Self::Connect(_)
                | Self::Rpc {
                    code: tonic::Code::DeadlineExceeded
                        | tonic::Code::Unavailable
                        | tonic::Code::Cancelled,
                    ..
                }
        )
    }
}

#[derive(Debug, Clone)]
pub struct HomeApi {
    inner: HomeServiceClient<Channel>,
    actor: Actor,
}

impl HomeApi {
    pub async fn heartbeat(
        &self,
        worker: mediaops_core::WorkerKind,
        ready: bool,
        completed_listing: Option<(i64, i64)>,
    ) -> Result<HomeObject, ClientError> {
        self.update_heartbeat(worker, Some(ready), completed_listing)
            .await
    }

    /// Refresh liveness without changing the publisher-owned readiness/commit marker.
    pub async fn touch_heartbeat(
        &self,
        worker: mediaops_core::WorkerKind,
    ) -> Result<HomeObject, ClientError> {
        self.update_heartbeat(worker, None, None).await
    }

    async fn update_heartbeat(
        &self,
        worker: mediaops_core::WorkerKind,
        ready: Option<bool>,
        completed_listing: Option<(i64, i64)>,
    ) -> Result<HomeObject, ClientError> {
        use mediaops_core::{NodeSpec, NodeStatus, Spec, StatusBody};
        for attempt in 0..3 {
            let (mut node, create) = match self.get(Kind::Node, worker.node_name()).await {
                Ok(node) => (node, false),
                Err(err) if err.is_not_found() => (
                    HomeObject::new(
                        Kind::Node,
                        worker.node_name(),
                        Spec::Node(NodeSpec {
                            worker_kind: worker,
                        }),
                        StatusBody::Node(NodeStatus::default()),
                    ),
                    true,
                ),
                Err(err) => return Err(err),
            };
            let StatusBody::Node(status) = &mut node.status else {
                return Err(HomeError::Invalid("Node status missing".into()).into());
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            update_node_heartbeat(status, ready, completed_listing, now);
            let result = if create {
                self.apply(node).await
            } else {
                self.patch(node, "status").await
            };
            match result {
                Err(err) if err.is_conflict() && attempt < 2 => continue,
                result => return result,
            }
        }
        unreachable!()
    }

    pub async fn connect(socket: impl AsRef<Path>, actor: Actor) -> Result<Self, ClientError> {
        let channel = connect_unix_plain(socket.as_ref()).await?;
        Ok(Self {
            inner: HomeServiceClient::new(channel),
            actor,
        })
    }

    fn attach<T>(&self, body: T) -> Request<T> {
        let mut req = Request::new(body);
        req.metadata_mut().insert(
            ACTOR_HEADER,
            self.actor.as_str().parse().expect("actor is ascii"),
        );
        req
    }

    pub async fn get(&self, kind: Kind, name: &str) -> Result<HomeObject, ClientError> {
        let resp = self
            .inner
            .clone()
            .get(self.attach(GetRequest {
                kind: kind.as_str().to_string(),
                name: name.to_string(),
            }))
            .await
            .map_err(ClientError::from_status)?;
        let obj = resp
            .into_inner()
            .object
            .ok_or_else(|| ClientError::Home(HomeError::Invalid("empty get response".into())))?;
        home_object_from_wire(obj).map_err(ClientError::Home)
    }

    pub async fn list(&self, kind: Option<Kind>) -> Result<Vec<HomeObject>, ClientError> {
        let resp = self
            .inner
            .clone()
            .list(self.attach(ListRequest {
                kind: kind.map(|k| k.as_str().to_string()).unwrap_or_default(),
            }))
            .await
            .map_err(ClientError::from_status)?;
        resp.into_inner()
            .items
            .into_iter()
            .map(|o| home_object_from_wire(o).map_err(ClientError::Home))
            .collect()
    }

    pub async fn apply(&self, obj: HomeObject) -> Result<HomeObject, ClientError> {
        let resp = self
            .inner
            .clone()
            .apply(self.attach(ApplyRequest {
                object: Some(home_object_to_wire(&obj)),
            }))
            .await
            .map_err(ClientError::from_status)?;
        let obj = resp
            .into_inner()
            .object
            .ok_or_else(|| ClientError::Home(HomeError::Invalid("empty apply response".into())))?;
        home_object_from_wire(obj).map_err(ClientError::Home)
    }

    pub async fn patch(
        &self,
        obj: HomeObject,
        subresource: &str,
    ) -> Result<HomeObject, ClientError> {
        let resp = self
            .inner
            .clone()
            .patch(self.attach(PatchRequest {
                object: Some(home_object_to_wire(&obj)),
                subresource: subresource.to_string(),
            }))
            .await
            .map_err(ClientError::from_status)?;
        let obj = resp
            .into_inner()
            .object
            .ok_or_else(|| ClientError::Home(HomeError::Invalid("empty patch response".into())))?;
        home_object_from_wire(obj).map_err(ClientError::Home)
    }

    pub async fn delete(&self, kind: Kind, name: &str) -> Result<HomeObject, ClientError> {
        let version = self.get(kind, name).await?.metadata.resource_version;
        self.delete_at_version(kind, name, version).await
    }

    pub async fn delete_at_version(
        &self,
        kind: Kind,
        name: &str,
        resource_version: i64,
    ) -> Result<HomeObject, ClientError> {
        let resp = self
            .inner
            .clone()
            .delete(self.attach(DeleteRequest {
                resource_version,
                kind: kind.as_str().to_string(),
                name: name.to_string(),
            }))
            .await
            .map_err(ClientError::from_status)?;
        let obj = resp
            .into_inner()
            .object
            .ok_or_else(|| ClientError::Home(HomeError::Invalid("empty delete response".into())))?;
        home_object_from_wire(obj).map_err(ClientError::Home)
    }

    pub async fn reconcile(&self) -> Result<i64, ClientError> {
        let resp = self
            .inner
            .clone()
            .reconcile(self.attach(ReconcileRequest {}))
            .await
            .map_err(ClientError::from_status)?;
        Ok(resp.into_inner().reconcile_generation)
    }

    pub async fn watch(
        &self,
        kind: Option<Kind>,
        resource_version: i64,
    ) -> Result<tonic::Streaming<mediaops_proto::home::WatchResponse>, ClientError> {
        let resp = self
            .inner
            .clone()
            .watch(self.attach(WatchRequest {
                kind: kind.map(|k| k.as_str().to_string()).unwrap_or_default(),
                resource_version,
            }))
            .await
            .map_err(ClientError::from_status)?;
        Ok(resp.into_inner())
    }

    pub async fn watch_home(
        &self,
        kind: Option<Kind>,
        resource_version: i64,
    ) -> Result<HomeWatch, ClientError> {
        Ok(HomeWatch::new(self.watch(kind, resource_version).await?))
    }
}

fn update_node_heartbeat(
    status: &mut mediaops_core::NodeStatus,
    ready: Option<bool>,
    listing: Option<(i64, i64)>,
    now: i64,
) {
    status.last_heartbeat_unix = now;
    if let Some(ready) = ready {
        status.ready = ready;
    }
    if let Some((generation, completed)) = listing {
        status.list_generation = generation;
        status.list_completed_unix = completed;
    }
}

pub async fn connect_unix_plain(path: &Path) -> Result<Channel, ClientError> {
    let path = PathBuf::from(path);
    let display_path = path.display().to_string();
    if let Err(err) = UnixStream::connect(&path).await {
        return Err(ClientError::Connect(format!("{}: {err}", path.display())));
    }
    let svc = service_fn(move |_: Uri| {
        let path = path.clone();
        async move {
            let stream = UnixStream::connect(path).await?;
            Ok::<_, std::io::Error>(TokioIo::new(stream))
        }
    });
    Endpoint::from_shared("http://mediaops-api")
        .map_err(|e| ClientError::Connect(e.to_string()))?
        .connect_timeout(Duration::from_secs(10))
        .connect_with_connector(svc)
        .await
        .map_err(|e| ClientError::Connect(format!("{display_path}: {e}")))
}

#[cfg(test)]
mod client_error_tests {
    use super::*;

    #[test]
    fn denied_matches_home_error_and_permission_denied_code() {
        let home = ClientError::Home(HomeError::Denied("no".into()));
        assert!(home.is_denied());
        assert!(!home.is_not_found());
        assert!(!home.is_conflict());
        let rpc = ClientError::Rpc {
            code: tonic::Code::PermissionDenied,
            message: "no".into(),
        };
        assert!(rpc.is_denied());
        let internal = ClientError::Rpc {
            code: tonic::Code::Internal,
            message: "denied".into(),
        };
        assert!(!internal.is_denied());
        assert!(!ClientError::Connect("denied".into()).is_denied());
    }
}

#[cfg(test)]
mod heartbeat_tests {
    use super::*;

    #[test]
    fn liveness_only_heartbeat_preserves_both_publication_states() {
        let mut status = mediaops_core::NodeStatus::default();
        update_node_heartbeat(&mut status, Some(true), Some((7, 100)), 100);
        update_node_heartbeat(&mut status, Some(false), None, 101);
        // A touch retried after invalidation must not restore captured readiness.
        update_node_heartbeat(&mut status, None, None, 102);
        assert!(!status.ready);
        assert_eq!(
            (status.list_generation, status.list_completed_unix),
            (7, 100)
        );
        update_node_heartbeat(&mut status, Some(true), Some((8, 103)), 103);
        update_node_heartbeat(&mut status, None, None, 104);
        assert!(status.ready);
        assert_eq!(
            (status.list_generation, status.list_completed_unix),
            (8, 103)
        );
        assert_eq!(status.last_heartbeat_unix, 104);
    }
}
