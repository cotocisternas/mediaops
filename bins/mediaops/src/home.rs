use std::path::PathBuf;

use mediaops_core::{
    DesiredState, Envelope, Placement, Probe, RemoteRef, TitleId, TitleKind, VerifiedStagingHandle,
    install, parse_placement,
};
use mediaops_store::Store;
use mediaops_sync::refuse_below_watermark;
use mediaops_transfer::{
    PullSpec, configure_pool, connect_home, grpc_source, list_entries, pool_status, probe_range,
    pull_file_with_progress, stat_entry,
};
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;
use crate::out::{PullMeter, Style, Tone, finish, fmt_bytes, human_from_path, indent, row};

#[derive(Debug, Serialize)]
struct ListEntry {
    root_id: String,
    rel_path: String,
    len: u64,
    mtime: i64,
    nlink: u64,
}

#[derive(Debug, Serialize)]
struct ListData {
    entries: Vec<ListEntry>,
}

#[derive(Debug, Serialize)]
struct PullData {
    staged: String,
    whole_file_b3: String,
    installed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_id: Option<i64>,
    resumed_ranges: Vec<ResumedRange>,
}

#[derive(Debug, Serialize)]
struct ResumedRange {
    offset: u64,
    len: u64,
}

pub async fn list(
    json: bool,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let channel = connect_home(&socket, &tls_dir)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    let entries = list_entries(channel)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    if json {
        let data = ListData {
            entries: entries
                .iter()
                .map(|e| ListEntry {
                    root_id: e.r#ref().root_id().to_string(),
                    rel_path: e.r#ref().rel_path().display().to_string(),
                    len: e.len(),
                    mtime: e.mtime(),
                    nlink: e.nlink(),
                })
                .collect(),
        };
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format_list(&entries))
    }
}

fn format_list(entries: &[mediaops_core::RemoteEntry]) -> String {
    if entries.is_empty() {
        return "nothing on the box".into();
    }
    let style = Style::stdout();
    let mut groups: Vec<(String, Vec<&mediaops_core::RemoteEntry>)> = Vec::new();
    for entry in entries {
        let root = entry.r#ref().root_id().to_string();
        if let Some((_, list)) = groups.iter_mut().find(|(r, _)| *r == root) {
            list.push(entry);
        } else {
            groups.push((root, vec![entry]));
        }
    }
    let mut lines = Vec::new();
    for (i, (root, files)) in groups.iter().enumerate() {
        if i > 0 {
            lines.push(String::new());
        }
        lines.push(style.bold(root));
        for entry in files {
            lines.push(format!(
                "  {:>8}  {}",
                fmt_bytes(entry.len()),
                entry.r#ref().rel_path().display()
            ));
        }
    }
    finish(lines)
}

