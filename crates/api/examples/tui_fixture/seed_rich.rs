use std::path::Path;

use mediaops_core::{HomeObject, JobPhase, Kind, Spec, StatusBody};
use mediaops_home_client::HomeApi;

use super::errors::FixtureError;
use super::seed::{
    HOLD_APPROVED_NAME, JOB_FAILED, JOB_PULLING, QA_NOISE, TITLE_ORPHAN, WANT_MATRIX,
};
use super::seed_holds::seed_holds;
use super::seed_jobs::{JobSeed, seed_job};
use super::seed_media::{apply_remote, apply_title, apply_want, movie_file};

const FILE_LEN: u64 = 4_000_000;
const BYTES_DONE: u64 = 1_200_000;
const FAIL_LEN: u64 = 4_000;

pub(super) async fn seed_rich(
    cli: &HomeApi,
    socket: &Path,
    library: &Path,
    now: i64,
) -> Result<(), FixtureError> {
    let matrix = movie_file(WANT_MATRIX, "The.Matrix", 1999)?;
    let fight = movie_file("movie:tmdb:550", "Fight.Club", 1999)?;
    apply_want(cli, WANT_MATRIX).await?;
    apply_title(cli, WANT_MATRIX).await?;
    apply_title(cli, TITLE_ORPHAN).await?;
    apply_remote(socket, &matrix, FILE_LEN).await?;
    apply_remote(socket, &fight, FAIL_LEN).await?;
    seed_job(
        socket,
        library,
        JobSeed {
            name: JOB_PULLING,
            file: &matrix,
            file_len: FILE_LEN,
            hold_name: "",
            phase: JobPhase::Pulling,
            bytes_done: BYTES_DONE,
            message: String::new(),
            now,
        },
    )
    .await?;
    seed_holds(cli, socket, now, &fight).await?;
    seed_job(
        socket,
        library,
        JobSeed {
            name: JOB_FAILED,
            file: &fight,
            file_len: FAIL_LEN,
            hold_name: HOLD_APPROVED_NAME,
            phase: JobPhase::Failed,
            bytes_done: 0,
            message: QA_NOISE.to_owned(),
            now,
        },
    )
    .await?;
    apply_event(cli, now).await?;
    Ok(())
}

async fn apply_event(cli: &HomeApi, now: i64) -> Result<(), FixtureError> {
    cli.apply(HomeObject::new(
        Kind::Event,
        "qa-synthetic-failure",
        Spec::Event,
        StatusBody::Event(mediaops_core::EventStatus {
            involved_kind: Kind::Job.as_str().into(),
            involved_name: JOB_FAILED.into(),
            reason: "SyntheticFailure".into(),
            message: QA_NOISE.into(),
            ts: now,
        }),
    ))
    .await?;
    Ok(())
}
