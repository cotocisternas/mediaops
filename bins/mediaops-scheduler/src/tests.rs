use super::*;
use mediaops_core::{
    CLUSTER_NAME, ClusterSpec, HomeError, JobSpec, JobStatus, NodeSpec, NodeStatus,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

struct FakeApi {
    objects: RefCell<Vec<HomeObject>>,
    patch_errors: RefCell<HashMap<String, ClientError>>,
    binds: RefCell<Vec<String>>,
}

impl FakeApi {
    fn ready(root: &Path) -> Self {
        Self {
            objects: RefCell::new(vec![
                HomeObject::new(
                    Kind::Cluster,
                    CLUSTER_NAME,
                    Spec::Cluster(ClusterSpec {
                        library_root: root.display().to_string(),
                        ..ClusterSpec::default()
                    }),
                    StatusBody::empty(Kind::Cluster),
                ),
                HomeObject::new(
                    Kind::Node,
                    WorkerKind::Pull.node_name(),
                    Spec::Node(NodeSpec {
                        worker_kind: WorkerKind::Pull,
                    }),
                    StatusBody::Node(NodeStatus {
                        ready: true,
                        last_heartbeat_unix: unix_now(),
                        ..NodeStatus::default()
                    }),
                ),
            ]),
            patch_errors: RefCell::new(HashMap::new()),
            binds: RefCell::new(Vec::new()),
        }
    }

    fn add_job(&self, job: HomeObject) {
        self.objects.borrow_mut().push(job);
    }

    fn fail(&self, name: &str, err: ClientError) {
        self.patch_errors.borrow_mut().insert(name.to_string(), err);
    }
}

impl SchedulerApi for FakeApi {
    async fn get(&self, kind: Kind, name: &str) -> Result<HomeObject, ClientError> {
        self.objects
            .borrow()
            .iter()
            .find(|o| o.kind == kind && o.metadata.name == name)
            .cloned()
            .ok_or_else(|| {
                HomeError::NotFound {
                    kind,
                    name: name.into(),
                }
                .into()
            })
    }

    async fn list(&self, kind: Option<Kind>) -> Result<Vec<HomeObject>, ClientError> {
        Ok(self
            .objects
            .borrow()
            .iter()
            .filter(|o| kind.is_none_or(|k| k == o.kind))
            .cloned()
            .collect())
    }

    async fn patch(
        &self,
        object: HomeObject,
        subresource: &str,
    ) -> Result<HomeObject, ClientError> {
        assert_eq!(subresource, "bind");
        if let Some(err) = self.patch_errors.borrow_mut().remove(&object.metadata.name) {
            return Err(err);
        }
        self.binds.borrow_mut().push(object.metadata.name.clone());
        Ok(object)
    }
}

fn pending_job(name: &str, title: &str, root: &Path, file_len: u64, max_copy: u64) -> HomeObject {
    HomeObject::new(
        Kind::Job,
        name,
        Spec::Job(JobSpec {
            title_id: title.into(),
            remote_root: "movies".into(),
            remote_path: format!("{name}.mkv"),
            dest_rel: format!("movies/{name}.mkv"),
            file_len,
            range_len: 4,
            range_concurrency: 1,
            max_copy,
            library_root: root.display().to_string(),
            worker_kind: WorkerKind::Pull.as_str().to_string(),
            ..JobSpec::default()
        }),
        StatusBody::Job(JobStatus::default()),
    )
}

fn temp_root() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mediaops-sched-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

#[tokio::test]
async fn bind_refusal_does_not_starve_later_job() {
    let root = temp_root();
    let api = FakeApi::ready(&root);
    api.add_job(pending_job("pull-a", "movie:key:alpha.1999", &root, 4, 0));
    api.add_job(pending_job("pull-b", "movie:key:beta.1999", &root, 4, 0));
    api.fail("pull-a", HomeError::Denied("want gone".into()).into());
    bind_pending(&api).await.expect("pass continues");
    assert_eq!(*api.binds.borrow(), ["pull-b"]);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn bind_conflict_does_not_reserve_or_starve() {
    let root = temp_root();
    let api = FakeApi::ready(&root);
    api.add_job(pending_job(
        "pull-a",
        "movie:key:alpha.1999",
        &root,
        100,
        150,
    ));
    api.add_job(pending_job(
        "pull-b",
        "movie:key:beta.1999",
        &root,
        100,
        150,
    ));
    api.fail(
        "pull-a",
        HomeError::Conflict {
            kind: Kind::Job,
            name: "pull-a".into(),
        }
        .into(),
    );
    bind_pending(&api).await.expect("pass continues");
    assert_eq!(*api.binds.borrow(), ["pull-b"]);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn bind_system_error_aborts_pass() {
    let root = temp_root();
    let api = FakeApi::ready(&root);
    api.add_job(pending_job("pull-a", "movie:key:alpha.1999", &root, 4, 0));
    api.add_job(pending_job("pull-b", "movie:key:beta.1999", &root, 4, 0));
    api.fail("pull-a", ClientError::Connect("api down".into()));
    bind_pending(&api)
        .await
        .expect_err("transport error aborts");
    assert!(api.binds.borrow().is_empty());
    let _ = std::fs::remove_dir_all(root);
}
