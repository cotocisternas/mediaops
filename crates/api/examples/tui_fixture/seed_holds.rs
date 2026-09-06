use std::path::Path;

use mediaops_core::{
    Actor, HoldDecisionSpec, HoldSpec, HoldStatus, HomeObject, Kind, Placement, Spec, StatusBody,
};
use mediaops_home_client::HomeApi;

use super::beat::{LIST_GENERATION, wait_connect};
use super::errors::FixtureError;
use super::seed::{HOLD_APPROVED_NAME, HOLD_OPEN_A, HOLD_OPEN_B, HOLD_REJECTED_NAME, QA_NOISE};
use super::seed_media::MovieFile;

pub(super) struct HoldSeed<'a> {
    pub name: &'a str,
    pub title_id: &'a str,
    pub release: &'a str,
    pub placement: Option<Placement>,
    pub remote_root: &'a str,
    pub remote_path: &'a str,
    pub reason: &'a str,
}

pub(super) async fn seed_holds(
    cli: &HomeApi,
    socket: &Path,
    now: i64,
    approved_file: &MovieFile,
) -> Result<(), FixtureError> {
    let inv = wait_connect(socket, Actor::Inventory).await?;
    apply_hold(
        &inv,
        now,
        HoldSeed {
            name: HOLD_OPEN_A,
            title_id: "movie:tmdb:4539",
            release: "watchable",
            placement: Some(Placement::movie("Hearts", 1991, "mkv")),
            remote_root: "",
            remote_path: "",
            reason: QA_NOISE,
        },
    )
    .await?;
    apply_hold(
        &inv,
        now,
        HoldSeed {
            name: HOLD_OPEN_B,
            title_id: "movie:tmdb:4539",
            release: "other",
            placement: Some(Placement::movie("Hearts", 1991, "mkv")),
            remote_root: "",
            remote_path: "",
            reason: "Manual Import required.",
        },
    )
    .await?;
    let mut approved = apply_hold(
        &inv,
        now,
        HoldSeed {
            name: HOLD_APPROVED_NAME,
            title_id: approved_file.title_id,
            release: "cam",
            placement: Some(approved_file.placement.clone()),
            remote_root: approved_file.remote_root,
            remote_path: &approved_file.remote_path,
            reason: "cam",
        },
    )
    .await?;
    let mut rejected = apply_hold(
        &inv,
        now,
        HoldSeed {
            name: HOLD_REJECTED_NAME,
            title_id: "movie:tmdb:13",
            release: "web",
            placement: Some(Placement::movie("New.York", 1970, "mkv")),
            remote_root: "",
            remote_path: "",
            reason: "rejected sample",
        },
    )
    .await?;
    if let Spec::Hold(spec) = &mut approved.spec {
        spec.decision = HoldDecisionSpec::Approved;
    }
    if let Spec::Hold(spec) = &mut rejected.spec {
        spec.decision = HoldDecisionSpec::Rejected;
    }
    cli.patch(approved, "spec").await?;
    cli.patch(rejected, "spec").await?;
    Ok(())
}

async fn apply_hold(
    inv: &HomeApi,
    now: i64,
    seed: HoldSeed<'_>,
) -> Result<HomeObject, FixtureError> {
    Ok(inv
        .apply(HomeObject::new(
            Kind::Hold,
            seed.name,
            Spec::Hold(HoldSpec {
                title_id: seed.title_id.into(),
                release_id: seed.release.into(),
                decision: HoldDecisionSpec::Empty,
            }),
            StatusBody::Hold(HoldStatus {
                list_generation: LIST_GENERATION,
                size: 7_100_000_000,
                reason: seed.reason.into(),
                release: seed.release.into(),
                added_unix: now.saturating_sub(75 * 60),
                placement: seed.placement,
                remote_root: seed.remote_root.into(),
                remote_path: seed.remote_path.into(),
                ..HoldStatus::default()
            }),
        ))
        .await?)
}
