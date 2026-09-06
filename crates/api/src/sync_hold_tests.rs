use mediaops_core::{
    ClusterSpec, HoldDecisionSpec, HoldSpec, HoldStatus, HomeObject, Kind, NodeSpec, NodeStatus,
    PathRoot, Placement, RemoteFileStatus, Spec, StatusBody, SyncDisposition, SyncPhase, SyncSpec,
    SyncStatus, TitleKind, WorkerKind,
};

fn objects(competing: Option<HoldDecisionSpec>) -> (Vec<HomeObject>, HomeObject) {
    let config = ClusterSpec {
        library_root: "/tmp/library".into(),
        roots: vec![PathRoot {
            id: "box".into(),
            path: "/box".into(),
            kind: Some(TitleKind::Movie),
        }],
        ..Default::default()
    };
    let remote = RemoteFileStatus {
        root_id: "box".into(),
        rel_path: "The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
        title_id: "movie:key:thematrix.1999".into(),
        len: 4,
        parse_ok: true,
        list_generation: 1,
    };
    let mut objects = vec![
        HomeObject::new(
            Kind::Cluster,
            "home",
            Spec::Cluster(config.clone()),
            StatusBody::empty(Kind::Cluster),
        ),
        HomeObject::new(
            Kind::RemoteFile,
            "box/matrix",
            Spec::RemoteFile,
            StatusBody::RemoteFile(remote.clone()),
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
                last_heartbeat_unix: crate::sync::now(),
                list_completed_unix: crate::sync::now(),
                ..Default::default()
            }),
        ),
    ];
    for (release, decision) in [
        ("approved", Some(HoldDecisionSpec::Approved)),
        ("competing", competing),
    ] {
        if let Some(decision) = decision {
            objects.push(HomeObject::new(
                Kind::Hold,
                format!("movie:tmdb:603-{release}"),
                Spec::Hold(HoldSpec {
                    title_id: "movie:tmdb:603".into(),
                    release_id: release.into(),
                    decision,
                }),
                StatusBody::Hold(HoldStatus {
                    list_generation: 1,
                    remote_root: remote.root_id.clone(),
                    remote_path: remote.rel_path.clone(),
                    placement: Some(Placement::movie("The.Matrix", 1999, "mkv")),
                    ..Default::default()
                }),
            ));
        }
    }
    let request = HomeObject::new(
        Kind::Sync,
        "sync-hold",
        Spec::Sync(SyncSpec::default()),
        StatusBody::Sync(SyncStatus {
            phase: SyncPhase::Captured,
            list_generation: 1,
            cluster: Some(config),
            ..Default::default()
        }),
    );
    (objects, request)
}

#[test]
fn one_approval_cannot_override_another_hold_on_the_same_source() {
    for decision in [
        HoldDecisionSpec::Empty,
        HoldDecisionSpec::Rejected,
        HoldDecisionSpec::Approved,
    ] {
        let (objects, request) = objects(Some(decision));
        let entries = crate::sync_plan::capture(&objects, &request);
        assert_eq!(
            entries[0].disposition,
            SyncDisposition::Blocked,
            "{decision:?}"
        );
    }
}

#[test]
fn new_hold_conflict_is_rechecked_at_authorization() {
    let (initial, request) = objects(None);
    let entries = crate::sync_plan::capture(&initial, &request);
    let job = entries[0].job.as_ref().expect("approved source");
    assert!(crate::controllers::authorization_refusal(&initial, job, crate::sync::now()).is_none());
    let (changed, _) = objects(Some(HoldDecisionSpec::Approved));
    assert!(crate::controllers::authorization_refusal(&changed, job, crate::sync::now()).is_some());
}
