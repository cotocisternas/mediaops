use std::path::PathBuf;

use mediaops_core::{
    DesiredState, Envelope, Placement, Probe, RemoteRef, TitleId, TitleKind, VerifiedStagingHandle,
    install, parse_placement,
};
use mediaops_store::Store;
use mediaops_sync::refuse_below_watermark;
use mediaops_transfer::{
    PullSpec, configure_pool, connect_home, grpc_source, list_entries, pool_status, probe_range,
    pull_file, stat_entry,
};
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;

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
    let outcome = pull_file(grpc_source(channel), &spec)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;

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
        assert_eq!(human, "(empty listing)");
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
        assert!(human.contains("seedbox a.bin"), "{human}");
        assert!(human.contains("10"), "{human}");
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
