use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mediaops_core::{
    Actor, Bytes, CLUSTER_NAME, ClusterSpec, HomeObject, Kind, PathRoot, RemoteFileStatus, Spec,
    StatusBody, SyncPhase, TitleKind, WorkerKind,
};
use mediaops_home_client::{ClientError, HomeApi};

use crate::support::TestApi;

pub const MATRIX: &str = "The.Matrix.(1999)/The.Matrix.(1999).mkv";

pub async fn configured(tag: &str) -> TestApi {
    let home = TestApi::start(tag).await;
    let root = home.dir.join("library");
    std::fs::create_dir_all(&root).expect("library");
    home.api
        .apply(HomeObject::new(
            Kind::Cluster,
            CLUSTER_NAME,
            Spec::Cluster(ClusterSpec {
                library_root: root.to_string_lossy().into_owned(),
                max_copy: Bytes::new(1024 * 1024 * 1024),
                roots: vec![PathRoot {
                    id: "box".into(),
                    path: "/seedbox/media".into(),
                    kind: Some(TitleKind::Movie),
                }],
                ..Default::default()
            }),
            StatusBody::empty(Kind::Cluster),
        ))
        .await
        .expect("cluster");
    home
}

pub fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs(),
    )
    .expect("unix")
}

pub async fn inventory(home: &TestApi) -> HomeApi {
    HomeApi::connect(&home.socket, Actor::Inventory)
        .await
        .expect("inventory")
}

pub async fn publish(home: &TestApi, generation: i64, paths: &[&str]) {
    let api = inventory(home).await;
    api.begin_inventory().await.expect("begin scan");
    rows(&api, generation, paths).await;
    api.heartbeat(WorkerKind::Inventory, true, Some((generation, now())))
        .await
        .expect("commit listing");
}

pub async fn rows(api: &HomeApi, generation: i64, paths: &[&str]) {
    for path in paths {
        let title_id =
            mediaops_core::parse_remote(Some(TitleKind::Movie), std::path::Path::new(path))
                .expect("parse")
                .0
                .render();
        let name = mediaops_core::remote_file_name("box", path);
        let mut obj = HomeObject::new(
            Kind::RemoteFile,
            &name,
            Spec::RemoteFile,
            StatusBody::RemoteFile(RemoteFileStatus {
                root_id: "box".into(),
                rel_path: (*path).into(),
                len: 4,
                parse_ok: true,
                title_id,
                list_generation: generation,
            }),
        );
        if let Ok(old) = api.get(Kind::RemoteFile, &name).await {
            obj.metadata = old.metadata;
        }
        api.apply(obj).await.expect("remote");
    }
}

pub async fn start(
    home: &TestApi,
    id: &str,
) -> tokio::task::JoinHandle<Result<HomeObject, ClientError>> {
    let mut watch = home
        .api
        .watch_home(Some(Kind::Sync), 0)
        .await
        .expect("watch");
    let client = home.api.clone();
    let name = id.to_string();
    let task = tokio::spawn(async move { client.sync(&name, false).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = watch
                .message()
                .await
                .expect("watch message")
                .expect("event");
            if event.object().metadata.name == id {
                break;
            }
        }
    })
    .await
    .expect("request accepted");
    task
}

pub async fn scheduled(
    task: tokio::task::JoinHandle<Result<HomeObject, ClientError>>,
) -> HomeObject {
    let obj = tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .expect("planning completion")
        .expect("join")
        .expect("sync");
    assert!(matches!(&obj.status, StatusBody::Sync(st) if st.phase == SyncPhase::Scheduled));
    obj
}
