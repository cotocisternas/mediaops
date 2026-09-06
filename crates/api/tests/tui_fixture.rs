//! Home API fixture contract: args, scratch, seed, restart. No TUI types.

mod support;

#[allow(dead_code)]
#[path = "../examples/tui_fixture/args.rs"]
mod args;
#[allow(dead_code)]
#[path = "../examples/tui_fixture/beat.rs"]
mod beat;
#[allow(dead_code)]
#[path = "../examples/tui_fixture/errors.rs"]
mod errors;
#[allow(dead_code)]
#[path = "../examples/tui_fixture/scratch.rs"]
mod scratch;
#[allow(dead_code)]
#[path = "../examples/tui_fixture/seed.rs"]
mod seed;
#[allow(dead_code)]
#[path = "../examples/tui_fixture/seed_holds.rs"]
mod seed_holds;
#[allow(dead_code)]
#[path = "../examples/tui_fixture/seed_jobs.rs"]
mod seed_jobs;
#[allow(dead_code)]
#[path = "../examples/tui_fixture/seed_media.rs"]
mod seed_media;
#[allow(dead_code)]
#[path = "../examples/tui_fixture/seed_rich.rs"]
mod seed_rich;

use std::path::Path;

use args::{Mode, parse_launch};
use errors::FixtureError;
use mediaops_core::{HoldDecisionSpec, JobPhase, Kind, Spec, StatusBody, WantSpec, node_is_ready};
use mediaops_home_client::HomeApi;
use scratch::prepare_scratch;
use seed::{
    HOLD_APPROVED_NAME, HOLD_OPEN_A, HOLD_OPEN_B, HOLD_REJECTED_NAME, JOB_FAILED, JOB_PULLING,
    TITLE_ORPHAN, WANT_MATRIX, seed_fixture,
};
use support::TestApi;

#[test]
fn unknown_mode_is_rejected_before_disk() {
    let dir = std::env::temp_dir().join(format!("mediaops-tui-qa-never-{}", std::process::id()));
    assert!(!dir.exists());
    let err = parse_launch([dir.to_str().expect("utf8"), "staging"]).unwrap_err();
    assert!(matches!(err, FixtureError::UnknownMode(mode) if mode == "staging"));
    assert!(!dir.exists());
}

#[test]
fn extra_args_and_relative_dir_are_rejected() {
    assert!(matches!(
        parse_launch(["/tmp/opencode/mediaops-tui-qa-x", "rich", "extra"]),
        Err(FixtureError::Usage)
    ));
    assert!(matches!(
        parse_launch(["relative/scratch", "empty"]),
        Err(FixtureError::ScratchNotDedicated)
    ));
    assert!(matches!(
        parse_launch(Vec::<&str>::new()),
        Err(FixtureError::Usage)
    ));
}

#[test]
fn git_work_tree_is_not_a_dedicated_scratch() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(matches!(
        prepare_scratch(repo),
        Err(FixtureError::ScratchNotDedicated)
    ));
}

#[tokio::test]
async fn test_api_dirs_are_unique_without_preemptive_wipe() {
    let a = TestApi::start("dup").await;
    let b = TestApi::start("dup").await;
    assert_ne!(a.dir, b.dir);
    assert!(a.dir.exists());
    assert!(b.dir.exists());
}

#[tokio::test]
async fn rich_seed_is_legal_and_restart_preserves_objects() {
    let home = TestApi::start("rich").await;
    let library = home.dir.join("library");
    seed_fixture(&home.api, &home.socket, Mode::Rich, &library)
        .await
        .expect("seed");
    assert_rich(&home.api).await;

    seed_fixture(&home.api, &home.socket, Mode::Rich, &library)
        .await
        .expect("restart");
    assert_rich(&home.api).await;
}

#[tokio::test]
async fn empty_seed_has_ready_nodes_and_no_wants() {
    let home = TestApi::start("fix-empty").await;
    let library = home.dir.join("library");
    seed_fixture(&home.api, &home.socket, Mode::Empty, &library)
        .await
        .expect("seed");
    let wants = home.api.list(Some(Kind::Want)).await.expect("wants");
    assert!(wants.is_empty());
    let inv = home
        .api
        .get(Kind::Node, "inventory")
        .await
        .expect("inventory");
    let StatusBody::Node(st) = &inv.status else {
        panic!("node status");
    };
    assert!(node_is_ready(st.ready, st.last_heartbeat_unix, now_unix()));
}

#[tokio::test]
async fn not_ready_seed_keeps_inventory_unusable() {
    let home = TestApi::start("fix-nr").await;
    let library = home.dir.join("library");
    seed_fixture(&home.api, &home.socket, Mode::NotReady, &library)
        .await
        .expect("seed");
    let inv = home
        .api
        .get(Kind::Node, "inventory")
        .await
        .expect("inventory");
    let StatusBody::Node(st) = &inv.status else {
        panic!("node status");
    };
    assert!(!node_is_ready(st.ready, st.last_heartbeat_unix, now_unix()));
}

async fn assert_rich(api: &HomeApi) {
    let pulling = api.get(Kind::Job, JOB_PULLING).await.expect("pulling job");
    match (&pulling.spec, &pulling.status) {
        (Spec::Job(spec), StatusBody::Job(st)) => {
            assert_eq!(spec.title_id, WANT_MATRIX);
            assert_eq!(st.phase, JobPhase::Pulling);
            assert!(st.bytes_done > 0);
            assert!(!spec.node_name.is_empty());
        }
        _ => panic!("job body"),
    }
    let failed = api.get(Kind::Job, JOB_FAILED).await.expect("failed job");
    assert!(matches!(
        failed.status,
        StatusBody::Job(st) if st.phase == JobPhase::Failed && !st.message.is_empty()
    ));

    let open_a = api.get(Kind::Hold, HOLD_OPEN_A).await.expect("open a");
    let open_b = api.get(Kind::Hold, HOLD_OPEN_B).await.expect("open b");
    let approved = api
        .get(Kind::Hold, HOLD_APPROVED_NAME)
        .await
        .expect("approved");
    let rejected = api
        .get(Kind::Hold, HOLD_REJECTED_NAME)
        .await
        .expect("rejected");
    assert!(matches!(open_a.spec, Spec::Hold(s) if s.decision == HoldDecisionSpec::Empty));
    assert!(matches!(open_b.spec, Spec::Hold(s) if s.decision == HoldDecisionSpec::Empty));
    assert!(matches!(
        approved.spec,
        Spec::Hold(s) if s.decision == HoldDecisionSpec::Approved
    ));
    assert!(matches!(
        rejected.spec,
        Spec::Hold(s) if s.decision == HoldDecisionSpec::Rejected
    ));
    assert!(matches!(
        open_a.status,
        StatusBody::Hold(st) if st.placement.is_some() && st.list_generation == 1
    ));

    api.get(Kind::Title, TITLE_ORPHAN)
        .await
        .expect("orphan title");
    let wants = api.list(Some(Kind::Want)).await.expect("wants");
    assert!(wants.iter().any(|w| matches!(
        &w.spec,
        Spec::Want(WantSpec { title_id }) if title_id == WANT_MATRIX
    )));
    assert!(!wants.iter().any(|w| w.metadata.name == TITLE_ORPHAN));
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .expect("clock")
}