#[allow(clippy::too_many_arguments)]
pub async fn pull(
    json: bool,
    root: String,
    path: PathBuf,
    title_id: String,
    name: String,
    library_root: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    state_db: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    do_install: bool,
    title: Option<String>,
    year: Option<u16>,
    season: Option<u8>,
    episode: Option<u8>,
) -> Result<String, AppError> {
    let use_home = crate::api_legacy::use_home(&state_db);
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let state_db = crate::api_legacy::state_db_path(state_db);
    let lock_path = state_db
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("mediaops.lock");
    let _lock = bootstrap::exclusive_lock(&lock_path).map_err(map_bootstrap)?;
    if use_home {
        let home = crate::api_legacy::HomeLibrary::load().await?;
        let id = TitleId::parse(&title_id).map_err(|err| AppError::Usage(err.to_string()))?;
        let placement = if do_install {
            Some(placement_for(
                &id, &path, &name, title, year, season, episode,
            )?)
        } else {
            None
        };
        let remote = RemoteRef::from_wire_parts(root, path)
            .map_err(|err| AppError::Usage(err.to_string()))?;
        return pull_home(
            json,
            home,
            ManualPull {
                title_id: id,
                remote,
                name,
                placement,
                library_root,
                socket,
                tls_dir,
            },
        )
        .await;
    }
    let desired_state =
        desired_state.unwrap_or_else(|| bootstrap::default_desired_state(&config_dir));
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    let library_root = match library_root {
        Some(p) => p,
        None => store
            .get_machine("library_root")
            .await
            .map_err(|err| AppError::Runtime(anyhow_err(err)))?
            .map(PathBuf::from)
            .ok_or_else(|| {
                AppError::Usage("pass --library-root or run mediaops library bootstrap".into())
            })?,
    };
    let library_root = if library_root.exists() {
        std::fs::canonicalize(&library_root).unwrap_or(library_root)
    } else {
        library_root
    };
    let ds_text =
        std::fs::read_to_string(&desired_state).map_err(|err| AppError::Runtime(err.into()))?;
    let ds = DesiredState::from_toml(&ds_text).map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    if ds.lock() {
        return Err(AppError::Policy(
            "config lock is set; pull is frozen".into(),
        ));
    }
    let title_id = TitleId::parse(&title_id).map_err(|err| AppError::Usage(err.to_string()))?;
    let placement = if do_install {
        Some(placement_for(
            &title_id, &path, &name, title, year, season, episode,
        )?)
    } else {
        None
    };
    let remote =
        RemoteRef::from_wire_parts(root, path).map_err(|err| AppError::Usage(err.to_string()))?;

    let channel = connect_home(&socket, &tls_dir)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    let (fingerprint, _) = pool_status(channel.clone())
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    let n = match store
        .get_probe(&fingerprint)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?
    {
        Some(probe) => probe.range_concurrency,
        None => {
            let n = probe_range(channel.clone(), 32)
                .await
                .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
            store
                .put_probe(&Probe {
                    endpoint_fingerprint: fingerprint.clone(),
                    range_concurrency: n,
                })
                .await
                .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
            n
        }
    };
    configure_pool(channel.clone(), n)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    let entry = stat_entry(channel.clone(), &remote)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    if entry.len() > ds.max_copy().get() {
        return Err(AppError::Policy(format!(
            "file len {} exceeds max_copy {}",
            entry.len(),
            ds.max_copy().get()
        )));
    }
    let watermark_path = if library_root.exists() {
        library_root.clone()
    } else {
        library_root
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| library_root.clone())
    };
    let free = refuse_below_watermark(&watermark_path, ds.min_free()).map_err(|err| match err {
        mediaops_sync::LibraryError::Watermark { .. } => AppError::Policy(err.to_string()),
        other => AppError::Runtime(anyhow_err(other)),
    })?;
    if free.saturating_sub(entry.len()) < ds.min_free().get() {
        return Err(AppError::Policy(format!(
            "copy of {} bytes would breach min_free {}",
            entry.len(),
            ds.min_free().get()
        )));
    }

    let spec = PullSpec {
        library_root: library_root.clone(),
        title_id: title_id.clone(),
        final_name: name.clone(),
        remote,
        file_len: entry.len(),
        range_len: ds.range_len().get(),
        concurrency: n as usize,
    };
    let pull_label = placement
        .as_ref()
        .map(crate::out::human_placement)
        .unwrap_or_else(|| name.clone());
    let mut meter = (!json).then(|| PullMeter::new(pull_label.clone()));
    let outcome = pull_file_with_progress(grpc_source(channel), &spec, |done, total| {
        if let Some(m) = meter.as_mut() {
            m.update(done, total);
        }
    })
    .await
    .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    if let Some(m) = meter.as_mut() {
        m.finish();
    }

    let job = store
        .create_job(mediaops_core::JobKind::Pull, &title_id, None)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    store
        .advance(
            job.id(),
            mediaops_core::JobEvent::Pull(mediaops_core::PullEvent::Start),
        )
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    store
        .advance(
            job.id(),
            mediaops_core::JobEvent::Pull(mediaops_core::PullEvent::FinishRanges),
        )
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;

    let mut installed = None;
    let mut whole_file_b3 = {
        let file = std::fs::File::open(&outcome.staged)
            .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
        mediaops_core::Blake3Hex::of_reader(file)
            .map_err(|err| AppError::Runtime(anyhow_err(err)))?
    };
    if do_install {
        let placement = placement.expect("validated before pull");
        let handle = VerifiedStagingHandle::verify(
            &library_root,
            &title_id,
            outcome.staged.clone(),
            &placement,
        )
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
        let placed = install(&library_root, &title_id, &handle)
            .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
        whole_file_b3 = placed.whole_file_b3.clone();
        store
            .record_install(
                &title_id,
                &placed.whole_file_b3,
                handle.dest_rel().to_str().unwrap_or(""),
            )
            .await
            .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
        store
            .advance(
                job.id(),
                mediaops_core::JobEvent::Pull(mediaops_core::PullEvent::Install),
            )
            .await
            .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
        installed = Some(placed.path.display().to_string());
    }

    let data = PullData {
        staged: outcome.staged.display().to_string(),
        whole_file_b3: whole_file_b3.to_string(),
        installed,
        job_id: Some(job.id().get()),
        resumed_ranges: outcome
            .resumed_ranges
            .into_iter()
            .map(|(offset, len)| ResumedRange { offset, len })
            .collect(),
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format_pull(&data, &pull_label))
    }
}

