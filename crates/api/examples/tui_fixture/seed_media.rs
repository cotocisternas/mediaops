use std::path::Path;

use mediaops_core::{
    Actor, HomeObject, Kind, Placement, RemoteFileStatus, Spec, StatusBody, TitleId, TitleSpec,
    TitleStatus, WantSpec, WantStatus, remote_file_name, render,
};
use mediaops_home_client::HomeApi;

use super::beat::{LIST_GENERATION, wait_connect};
use super::errors::FixtureError;

pub(super) struct MovieFile {
    pub title_id: &'static str,
    pub dest_rel: String,
    pub remote_root: &'static str,
    pub remote_path: String,
    pub placement: Placement,
}

pub(super) fn movie_file(
    title_id: &'static str,
    title: &str,
    year: u16,
) -> Result<MovieFile, FixtureError> {
    let id = TitleId::parse(title_id).map_err(|err| FixtureError::Invalid(err.to_string()))?;
    let placement = Placement::movie(title, year, "mkv");
    let dest = render(&id, &placement).map_err(|err| FixtureError::Invalid(err.to_string()))?;
    let dest_rel = dest.to_string_lossy().into_owned();
    let remote_path = dest_rel
        .strip_prefix("movies/")
        .ok_or_else(|| FixtureError::Invalid("movie dest_rel missing movies/".into()))?
        .to_owned();
    Ok(MovieFile {
        title_id,
        dest_rel,
        remote_root: "movies",
        remote_path,
        placement,
    })
}

pub(super) async fn apply_want(cli: &HomeApi, title_id: &str) -> Result<(), FixtureError> {
    cli.apply(HomeObject::new(
        Kind::Want,
        title_id,
        Spec::Want(WantSpec {
            title_id: title_id.into(),
        }),
        StatusBody::Want(WantStatus::default()),
    ))
    .await?;
    Ok(())
}

pub(super) async fn apply_title(cli: &HomeApi, title_id: &str) -> Result<(), FixtureError> {
    cli.apply(HomeObject::new(
        Kind::Title,
        title_id,
        Spec::Title(TitleSpec {
            title_id: title_id.into(),
            desired_present: true,
        }),
        StatusBody::Title(TitleStatus::default()),
    ))
    .await?;
    Ok(())
}

pub(super) async fn apply_remote(
    socket: &Path,
    file: &MovieFile,
    len: u64,
) -> Result<(), FixtureError> {
    let inv = wait_connect(socket, Actor::Inventory).await?;
    inv.apply(HomeObject::new(
        Kind::RemoteFile,
        remote_file_name(file.remote_root, &file.remote_path),
        Spec::RemoteFile,
        StatusBody::RemoteFile(RemoteFileStatus {
            root_id: file.remote_root.into(),
            rel_path: file.remote_path.clone(),
            len,
            parse_ok: true,
            title_id: file.title_id.into(),
            list_generation: LIST_GENERATION,
        }),
    ))
    .await?;
    Ok(())
}
