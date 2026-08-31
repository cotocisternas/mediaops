use std::path::PathBuf;

use mediaops_core::{
    DesiredState, Envelope, Placement, Probe, RemoteRef, TitleId, TitleKind,
    VerifiedStagingHandle, install,
};
use mediaops_store::Store;
use mediaops_sync::refuse_below_watermark;
use mediaops_transfer::{
    PullSpec, configure_pool, connect_home, grpc_source, list_entries, pool_status, probe_range,
    pull_file, stat_entry,
};
use serde::Serialize;

use crate::bootstrap;
use crate::AppError;

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
    job_id: i64,
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
        let mut out = String::new();
        for e in &entries {
            out.push_str(&format!(
                "{} {}\t{}\n",
                e.r#ref().root_id(),
                e.r#ref().rel_path().display(),
                e.len()
            ));
        }
        if out.is_empty() {
            out.push_str("(empty listing)\n");
        }
        Ok(out.trim_end().to_string())
    }
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
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let lock_path = state_db
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("mediaops.lock");
    let _lock = bootstrap::exclusive_lock(&lock_path).map_err(map_bootstrap)?;
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
    let ds_text = std::fs::read_to_string(&desired_state)
        .map_err(|err| AppError::Runtime(err.into()))?;
    let ds = DesiredState::from_toml(&ds_text).map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    if ds.lock() {
        return Err(AppError::Policy(
            "desired-state lock is set; pull is frozen".into(),
        ));
    }
    let title_id = TitleId::parse(&title_id).map_err(|err| AppError::Usage(err.to_string()))?;
    let remote = RemoteRef::from_wire_parts(root, path)
        .map_err(|err| AppError::Usage(err.to_string()))?;
    let placement = if do_install {
        Some(placement_for(&title_id, &name, title, year, season, episode)?)
    } else {
        None
    };

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
    let outcome = pull_file(grpc_source(channel), &spec)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;

    let job = store
        .create_job(mediaops_core::JobKind::Pull, &title_id, None)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    store
        .advance(job.id(), mediaops_core::JobEvent::Pull(mediaops_core::PullEvent::Start))
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
    let mut whole_file_b3 = outcome.whole_file_b3.clone();
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
            .record_install(&title_id, &placed.whole_file_b3)
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
        job_id: job.id().get(),
        resumed_ranges: outcome
            .resumed_ranges
            .into_iter()
            .map(|(offset, len)| ResumedRange { offset, len })
            .collect(),
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        let mut line = format!("staged {} b3 {}", data.staged, data.whole_file_b3);
        if let Some(path) = data.installed {
            line.push_str(&format!(" installed {path}"));
        }
        if !data.resumed_ranges.is_empty() {
            line.push_str(&format!(" resumed {}", data.resumed_ranges.len()));
        }
        Ok(line)
    }
}

fn placement_for(
    title_id: &TitleId,
    name: &str,
    title: Option<String>,
    year: Option<u16>,
    season: Option<u8>,
    episode: Option<u8>,
) -> Result<Placement, AppError> {
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_string())
        .ok_or_else(|| AppError::Usage("--name must have an extension for --install".into()))?;
    let title = title.ok_or_else(|| AppError::Usage("--install requires --title".into()))?;
    let year = year.ok_or_else(|| AppError::Usage("--install requires --year".into()))?;
    match title_id.kind() {
        TitleKind::Movie => Ok(Placement::movie(title, year, ext)),
        TitleKind::Series => Ok(Placement::episode(
            title,
            year,
            season.ok_or_else(|| AppError::Usage("--install of a series requires --season".into()))?,
            episode
                .ok_or_else(|| AppError::Usage("--install of a series requires --episode".into()))?,
            ext,
        )),
        TitleKind::Album => Err(AppError::Usage(
            "--install for albums needs Epic 4; omit --install to keep the staged file".into(),
        )),
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
