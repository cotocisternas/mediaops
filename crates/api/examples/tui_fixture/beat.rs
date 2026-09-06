use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mediaops_core::{Actor, NODE_HEARTBEAT_SECS, WorkerKind};
use mediaops_home_client::HomeApi;

use super::errors::FixtureError;

pub const READY_DEADLINE: Duration = Duration::from_secs(5);
pub const LIST_GENERATION: i64 = 1;

pub async fn wait_connect(socket: &Path, actor: Actor) -> Result<HomeApi, FixtureError> {
    let start = Instant::now();
    loop {
        match HomeApi::connect(socket, actor).await {
            Ok(api) => return Ok(api),
            Err(err) if start.elapsed() < READY_DEADLINE => {
                drop(err);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(err) => return Err(err.into()),
        }
    }
}

pub async fn heartbeat_all(socket: &Path, now: i64) -> Result<(), FixtureError> {
    heartbeat_kind(socket, WorkerKind::Inventory, now).await?;
    heartbeat_kind(socket, WorkerKind::Pull, now).await?;
    heartbeat_kind(socket, WorkerKind::Scheduler, now).await?;
    Ok(())
}

async fn heartbeat_kind(socket: &Path, worker: WorkerKind, now: i64) -> Result<(), FixtureError> {
    let (actor, listing) = match worker {
        WorkerKind::Inventory => (Actor::Inventory, Some((LIST_GENERATION, now))),
        WorkerKind::Pull => (Actor::Pull, None),
        WorkerKind::Scheduler => (Actor::Scheduler, None),
    };
    let api = wait_connect(socket, actor).await?;
    api.heartbeat(worker, true, listing).await?;
    Ok(())
}

pub fn unix_now() -> Result<i64, FixtureError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .map_err(|err| FixtureError::Invalid(err.to_string()))
}

pub const fn heartbeat_period() -> Duration {
    Duration::from_secs(NODE_HEARTBEAT_SECS)
}
