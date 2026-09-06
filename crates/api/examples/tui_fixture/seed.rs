use std::path::Path;

use mediaops_core::{
    Actor, CLUSTER_NAME, ClusterSpec, ClusterStatus, HomeObject, Kind, NodeSpec, NodeStatus, Spec,
    StatusBody, WorkerKind,
};
use mediaops_home_client::HomeApi;

use super::args::Mode;
use super::beat::{LIST_GENERATION, unix_now, wait_connect};
use super::errors::FixtureError;
use super::seed_rich::seed_rich;

pub const WANT_MATRIX: &str = "movie:tmdb:603";
pub const TITLE_ORPHAN: &str = "movie:tmdb:680";
pub const HOLD_OPEN_A: &str = "movie:tmdb:4539-watchable";
pub const HOLD_OPEN_B: &str = "movie:tmdb:4539-other";
pub const HOLD_APPROVED_NAME: &str = "movie:tmdb:550-cam";
pub const HOLD_REJECTED_NAME: &str = "movie:tmdb:13-web";
pub const JOB_PULLING: &str = "pull-movie-tmdb-603-whole";
pub const JOB_FAILED: &str = "pull-movie-tmdb-550-whole";
pub const QA_NOISE: &str = "長い名前\u{0007}\u{001b}[31mRED\u{001b}[0m\nnext";

pub async fn seed_fixture(
    cli: &HomeApi,
    socket: &Path,
    mode: Mode,
    library: &Path,
) -> Result<(), FixtureError> {
    std::fs::create_dir_all(library)?;
    match cli.get(Kind::Cluster, CLUSTER_NAME).await {
        Ok(_) => return Ok(()),
        Err(err) if err.is_not_found() => {}
        Err(err) => return Err(err.into()),
    }
    let now = unix_now()?;
    apply_cluster(cli, library).await?;
    match mode {
        Mode::Empty => seed_nodes(socket, true, now).await,
        Mode::NotReady => seed_nodes(socket, false, now).await,
        Mode::Rich => {
            seed_nodes(socket, true, now).await?;
            seed_rich(cli, socket, library, now).await
        }
    }
}

async fn apply_cluster(cli: &HomeApi, library: &Path) -> Result<(), FixtureError> {
    let library_root = library
        .to_str()
        .ok_or_else(|| FixtureError::Invalid("library path is not utf-8".into()))?
        .to_owned();
    cli.apply(HomeObject::new(
        Kind::Cluster,
        CLUSTER_NAME,
        Spec::Cluster(ClusterSpec {
            library_root,
            ..ClusterSpec::default()
        }),
        StatusBody::Cluster(ClusterStatus::default()),
    ))
    .await?;
    Ok(())
}

async fn seed_nodes(socket: &Path, ready: bool, now: i64) -> Result<(), FixtureError> {
    apply_node(
        socket,
        NodeSeed {
            worker: WorkerKind::Inventory,
            ready,
            now,
            listing: true,
        },
    )
    .await?;
    apply_node(
        socket,
        NodeSeed {
            worker: WorkerKind::Pull,
            ready,
            now,
            listing: false,
        },
    )
    .await?;
    apply_node(
        socket,
        NodeSeed {
            worker: WorkerKind::Scheduler,
            ready,
            now,
            listing: false,
        },
    )
    .await?;
    Ok(())
}

struct NodeSeed {
    worker: WorkerKind,
    ready: bool,
    now: i64,
    listing: bool,
}

async fn apply_node(socket: &Path, seed: NodeSeed) -> Result<(), FixtureError> {
    let actor = match seed.worker {
        WorkerKind::Inventory => Actor::Inventory,
        WorkerKind::Pull => Actor::Pull,
        WorkerKind::Scheduler => Actor::Scheduler,
    };
    let (beat, listed, generation) = if seed.ready {
        let listed = if seed.listing { seed.now } else { 0 };
        let generation = if seed.listing { LIST_GENERATION } else { 0 };
        (seed.now, listed, generation)
    } else {
        (
            seed.now.saturating_sub(120),
            seed.now.saturating_sub(120),
            0,
        )
    };
    let api = wait_connect(socket, actor).await?;
    api.apply(HomeObject::new(
        Kind::Node,
        seed.worker.node_name(),
        Spec::Node(NodeSpec {
            worker_kind: seed.worker,
        }),
        StatusBody::Node(NodeStatus {
            list_generation: generation,
            list_completed_unix: listed,
            ready: seed.ready,
            last_heartbeat_unix: beat,
        }),
    ))
    .await?;
    Ok(())
}
