//! Local Home API fixture for TUI QA. No workers, no seedbox.

use std::io::Write;

use mediaops_apiserver::{ApiConfig, serve_api};
use mediaops_core::Actor;

mod args;
mod beat;
mod errors;
mod scratch;
mod seed;
mod seed_holds;
mod seed_jobs;
mod seed_media;
mod seed_rich;

use args::{Mode, parse_launch};
use beat::{heartbeat_all, heartbeat_period, unix_now, wait_connect};
use errors::FixtureError;
use scratch::{Scratch, prepare_scratch};
use seed::seed_fixture;

#[tokio::main]
async fn main() -> Result<(), FixtureError> {
    let launch = parse_launch(std::env::args().skip(1))?;
    let scratch = prepare_scratch(&launch.dir)?;
    let mut server = tokio::spawn(serve_api(ApiConfig {
        socket: scratch.socket.clone(),
        api_db: scratch.api_db.clone(),
    }));
    let result = run(&scratch, launch.mode, &mut server).await;
    server.abort();
    result
}

async fn run(
    scratch: &Scratch,
    mode: Mode,
    server: &mut tokio::task::JoinHandle<Result<(), mediaops_apiserver::ApiError>>,
) -> Result<(), FixtureError> {
    let cli = wait_connect(&scratch.socket, Actor::Cli).await?;
    seed_fixture(&cli, &scratch.socket, mode, &scratch.library).await?;
    println!("{}", scratch.socket.display());
    std::io::stdout().flush()?;
    if !mode.heartbeats() {
        server
            .await
            .map_err(|err| FixtureError::Invalid(err.to_string()))?
            .map_err(|err| FixtureError::Invalid(err.to_string()))?;
        return Ok(());
    }
    let mut interval = tokio::time::interval(heartbeat_period());
    loop {
        tokio::select! {
            result = &mut *server => {
                return result
                    .map_err(|err| FixtureError::Invalid(err.to_string()))?
                    .map_err(|err| FixtureError::Invalid(err.to_string()));
            }
            _ = interval.tick() => {
                heartbeat_all(&scratch.socket, unix_now()?).await?;
            }
        }
    }
}
