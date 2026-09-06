use super::unmonitor::UnmonitorPort;
use super::*;
use mediaops_core::{
    Blake3Hex, ClusterSpec, Grabber, HoldDecisionSpec, HoldKey, HoldLiveItem, NodeSpec, NodeStatus,
    ReleaseId, RemoteEntry, RemoteRef, TitleFileStatus, TitleId, TitleSpec, TitleStatus,
};
use mediaops_home_client::ClientError;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

struct MemoryApi {
    objects: RefCell<Vec<HomeObject>>,
    fail_kind: Cell<Option<Kind>>,
    supersede_on_node_patch: Cell<bool>,
}

impl MemoryApi {
    fn new() -> Self {
        Self {
            objects: RefCell::new(vec![
                HomeObject::new(
                    Kind::Cluster,
                    mediaops_core::CLUSTER_NAME,
                    Spec::Cluster(ClusterSpec::default()),
                    StatusBody::empty(Kind::Cluster),
                ),
                HomeObject::new(
                    Kind::Node,
                    "inventory",
                    Spec::Node(NodeSpec {
                        worker_kind: WorkerKind::Inventory,
                    }),
                    StatusBody::Node(NodeStatus {
                        ready: true,
                        list_generation: 1,
                        list_completed_unix: unix_now(),
                        last_heartbeat_unix: unix_now(),
                        ..NodeStatus::default()
                    }),
                ),
            ]),
            fail_kind: Cell::new(None),
            supersede_on_node_patch: Cell::new(false),
        }
    }

    fn write(&self, mut object: HomeObject) -> Result<HomeObject, ClientError> {
        if self.fail_kind.get() == Some(object.kind) {
            return Err(
                mediaops_core::HomeError::Invalid("injected persistence failure".into()).into(),
            );
        }
        let mut objects = self.objects.borrow_mut();
        let stored_rv = objects
            .iter()
            .find(|o| o.kind == object.kind && o.metadata.name == object.metadata.name)
            .map(|o| o.metadata.resource_version);
        if let Some(stored_rv) = stored_rv
            && stored_rv != object.metadata.resource_version
        {
            return Err(mediaops_core::HomeError::Conflict {
                kind: object.kind,
                name: object.metadata.name.clone(),
            }
            .into());
        }
        object.metadata.resource_version += 1;
        if let Some(old) = objects
            .iter_mut()
            .find(|o| o.kind == object.kind && o.metadata.name == object.metadata.name)
        {
            *old = object.clone();
        } else {
            objects.push(object.clone());
        }
        Ok(object)
    }

    async fn marker(&self) -> NodeStatus {
        let node = self.get(Kind::Node, "inventory").await.expect("node");
        let StatusBody::Node(status) = node.status else {
            panic!("Node");
        };
        status
    }

    fn set_cluster(&self, spec: ClusterSpec) {
        let mut objects = self.objects.borrow_mut();
        let cluster = objects
            .iter_mut()
            .find(|o| o.kind == Kind::Cluster)
            .expect("cluster");
        cluster.spec = Spec::Cluster(spec);
    }
}