struct ManualPull {
    title_id: TitleId,
    remote: RemoteRef,
    name: String,
    placement: Option<Placement>,
    library_root: Option<PathBuf>,
    socket: PathBuf,
    tls_dir: PathBuf,
}

async fn pull_home(
    json: bool,
    mut home: crate::api_legacy::HomeLibrary,
    request: ManualPull,
) -> Result<String, AppError> {
    let root = home.root(request.library_root)?;
    let cluster = home.spec()?.clone();
    if cluster.lock {
        return Err(AppError::Policy(
            "Cluster lock is set; pull is frozen".into(),
        ));
    }
    if let Some(placement) = &request.placement {
        let destination = root.join(
            mediaops_core::render(&request.title_id, placement)
                .map_err(crate::api_legacy::error)?,
        );
        match std::fs::symlink_metadata(&destination) {
            Ok(_) => {
                return Err(AppError::Policy(format!(
                    "destination already exists: {}; use library reindex to recover a missing proof",
                    destination.display()
                )));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(crate::api_legacy::error(err)),
        }
    }
    let channel = connect_home(&request.socket, &request.tls_dir)
        .await
        .map_err(crate::api_legacy::error)?;
    let entry = stat_entry(channel.clone(), &request.remote)
        .await
        .map_err(crate::api_legacy::error)?;
    let label = request
        .placement
        .as_ref()
        .map(crate::out::human_placement)
        .unwrap_or_else(|| request.name.clone());
    let spec = PullSpec {
        library_root: root.clone(),
        title_id: request.title_id.clone(),
        final_name: request.name,
        remote: request.remote,
        file_len: entry.len(),
        range_len: cluster.range_len.get(),
        concurrency: cluster.range_concurrency.unwrap_or(1) as usize,
    };
    check_manual_budget(&spec, &cluster)?;
    let was_locked = home.begin_maintenance().await?;
    let staged = async {
        check_manual_budget(&spec, &cluster)?;
        configure_pool(channel.clone(), spec.concurrency as u32)
            .await
            .map_err(crate::api_legacy::error)?;
        let mut meter = (!json).then(|| PullMeter::new(label.clone()));
        let copied = pull_file_with_progress(grpc_source(channel), &spec, |done, total| {
            if let Some(meter) = meter.as_mut() {
                meter.update(done, total);
            }
        })
        .await
        .map_err(crate::api_legacy::error)?;
        if let Some(meter) = meter.as_mut() {
            meter.finish();
        }
        let digest = mediaops_core::Blake3Hex::of_reader(
            std::fs::File::open(&copied.staged).map_err(crate::api_legacy::error)?,
        )
        .map_err(crate::api_legacy::error)?;
        Ok::<_, AppError>((copied, digest))
    }
    .await;
    let (copied, digest) = match staged {
        Ok(copied) => copied,
        Err(err) => return Err(crate::api_legacy::maintenance_failure(err)),
    };
    let installed = if let Some(placement) = request.placement {
        let handle = match VerifiedStagingHandle::verify(
            &root,
            &request.title_id,
            copied.staged.clone(),
            &placement,
        ) {
            Ok(handle) => handle,
            Err(err) => {
                return Err(crate::api_legacy::maintenance_failure(
                    crate::api_legacy::error(err),
                ));
            }
        };
        if let Err(err) = check_manual_install(&spec, &cluster, handle.dest_rel()) {
            home.finish_maintenance(was_locked).await?;
            return Err(err);
        }
        let placed = match install(&root, &request.title_id, &handle) {
            Ok(placed) => placed,
            Err(err) => {
                return Err(crate::api_legacy::maintenance_failure(
                    crate::api_legacy::error(err),
                ));
            }
        };
        if let Err(err) = home
            .publish_rows(
                &[mediaops_core::TitleIndexEntry::new(
                    request.title_id,
                    handle.dest_rel().display().to_string(),
                    placed.whole_file_b3.clone(),
                    placed.whole_file_b3,
                )],
                false,
            )
            .await
        {
            return Err(crate::api_legacy::maintenance_failure(
                crate::api_legacy::error(format!(
                    "installed file retained at {}; Home proof publication failed: {err}; run library reindex to recover its proof",
                    placed.path.display(),
                )),
            ));
        }
        Some(placed.path.display().to_string())
    } else {
        None
    };
    home.finish_maintenance(was_locked).await?;
    let data = PullData {
        staged: copied.staged.display().to_string(),
        whole_file_b3: digest.to_string(),
        installed,
        job_id: None,
        resumed_ranges: copied
            .resumed_ranges
            .into_iter()
            .map(|(offset, len)| ResumedRange { offset, len })
            .collect(),
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(crate::api_legacy::error)
    } else {
        Ok(format_pull(&data, &label))
    }
}

fn manual_install_job(
    library_root: &std::path::Path,
    title_id: &TitleId,
    dest_rel: &std::path::Path,
    file_len: u64,
    min_free: u64,
) -> mediaops_core::JobSpec {
    mediaops_core::JobSpec {
        library_root: library_root.display().to_string(),
        title_id: title_id.render(),
        dest_rel: dest_rel.display().to_string(),
        file_len,
        min_free,
        ..mediaops_core::JobSpec::default()
    }
}

fn check_manual_install(
    spec: &PullSpec,
    cluster: &mediaops_core::ClusterSpec,
    dest_rel: &std::path::Path,
) -> Result<(), AppError> {
    let job = manual_install_job(
        &spec.library_root,
        &spec.title_id,
        dest_rel,
        spec.file_len,
        cluster.min_free.get(),
    );
    if !mediaops_core::install_fits(&job).map_err(crate::api_legacy::error)? {
        return Err(AppError::Policy(
            "manual install would exceed destination filesystem free-space reserve".into(),
        ));
    }
    check_manual_budget(spec, cluster)
}

fn check_manual_budget(
    spec: &PullSpec,
    cluster: &mediaops_core::ClusterSpec,
) -> Result<(), AppError> {
    let free = mediaops_core::free_bytes(&spec.library_root).map_err(crate::api_legacy::error)?;
    let remaining = remaining_staging_bytes(spec)?;
    if (cluster.max_copy.get() > 0 && spec.file_len > cluster.max_copy.get())
        || !mediaops_core::pull_fits(free, cluster.min_free.get(), 0, 0, remaining)
    {
        return Err(AppError::Policy(
            "manual pull would exceed Cluster copy budget or free-space reserve".into(),
        ));
    }
    Ok(())
}

fn remaining_staging_bytes(spec: &PullSpec) -> Result<u64, AppError> {
    use std::os::unix::fs::MetadataExt;
    let staged = spec.library_root.join(
        mediaops_core::staging_path(&spec.title_id, &spec.final_name)
            .map_err(crate::api_legacy::error)?,
    );
    let mut partial = staged.clone();
    partial.as_mut_os_string().push(".partial");
    let allocated =
        [staged, partial].iter().try_fold(
            0u64,
            |largest, path| match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.is_file() => Ok(if meta.len() == spec.file_len {
                    largest.max(meta.blocks().saturating_mul(512))
                } else {
                    largest
                }),
                Ok(_) => Err(AppError::Policy(format!(
                    "staging path is not a regular file: {}",
                    path.display()
                ))),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(largest),
                Err(err) => Err(crate::api_legacy::error(err)),
            },
        )?;
    Ok(spec.file_len.saturating_sub(allocated))
}