impl InventoryApi for MemoryApi {
    async fn get(&self, kind: Kind, name: &str) -> Result<HomeObject, ClientError> {
        self.objects
            .borrow()
            .iter()
            .find(|o| o.kind == kind && o.metadata.name == name)
            .cloned()
            .ok_or_else(|| {
                mediaops_core::HomeError::NotFound {
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
    async fn apply(&self, object: HomeObject) -> Result<HomeObject, ClientError> {
        self.write(object)
    }
    async fn patch(
        &self,
        object: HomeObject,
        subresource: &str,
    ) -> Result<HomeObject, ClientError> {
        assert_eq!(subresource, "status");
        if object.kind == Kind::Node && self.supersede_on_node_patch.get() {
            self.supersede_on_node_patch.set(false);
            let mut stored = self.get(Kind::Node, &object.metadata.name).await?;
            if let StatusBody::Node(status) = &mut stored.status {
                status.scan_started_rv = status.scan_started_rv.saturating_add(1);
            }
            self.write(stored)?;
        }
        self.write(object)
    }
    async fn delete(&self, kind: Kind, name: &str) -> Result<HomeObject, ClientError> {
        let old = self.get(kind, name).await?;
        self.objects
            .borrow_mut()
            .retain(|o| o.kind != kind || o.metadata.name != name);
        Ok(old)
    }
    async fn begin_inventory(&self) -> Result<HomeObject, ClientError> {
        let cluster = self.get(Kind::Cluster, mediaops_core::CLUSTER_NAME).await?;
        let secret_rv = match self.get(Kind::Secret, mediaops_core::SECRET_NAME).await {
            Ok(secret) => secret.metadata.resource_version,
            Err(err) if err.is_not_found() => 0,
            Err(err) => return Err(err),
        };
        let max_rv = self
            .objects
            .borrow()
            .iter()
            .map(|o| o.metadata.resource_version)
            .max()
            .unwrap_or(0);
        let mut node = self
            .get(Kind::Node, WorkerKind::Inventory.node_name())
            .await?;
        if let StatusBody::Node(status) = &mut node.status {
            status.ready = false;
            status.scan_started_rv = max_rv.saturating_add(1).max(1);
            status.scan_cluster_generation = cluster.metadata.generation;
            status.scan_secret_resource_version = secret_rv;
        }
        self.write(node)
    }
}

async fn scan(
    api: &MemoryApi,
    control: &Rejector,
    entries: Vec<RemoteEntry>,
    holds: Vec<HoldLiveItem>,
) -> anyhow::Result<()> {
    let started = api.begin_inventory().await?;
    let start = scan_start(&started)?;
    publish_inventory(api, control, entries, holds, start).await
}

#[derive(Default)]
struct Rejector {
    fail: Cell<bool>,
    calls: Cell<usize>,
    wanted: RefCell<Vec<TitleId>>,
    wanted_fail: Cell<bool>,
    wanted_calls: Cell<usize>,
    unmonitor_calls: RefCell<Vec<TitleId>>,
    unmonitor_fail: RefCell<HashSet<TitleId>>,
}
impl RejectRelease for Rejector {
    async fn reject(&self, _: &HoldKey) -> anyhow::Result<()> {
        self.calls.set(self.calls.get() + 1);
        if self.fail.get() {
            anyhow::bail!("injected box rejection failure");
        }
        Ok(())
    }
}

impl UnmonitorPort for Rejector {
    async fn wanted_missing(&self) -> anyhow::Result<Vec<TitleId>> {
        self.wanted_calls.set(self.wanted_calls.get() + 1);
        if self.wanted_fail.get() {
            anyhow::bail!("injected wanted_missing failure");
        }
        Ok(self.wanted.borrow().clone())
    }

    async fn unmonitor(&self, title_id: &TitleId) -> anyhow::Result<()> {
        self.unmonitor_calls.borrow_mut().push(title_id.clone());
        if self.unmonitor_fail.borrow().contains(title_id) {
            anyhow::bail!("injected unmonitor failure");
        }
        Ok(())
    }
}

fn entry() -> RemoteEntry {
    RemoteEntry::from_wire_parts(
        RemoteRef::from_wire_parts("box".into(), "Scene.Release.mkv".into()).expect("remote"),
        4,
        0,
        1,
    )
}

fn hold() -> HoldLiveItem {
    let mut item = HoldLiveItem::new(
        HoldKey::new(
            TitleId::movie("603").expect("title"),
            ReleaseId::parse("release").expect("release"),
        ),
        0,
        4,
        "manual import",
    );
    item.remote = Some(entry().r#ref().clone());
    item.placement = Some(mediaops_core::Placement::movie("The.Matrix", 1999, "mkv"));
    item
}

#[tokio::test]
async fn failed_publication_never_commits_and_empty_success_expires_old_holds() {
    let api = MemoryApi::new();
    let control = Rejector::default();
    api.fail_kind.set(Some(Kind::Hold));
    assert!(
        scan(&api, &control, vec![entry()], vec![hold()])
            .await
            .is_err()
    );
    let marker = api.marker().await;
    assert!(!marker.ready);
    assert_eq!(marker.list_generation, 1);
    assert_eq!(
        api.list(Some(Kind::RemoteFile))
            .await
            .expect("partial rows")
            .len(),
        1
    );
    api.fail_kind.set(None);
    scan(&api, &control, vec![entry()], vec![hold()])
        .await
        .expect("retry");
    let committed = api.marker().await;
    assert!(committed.ready);
    assert_eq!(
        committed.list_generation, 3,
        "partial generation is never reused"
    );
    let saved_hold = api.list(Some(Kind::Hold)).await.expect("hold").remove(0);
    assert!(
        matches!(saved_hold.status, StatusBody::Hold(s) if s.list_generation == committed.list_generation)
    );
    scan(&api, &control, vec![], vec![])
        .await
        .expect("empty listing");
    assert!(
        api.list(Some(Kind::RemoteFile))
            .await
            .expect("remotes")
            .is_empty()
    );
    let empty = api.marker().await;
    let old_hold = api.list(Some(Kind::Hold)).await.expect("history").remove(0);
    assert!(
        matches!(old_hold.status, StatusBody::Hold(s) if s.list_generation < empty.list_generation)
    );
}

#[tokio::test]
async fn failed_rejection_is_not_acknowledged_or_committed_and_retries_exact_key() {
    let api = MemoryApi::new();
    let control = Rejector::default();
    api.apply(HomeObject::new(
        Kind::Hold,
        "movie:tmdb:603-release",
        Spec::Hold(HoldSpec {
            title_id: "movie:tmdb:603".into(),
            release_id: "release".into(),
            decision: HoldDecisionSpec::Rejected,
        }),
        StatusBody::Hold(HoldStatus::default()),
    ))
    .await
    .expect("decision");
    control.fail.set(true);
    assert!(
        scan(&api, &control, vec![entry()], vec![hold()])
            .await
            .is_err()
    );
    assert!(!api.marker().await.ready);
    let observed = api
        .get(Kind::Hold, "movie:tmdb:603-release")
        .await
        .expect("hold");
    assert!(matches!(observed.status, StatusBody::Hold(s) if !s.rejection_observed));
    control.fail.set(false);
    scan(&api, &control, vec![entry()], vec![hold()])
        .await
        .expect("retry");
    assert_eq!(control.calls.get(), 2);
    assert!(api.marker().await.ready);
    scan(&api, &control, vec![], vec![])
        .await
        .expect("already acknowledged");
    assert_eq!(control.calls.get(), 2, "no repeated remote side effect");
}

#[tokio::test]
async fn abandoned_hold_only_generation_is_not_reused_by_empty_publication() {
    let api = MemoryApi::new();
    let control = Rejector::default();
    api.apply(HomeObject::new(
        Kind::Hold,
        "movie:tmdb:603-release",
        Spec::Hold(HoldSpec {
            title_id: "movie:tmdb:603".into(),
            release_id: "release".into(),
            decision: HoldDecisionSpec::Approved,
        }),
        StatusBody::Hold(HoldStatus {
            list_generation: 2,
            ..Default::default()
        }),
    ))
    .await
    .expect("partial Hold-only publication");
    scan(&api, &control, vec![], vec![])
        .await
        .expect("empty listing");
    assert_eq!(api.marker().await.list_generation, 3);
}

struct Library(PathBuf);

impl Library {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "mediaops-inventory-unmonitor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("library");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn place(&self, rel: &str) {
        let path = self.0.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(&path, b"ok").expect("file");
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn movie_id() -> TitleId {
    TitleId::movie("603").expect("movie")
}

fn album_id() -> TitleId {
    TitleId::album("0f82b02e-c6cd-4242-b195-93d4bf3e0d63").expect("album")
}

fn series_id() -> TitleId {
    TitleId::series("79126").expect("series")
}

const MOVIE_REL: &str = "movies/Coco.(2017)/Coco.(2017).mkv";
const ALBUM_REL: &str = "music/Tool/Lateralus.(2001)/01.The.Grudge.flac";
const SERIES_REL: &str = "series/Silo.(2023)/Season.01/Silo.(2023).S01E01.mkv";

fn digest() -> Blake3Hex {
    Blake3Hex::parse(&"a".repeat(64)).expect("digest")
}

fn title_with_files(id: &TitleId, files: &[(&str, bool)]) -> HomeObject {
    let digest = digest();
    let files: Vec<TitleFileStatus> = files
        .iter()
        .map(|(path, drifted)| TitleFileStatus {
            path: (*path).into(),
            install_b3: digest.clone(),
            current_b3: digest.clone(),
            drifted: *drifted,
        })
        .collect();
    let status = match files.first() {
        Some(first) => TitleStatus {
            path: first.path.clone(),
            install_b3: Some(first.install_b3.clone()),
            current_b3: Some(first.current_b3.clone()),
            drifted: files.iter().any(|file| file.drifted),
            files,
        },
        None => TitleStatus::default(),
    };
    HomeObject::new(
        Kind::Title,
        id.render(),
        Spec::Title(TitleSpec {
            title_id: id.render(),
            desired_present: true,
        }),
        StatusBody::Title(status),
    )
}

fn servarr_cluster(library: &Library, lock: bool) -> ClusterSpec {
    ClusterSpec {
        grabber: Grabber::Servarr,
        lock,
        library_root: library.path().display().to_string(),
        ..ClusterSpec::default()
    }
}

async fn seed_title(api: &MemoryApi, title: HomeObject) {
    api.apply(title).await.expect("title");
}

#[tokio::test]
async fn unmonitors_movie_and_album_when_installed_and_wanted_missing() {
    let library = Library::new();
    library.place(MOVIE_REL);
    library.place(ALBUM_REL);
    let api = MemoryApi::new();
    api.set_cluster(servarr_cluster(&library, false));
    let movie = movie_id();
    let album = album_id();
    seed_title(&api, title_with_files(&movie, &[(MOVIE_REL, false)])).await;
    seed_title(&api, title_with_files(&album, &[(ALBUM_REL, false)])).await;
    let control = Rejector::default();
    control
        .wanted
        .borrow_mut()
        .extend([movie.clone(), album.clone()]);

    scan(&api, &control, vec![], vec![]).await.expect("listing");

    let calls = control.unmonitor_calls.borrow().clone();
    assert!(calls.contains(&movie), "{calls:?}");
    assert!(calls.contains(&album), "{calls:?}");
    assert_eq!(calls.len(), 2, "{calls:?}");
}

#[tokio::test]
async fn unmonitors_when_lock_is_set_and_want_is_absent() {
    let library = Library::new();
    library.place(MOVIE_REL);
    let api = MemoryApi::new();
    api.set_cluster(servarr_cluster(&library, true));
    let movie = movie_id();
    seed_title(&api, title_with_files(&movie, &[(MOVIE_REL, false)])).await;
    let control = Rejector::default();
    control.wanted.borrow_mut().push(movie.clone());

    scan(&api, &control, vec![], vec![]).await.expect("listing");

    assert_eq!(*control.unmonitor_calls.borrow(), vec![movie]);
}

#[tokio::test]
async fn begin_inventory_writes_nonzero_start_token_and_clears_ready() {
    let api = MemoryApi::new();
    let before = api.marker().await;
    assert!(before.ready);
    assert_eq!(before.scan_started_rv, 0);
    let node = api.begin_inventory().await.expect("begin");
    let start = scan_start(&node).expect("token");
    assert!(start.started_rv > 0);
    let after = api.marker().await;
    assert!(!after.ready);
    assert_eq!(after.scan_started_rv, start.started_rv);
    assert_eq!(after.list_generation, before.list_generation);
}

#[tokio::test]
async fn publication_without_start_token_does_not_commit() {
    let api = MemoryApi::new();
    let control = Rejector::default();
    assert!(
        publish_inventory(
            &api,
            &control,
            vec![entry()],
            vec![hold()],
            ScanStart {
                started_rv: 0,
                cluster_generation: 0,
                secret_resource_version: 0,
            },
        )
        .await
        .is_err()
    );
    let marker = api.marker().await;
    assert!(marker.ready);
    assert_eq!(marker.list_generation, 1);
    assert!(
        api.list(Some(Kind::RemoteFile))
            .await
            .expect("remotes")
            .is_empty()
    );
}

#[tokio::test]
async fn successful_publication_preserves_scan_token() {
    let api = MemoryApi::new();
    let control = Rejector::default();
    let started = api.begin_inventory().await.expect("begin");
    let token = scan_start(&started).expect("token");
    publish_inventory(&api, &control, vec![entry()], vec![hold()], token)
        .await
        .expect("publish");
    let marker = api.marker().await;
    assert!(marker.ready);
    assert_eq!(marker.scan_started_rv, token.started_rv);
    assert_eq!(marker.scan_cluster_generation, token.cluster_generation);
    assert_eq!(
        marker.scan_secret_resource_version,
        token.secret_resource_version
    );
}

#[tokio::test]
async fn superseded_scan_token_does_not_commit_listing() {
    let api = MemoryApi::new();
    let control = Rejector::default();
    let started = api.begin_inventory().await.expect("begin");
    let token = scan_start(&started).expect("token");
    let mut node = api
        .get(Kind::Node, WorkerKind::Inventory.node_name())
        .await
        .expect("node");
    if let StatusBody::Node(status) = &mut node.status {
        status.scan_started_rv = token.started_rv.saturating_add(1);
    }
    api.write(node).expect("new token");
    assert!(
        publish_inventory(&api, &control, vec![entry()], vec![hold()], token)
            .await
            .is_err()
    );
    let marker = api.marker().await;
    assert!(!marker.ready);
    assert_eq!(marker.list_generation, 1);
}

#[tokio::test]
async fn supersession_at_final_commit_does_not_adopt_new_token() {
    let api = MemoryApi::new();
    let control = Rejector::default();
    let started = api.begin_inventory().await.expect("begin");
    let token = scan_start(&started).expect("token");
    api.supersede_on_node_patch.set(true);
    assert!(
        publish_inventory(&api, &control, vec![entry()], vec![hold()], token)
            .await
            .is_err()
    );
    let marker = api.marker().await;
    assert!(!marker.ready);
    assert_eq!(marker.list_generation, 1);
    assert!(marker.scan_started_rv > token.started_rv);
    assert_eq!(
        api.list(Some(Kind::RemoteFile))
            .await
            .expect("partial rows")
            .len(),
        1
    );
}

#[tokio::test]
async fn never_unmonitors_series_when_otherwise_eligible() {
    let library = Library::new();
    library.place(MOVIE_REL);
    library.place(SERIES_REL);
    let api = MemoryApi::new();
    api.set_cluster(servarr_cluster(&library, false));
    let movie = movie_id();
    let series = series_id();
    seed_title(&api, title_with_files(&movie, &[(MOVIE_REL, false)])).await;
    seed_title(&api, title_with_files(&series, &[(SERIES_REL, false)])).await;
    let control = Rejector::default();
    control
        .wanted
        .borrow_mut()
        .extend([movie.clone(), series.clone()]);

    scan(&api, &control, vec![], vec![]).await.expect("listing");

    assert_eq!(*control.unmonitor_calls.borrow(), vec![movie]);
}

#[tokio::test]
async fn makes_zero_control_calls_when_grabber_is_none() {
    let library = Library::new();
    library.place(MOVIE_REL);
    let api = MemoryApi::new();
    api.set_cluster(ClusterSpec {
        library_root: library.path().display().to_string(),
        ..ClusterSpec::default()
    });
    let movie = movie_id();
    seed_title(&api, title_with_files(&movie, &[(MOVIE_REL, false)])).await;
    let control = Rejector::default();
    control.wanted.borrow_mut().push(movie);

    scan(&api, &control, vec![], vec![]).await.expect("listing");

    assert_eq!(control.wanted_calls.get(), 0);
    assert!(control.unmonitor_calls.borrow().is_empty());
}

#[tokio::test]
async fn skips_unindexed_missing_drifted_and_nonmatching_titles() {
    let library = Library::new();
    library.place(MOVIE_REL);
    library.place("movies/Drifted.(1999)/Drifted.(1999).mkv");
    let api = MemoryApi::new();
    api.set_cluster(servarr_cluster(&library, false));
    let indexed = movie_id();
    let unindexed = TitleId::movie("604").expect("unindexed");
    let missing = TitleId::movie("605").expect("missing");
    let drifted = TitleId::movie("606").expect("drifted");
    let nonmatching = TitleId::movie("607").expect("nonmatching");
    seed_title(&api, title_with_files(&indexed, &[(MOVIE_REL, false)])).await;
    seed_title(&api, title_with_files(&unindexed, &[])).await;
    seed_title(
        &api,
        title_with_files(
            &missing,
            &[("movies/Missing.(1999)/Missing.(1999).mkv", false)],
        ),
    )
    .await;
    seed_title(
        &api,
        title_with_files(
            &drifted,
            &[("movies/Drifted.(1999)/Drifted.(1999).mkv", true)],
        ),
    )
    .await;
    seed_title(
        &api,
        title_with_files(
            &nonmatching,
            &[("movies/Coco.(2017)/Coco.(2017).mkv", false)],
        ),
    )
    .await;
    let control = Rejector::default();
    control.wanted.borrow_mut().extend([
        unindexed,
        missing,
        drifted,
        TitleId::movie("608").expect("absent"),
    ]);

    scan(&api, &control, vec![], vec![]).await.expect("listing");

    assert!(control.unmonitor_calls.borrow().is_empty());
}

#[tokio::test]
async fn continues_and_retries_when_one_unmonitor_fails() {
    let library = Library::new();
    library.place(MOVIE_REL);
    library.place(ALBUM_REL);
    let api = MemoryApi::new();
    api.set_cluster(servarr_cluster(&library, false));
    let movie = movie_id();
    let album = album_id();
    seed_title(&api, title_with_files(&movie, &[(MOVIE_REL, false)])).await;
    seed_title(&api, title_with_files(&album, &[(ALBUM_REL, false)])).await;
    let control = Rejector::default();
    control
        .wanted
        .borrow_mut()
        .extend([movie.clone(), album.clone()]);
    control.unmonitor_fail.borrow_mut().insert(movie.clone());

    scan(&api, &control, vec![], vec![]).await.expect("listing");
    let first = control.unmonitor_calls.borrow().clone();
    assert!(first.contains(&movie), "{first:?}");
    assert!(first.contains(&album), "{first:?}");
    assert_eq!(first.len(), 2, "{first:?}");
    assert!(api.marker().await.ready);

    scan(&api, &control, vec![], vec![]).await.expect("retry");
    let second = control.unmonitor_calls.borrow().clone();
    assert_eq!(second.len(), 4, "{second:?}");
    assert_eq!(
        second.iter().filter(|id| *id == &movie).count(),
        2,
        "{second:?}"
    );
}

#[tokio::test]
async fn keeps_listing_generation_when_wanted_missing_fails() {
    let library = Library::new();
    library.place(MOVIE_REL);
    let api = MemoryApi::new();
    api.set_cluster(servarr_cluster(&library, false));
    let movie = movie_id();
    seed_title(&api, title_with_files(&movie, &[(MOVIE_REL, false)])).await;
    let control = Rejector::default();
    control.wanted.borrow_mut().push(movie);
    control.wanted_fail.set(true);

    scan(&api, &control, vec![], vec![]).await.expect("listing");

    let marker = api.marker().await;
    assert!(marker.ready);
    assert_eq!(marker.list_generation, 2);
    assert_eq!(control.wanted_calls.get(), 1);
    assert!(control.unmonitor_calls.borrow().is_empty());
}

#[tokio::test]
async fn unmonitors_once_per_title_when_observations_are_duplicated() {
    let library = Library::new();
    library.place(MOVIE_REL);
    library.place("movies/Coco.(2017)/Coco.(2017).en.srt");
    let api = MemoryApi::new();
    api.set_cluster(servarr_cluster(&library, false));
    let movie = movie_id();
    seed_title(
        &api,
        title_with_files(
            &movie,
            &[
                (MOVIE_REL, false),
                ("movies/Coco.(2017)/Coco.(2017).en.srt", false),
            ],
        ),
    )
    .await;
    let control = Rejector::default();
    control
        .wanted
        .borrow_mut()
        .extend([movie.clone(), movie.clone()]);

    scan(&api, &control, vec![], vec![]).await.expect("listing");

    assert_eq!(*control.unmonitor_calls.borrow(), vec![movie]);
}