fn format_pull(data: &PullData, label: &str) -> String {
    let style = Style::stdout();
    let dest = data.installed.as_deref().unwrap_or(data.staged.as_str());
    let title = human_from_path(dest).unwrap_or_else(|| label.to_string());
    let mut lines = vec![row(style, "pulled", Tone::Go, &title, "")];
    if let Some(path) = &data.installed {
        lines.push(indent(style, path));
    } else {
        lines.push(indent(style, "staged, not installed"));
    }
    if !data.resumed_ranges.is_empty() {
        lines.push(indent(
            style,
            &format!("resumed {} ranges", data.resumed_ranges.len()),
        ));
    }
    finish(lines)
}

fn placement_for(
    title_id: &TitleId,
    remote_path: &std::path::Path,
    name: &str,
    title: Option<String>,
    year: Option<u16>,
    season: Option<u8>,
    episode: Option<u8>,
) -> Result<Placement, AppError> {
    // A schema-shaped remote path names its own placement. It must agree with
    // `--title-id`: exactly for a key id, by kind for an *arr authority id.
    if let Ok((parsed_id, placement)) = parse_placement(remote_path)
        .or_else(|_| mediaops_core::parse_remote(Some(title_id.kind()), remote_path))
    {
        let agrees = if title_id.is_key() {
            parsed_id == *title_id
        } else {
            parsed_id.kind() == title_id.kind()
        };
        if agrees {
            return Ok(placement);
        }
        return Err(AppError::Usage(
            "--path TitleId does not match --title-id".into(),
        ));
    }
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_string())
        .ok_or_else(|| AppError::Usage("--name must have an extension for --install".into()))?;
    match title_id.kind() {
        TitleKind::Album => Err(AppError::Usage(
            "album --install requires a schema-valid --path (parse_placement)".into(),
        )),
        TitleKind::Movie | TitleKind::Series => {
            let title =
                title.ok_or_else(|| AppError::Usage("--install requires --title".into()))?;
            let year = year.ok_or_else(|| AppError::Usage("--install requires --year".into()))?;
            match title_id.kind() {
                TitleKind::Movie => Ok(Placement::movie(title, year, ext)),
                TitleKind::Series => Ok(Placement::episode(
                    title,
                    year,
                    season.ok_or_else(|| {
                        AppError::Usage("--install of a series requires --season".into())
                    })?,
                    u16::from(episode.ok_or_else(|| {
                        AppError::Usage("--install of a series requires --episode".into())
                    })?),
                    ext,
                )),
                TitleKind::Album => unreachable!(),
            }
        }
    }
}

fn anyhow_err(err: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}

fn map_bootstrap(err: bootstrap::BootstrapError) -> AppError {
    match err.exit_code() {
        mediaops_core::ExitCode::Usage => AppError::Usage(err.to_string()),
        mediaops_core::ExitCode::PolicyRefusal => AppError::Policy(err.to_string()),
        mediaops_core::ExitCode::LockConflict => AppError::LockConflict(err.to_string()),
        _ => AppError::Runtime(anyhow_err(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manual_home_pull_publishes_proof_and_restores_scheduling() {
        let _serial = crate::test_support::serial_net();
        let (home, dir, server) = crate::api_legacy::test_home("manual-home-pull").await;
        let api = home.api.clone();
        let library_root = home.root(None).expect("root");
        let pair =
            crate::test_support::start_pair(Some(crate::test_support::MOVIE_REL), b"original")
                .await;
        let title = TitleId::movie_key("The.Matrix", 1999).expect("title");
        let json = pull_home(
            true,
            home,
            ManualPull {
                title_id: title.clone(),
                remote: RemoteRef::from_wire_parts(
                    "seedbox".into(),
                    PathBuf::from(crate::test_support::MOVIE_REL),
                )
                .expect("remote"),
                name: "The.Matrix.(1999).mkv".into(),
                placement: Some(Placement::movie("The.Matrix", 1999, "mkv")),
                library_root: None,
                socket: pair.sock.clone(),
                tls_dir: pair.tls_dir.clone(),
            },
        )
        .await
        .expect("manual pull");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        assert!(
            value["data"].get("job_id").is_none(),
            "manual Home pull must not invent a legacy Job"
        );
        let object = api
            .get(mediaops_core::Kind::Title, &title.render())
            .await
            .expect("title");
        let mediaops_core::StatusBody::Title(status) = object.status else {
            panic!("Title status");
        };
        let files = status.observed_files();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, crate::test_support::MOVIE_REL);
        assert_eq!(
            std::fs::read(library_root.join(&files[0].path)).expect("installed"),
            b"original"
        );
        let cluster = api
            .get(mediaops_core::Kind::Cluster, mediaops_core::CLUSTER_NAME)
            .await
            .expect("cluster");
        let mediaops_core::Spec::Cluster(spec) = cluster.spec else {
            panic!("Cluster spec");
        };
        assert!(!spec.lock);
        server.abort();
        let _ = std::fs::remove_dir_all(dir);
    }

    fn fs_info(path: &std::path::Path) -> Option<(u64, u64)> {
        use std::os::unix::fs::MetadataExt;
        Some((
            std::fs::metadata(path).ok()?.dev(),
            mediaops_core::free_bytes(path).ok()?,
        ))
    }

    fn dest_watermark_bases() -> Vec<std::path::PathBuf> {
        let mut bases = Vec::new();
        if let Some(path) = std::env::var_os("MEDIAOPS_TEST_INSTALL_FS") {
            bases.push(std::path::PathBuf::from(path));
        }
        for path in ["/tmp", "/dev/shm", "/var/tmp"] {
            bases.push(std::path::PathBuf::from(path));
        }
        bases.into_iter().filter(|path| path.is_dir()).collect()
    }

    struct DestFixture {
        extra: Vec<std::path::PathBuf>,
    }
    impl Drop for DestFixture {
        fn drop(&mut self) {
            for path in self.extra.iter().rev() {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }

    fn unique_dir(base: &std::path::Path, tag: &str) -> std::path::PathBuf {
        base.join(format!(
            "mediaops-home-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    async fn prepare_dest_watermark(
        home: &mut crate::api_legacy::HomeLibrary,
    ) -> Option<DestFixture> {
        let original = home.root(None).expect("root");
        let (orig_dev, orig_free) = fs_info(&original)?;
        let mut tighter = None;
        let mut roomier = None;
        for base in dest_watermark_bases() {
            let Some((dev, free)) = fs_info(&base) else {
                continue;
            };
            if dev != orig_dev && free < orig_free {
                tighter = Some(base);
                break;
            }
            if dev != orig_dev && free > orig_free {
                roomier = Some(base);
            }
        }
        let mut extra = Vec::new();
        let (library_root, dest_fs) = if let Some(dest_base) = tighter {
            let dest_fs = unique_dir(&dest_base, "dest");
            std::fs::create_dir_all(&dest_fs).expect("dest fs");
            extra.push(dest_fs.clone());
            (original, dest_fs)
        } else {
            let lib_base = roomier?;
            let library_root = unique_dir(&lib_base, "lib");
            mediaops_sync::ensure_layout(&library_root).expect("layout");
            extra.push(library_root.clone());
            let dest_fs = unique_dir(&original, "dest");
            std::fs::create_dir_all(&dest_fs).expect("dest fs");
            extra.push(dest_fs.clone());
            (library_root, dest_fs)
        };
        let dest_free = mediaops_core::free_bytes(&dest_fs).expect("dest free");
        let root_free = mediaops_core::free_bytes(&library_root).expect("root free");
        if root_free <= dest_free {
            eprintln!("skipping dest-watermark fixture: library root is not roomier than dest fs");
            return None;
        }
        let movies = library_root.join("movies");
        let _ = std::fs::remove_dir_all(&movies);
        std::os::unix::fs::symlink(&dest_fs, &movies).expect("movies symlink");
        let mut cluster = home.cluster.clone();
        let mediaops_core::Spec::Cluster(spec) = &mut cluster.spec else {
            panic!("Cluster");
        };
        spec.library_root = library_root.display().to_string();
        spec.min_free = mediaops_core::Bytes::new(dest_free);
        home.cluster = home.api.patch(cluster, "spec").await.expect("cluster");
        Some(DestFixture { extra })
    }

    #[tokio::test]
    async fn manual_install_dest_watermark_refuses_before_publication() {
        let _serial = crate::test_support::serial_net();
        let (mut home, dir, server) = crate::api_legacy::test_home("manual-dest-watermark").await;
        let api = home.api.clone();
        let Some(_dest) = prepare_dest_watermark(&mut home).await else {
            server.abort();
            let _ = std::fs::remove_dir_all(dir);
            return;
        };
        let library_root = home.root(None).expect("root");
        let pair =
            crate::test_support::start_pair(Some(crate::test_support::MOVIE_REL), b"original")
                .await;
        let title = TitleId::movie_key("The.Matrix", 1999).expect("title");
        let err = pull_home(
            true,
            home,
            ManualPull {
                title_id: title.clone(),
                remote: RemoteRef::from_wire_parts(
                    "seedbox".into(),
                    PathBuf::from(crate::test_support::MOVIE_REL),
                )
                .expect("remote"),
                name: "The.Matrix.(1999).mkv".into(),
                placement: Some(Placement::movie("The.Matrix", 1999, "mkv")),
                library_root: None,
                socket: pair.sock.clone(),
                tls_dir: pair.tls_dir.clone(),
            },
        )
        .await
        .expect_err("dest watermark");
        assert!(matches!(err, AppError::Policy(_)), "{err}");
        let staged = library_root
            .join(mediaops_core::staging_path(&title, "The.Matrix.(1999).mkv").expect("stage"));
        assert!(staged.is_file(), "staging retained at {}", staged.display());
        assert!(
            std::fs::symlink_metadata(library_root.join(crate::test_support::MOVIE_REL)).is_err(),
            "dest must stay absent"
        );
        let title_get = api.get(mediaops_core::Kind::Title, &title.render()).await;
        assert!(
            title_get.as_ref().is_err_and(|e| e.is_not_found()),
            "no Title proof: {title_get:?}"
        );
        let cluster = api
            .get(mediaops_core::Kind::Cluster, mediaops_core::CLUSTER_NAME)
            .await
            .expect("cluster");
        let mediaops_core::Spec::Cluster(spec) = cluster.spec else {
            panic!("Cluster spec");
        };
        assert!(!spec.lock, "maintenance lock restored");
        server.abort();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn manual_staging_only_pull_does_not_consult_dest_fs() {
        let _serial = crate::test_support::serial_net();
        let (mut home, dir, server) = crate::api_legacy::test_home("manual-stage-only-dest").await;
        let Some(_dest) = prepare_dest_watermark(&mut home).await else {
            server.abort();
            let _ = std::fs::remove_dir_all(dir);
            return;
        };
        let library_root = home.root(None).expect("root");
        let pair =
            crate::test_support::start_pair(Some(crate::test_support::MOVIE_REL), b"original")
                .await;
        let title = TitleId::movie_key("The.Matrix", 1999).expect("title");
        let json = pull_home(
            true,
            home,
            ManualPull {
                title_id: title.clone(),
                remote: RemoteRef::from_wire_parts(
                    "seedbox".into(),
                    PathBuf::from(crate::test_support::MOVIE_REL),
                )
                .expect("remote"),
                name: "The.Matrix.(1999).mkv".into(),
                placement: None,
                library_root: None,
                socket: pair.sock.clone(),
                tls_dir: pair.tls_dir.clone(),
            },
        )
        .await
        .expect("staging-only");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        assert!(value["data"]["installed"].is_null());
        let staged = library_root
            .join(mediaops_core::staging_path(&title, "The.Matrix.(1999).mkv").expect("stage"));
        assert!(staged.is_file(), "{}", staged.display());
        assert!(
            std::fs::symlink_metadata(library_root.join(crate::test_support::MOVIE_REL)).is_err()
        );
        server.abort();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manual_install_job_spec_uses_dest_rel_and_cluster_min_free() {
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let dest_rel = std::path::Path::new(crate::test_support::MOVIE_REL);
        let job = manual_install_job(std::path::Path::new("/library"), &title, dest_rel, 10, 42);
        assert_eq!(job.library_root, "/library");
        assert_eq!(job.title_id, title.render());
        assert_eq!(job.dest_rel, dest_rel.display().to_string());
        assert_eq!(job.file_len, 10);
        assert_eq!(job.min_free, 42);
    }

    #[test]
    fn same_filesystem_install_fits_even_when_min_free_exceeds_capacity() {
        let dir = crate::test_support::scratch("same-fs-fits");
        let library = crate::test_support::library_root(&dir);
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let job = manual_install_job(
            &library,
            &title,
            std::path::Path::new(crate::test_support::MOVIE_REL),
            10,
            u64::MAX,
        );
        assert!(mediaops_core::install_fits(&job).expect("same-fs dest is root-covered"));
        let _ = std::fs::remove_dir_all(dir);
    }

    use std::path::Path;

    #[test]
    fn album_install_uses_parse_placement_without_title_year() {
        let title = TitleId::album("0f82b02e-c6cd-4242-b195-93d4bf3e0d63").expect("album");
        let path =
            Path::new("music/Yes/Relayer.(2013)/Relayer.(2013).01.The.Gates.Of.Delirium.flac");
        let placement = placement_for(
            &title,
            path,
            "Relayer.(2013).01.The.Gates.Of.Delirium.flac",
            None,
            None,
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            placement,
            Placement::track(
                "Yes",
                "Relayer",
                2013,
                None,
                Some(1),
                "The.Gates.Of.Delirium",
                "flac"
            )
        );
    }

    #[test]
    fn parse_placement_title_mismatch_is_usage() {
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let path = Path::new("movies/Other.(2000)/Other.(2000).mkv");
        let err = placement_for(
            &title,
            path,
            "Other.(2000).mkv",
            Some("Other".into()),
            Some(2000),
            None,
            None,
        );
        assert!(
            matches!(err, Err(AppError::Usage(_))),
            "mismatched TitleId must be usage, not a silent --title/--year fallback"
        );
    }

    #[tokio::test]
    async fn list_json_and_human_empty_and_one_file() {
        let _serial = crate::test_support::serial_net();
        let empty = crate::test_support::start_pair(None, b"").await;
        let human = list(
            false,
            Some(empty.sock.clone()),
            Some(empty.tls_dir.clone()),
            None,
        )
        .await
        .expect("empty human");
        assert_eq!(human, "nothing on the box");
        drop(empty);

        let lb = crate::test_support::start_pair(Some("a.bin"), b"abcdefghij").await;
        let json = list(true, Some(lb.sock.clone()), Some(lb.tls_dir.clone()), None)
            .await
            .expect("list json");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["entries"].as_array().expect("arr").len(), 1);
        assert_eq!(value["data"]["entries"][0]["rel_path"], "a.bin");
        assert_eq!(value["data"]["entries"][0]["len"], 10);
        let human = list(false, Some(lb.sock.clone()), Some(lb.tls_dir.clone()), None)
            .await
            .expect("list human");
        assert_eq!(
            human,
            "\
seedbox
      10 B  a.bin"
        );
    }

    #[tokio::test]
    async fn pull_stages_without_install_and_records_job() {
        let _serial = crate::test_support::serial_net();
        let lb = crate::test_support::start_pair(Some("a.bin"), b"abcdefghij").await;
        let dir = crate::test_support::scratch("pull-stage");
        let library = crate::test_support::library_root(&dir);
        let store = crate::test_support::open_store(&dir).await;
        crate::test_support::seed_probe(&store, &lb.fingerprint).await;
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let json = pull(
            true,
            "seedbox".into(),
            PathBuf::from("a.bin"),
            "movie:key:thematrix.1999".into(),
            "The.Matrix.(1999).mkv".into(),
            Some(library.clone()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            None,
            Some(dir.join("state.db")),
            Some(ds),
            false,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("pull");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        assert!(value["data"]["installed"].is_null());
        let staged = value["data"]["staged"].as_str().expect("staged");
        assert!(staged.contains("_incoming"), "{staged}");
        assert!(std::path::Path::new(staged).is_file());
        let jobs = store.list_jobs().await.expect("jobs");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind(), mediaops_core::JobKind::Pull);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn pull_install_uses_schema_path_parse_placement() {
        let _serial = crate::test_support::serial_net();
        let lb =
            crate::test_support::start_pair(Some(crate::test_support::MOVIE_REL), b"abcdefghij")
                .await;
        let dir = crate::test_support::scratch("pull-install");
        let library = crate::test_support::library_root(&dir);
        let store = crate::test_support::open_store(&dir).await;
        crate::test_support::seed_probe(&store, &lb.fingerprint).await;
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let json = pull(
            true,
            "seedbox".into(),
            PathBuf::from(crate::test_support::MOVIE_REL),
            "movie:key:thematrix.1999".into(),
            "The.Matrix.(1999).mkv".into(),
            Some(library.clone()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            None,
            Some(dir.join("state.db")),
            Some(ds),
            true,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("install");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        let installed = value["data"]["installed"].as_str().expect("installed");
        assert!(installed.contains("The.Matrix.(1999).mkv"), "{installed}");
        assert!(std::path::Path::new(installed).is_file());
        let title = store
            .get_title(&TitleId::movie_key("The.Matrix", 1999).expect("id"))
            .await
            .expect("title")
            .into_iter()
            .next()
            .expect("indexed");
        assert!(!title.path_missing());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn pull_lock_true_is_policy_refusal() {
        let dir = crate::test_support::scratch("pull-lock");
        let library = crate::test_support::library_root(&dir);
        let _store = crate::test_support::open_store(&dir).await;
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_LOCKED);
        let err = pull(
            true,
            "seedbox".into(),
            PathBuf::from("a.bin"),
            "movie:key:thematrix.1999".into(),
            "The.Matrix.(1999).mkv".into(),
            Some(library),
            Some(dir.join("missing.sock")),
            Some(dir.join("tls")),
            None,
            Some(dir.join("state.db")),
            Some(ds),
            false,
            None,
            None,
            None,
            None,
        )
        .await
        .expect_err("lock");
        assert!(
            matches!(err, AppError::Policy(ref m) if m.contains("lock")),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn pull_over_max_copy_is_policy() {
        let _serial = crate::test_support::serial_net();
        let lb = crate::test_support::start_pair(Some("a.bin"), b"abcdefghij").await;
        let dir = crate::test_support::scratch("pull-max");
        let library = crate::test_support::library_root(&dir);
        let store = crate::test_support::open_store(&dir).await;
        crate::test_support::seed_probe(&store, &lb.fingerprint).await;
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_MAX_COPY_ZERO);
        let err = pull(
            true,
            "seedbox".into(),
            PathBuf::from("a.bin"),
            "movie:key:thematrix.1999".into(),
            "The.Matrix.(1999).mkv".into(),
            Some(library),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            None,
            Some(dir.join("state.db")),
            Some(ds),
            false,
            None,
            None,
            None,
            None,
        )
        .await
        .expect_err("max_copy");
        assert!(
            matches!(err, AppError::Policy(ref m) if m.contains("max_copy")),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn pull_without_library_root_is_usage() {
        let dir = crate::test_support::scratch("pull-usage");
        let _store = crate::test_support::open_store(&dir).await;
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let err = pull(
            true,
            "seedbox".into(),
            PathBuf::from("a.bin"),
            "movie:key:thematrix.1999".into(),
            "The.Matrix.(1999).mkv".into(),
            None,
            Some(dir.join("missing.sock")),
            Some(dir.join("tls")),
            None,
            Some(dir.join("state.db")),
            Some(ds),
            false,
            None,
            None,
            None,
            None,
        )
        .await
        .expect_err("usage");
        assert!(matches!(err, AppError::Usage(_)), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
