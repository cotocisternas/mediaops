use std::path::PathBuf;

use mediaops_core::{
    DesiredState, EncodeEvent, Envelope, Job, JobEvent, JobKind, JobState, TitleId, encode_ready,
    parse_placement,
};
use mediaops_encode::{
    EncodeDecision, TranscodeSpec, classify, encode_to_converting, probe_media, replace_converting,
    session_cap, should_start_next,
};
use mediaops_ssh::SystemExec;
use mediaops_store::Store;
use mediaops_sync::scan_schema_files;
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;

#[derive(Debug, Serialize)]
struct ScanFile {
    path: String,
    title_id: String,
    decision: String,
}

#[derive(Debug, Serialize)]
struct ScanData {
    files: Vec<ScanFile>,
}

#[derive(Debug, Serialize)]
struct PauseData {
    encode_pause: bool,
}

#[derive(Debug, Serialize)]
struct EncodeRunData {
    ran: usize,
    skipped: usize,
    paused: bool,
}

pub async fn scan(
    json: bool,
    library_root: Option<PathBuf>,
    state_db: Option<PathBuf>,
) -> Result<String, AppError> {
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let library_root = match library_root {
        Some(p) => p,
        None => store
            .get_machine("library_root")
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
            .map(PathBuf::from)
            .ok_or_else(|| AppError::Usage("pass --library-root or library bootstrap".into()))?,
    };
    let movies = library_root.join("movies");
    let mut files = Vec::new();
    if movies.is_dir() {
        for (title_id, rel, _) in scan_schema_files(&library_root).map_err(runtime_display)? {
            if title_id.kind() != mediaops_core::TitleKind::Movie {
                continue;
            }
            let path = library_root.join(&rel);
            let decision = match probe_media(&SystemExec, &path).await {
                Ok(media) => classify(title_id.kind(), &media),
                Err(_) => EncodeDecision::Keep,
            };
            files.push(ScanFile {
                path: rel.display().to_string(),
                title_id: title_id.render(),
                decision: decision.as_str().to_string(),
            });
        }
    }
    let data = ScanData { files };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!("encode scan {} files", data.files.len()))
    }
}

pub async fn pause(json: bool, off: bool, state_db: Option<PathBuf>) -> Result<String, AppError> {
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let value = if off { "0" } else { "1" };
    store
        .put_machine("encode_pause", value)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let data = PauseData { encode_pause: !off };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!(
            "encode pause {}",
            if data.encode_pause { "on" } else { "off" }
        ))
    }
}

pub async fn run(
    json: bool,
    title: Option<String>,
    state_db: Option<PathBuf>,
    library_root: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    config_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let lock_path = bootstrap::lock_path(&state_db);
    let _lock = bootstrap::exclusive_lock(&lock_path).map_err(map_bootstrap)?;
    let desired_state =
        desired_state.unwrap_or_else(|| bootstrap::default_desired_state(&config_dir));
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let library_root = match library_root {
        Some(p) => p,
        None => store
            .get_machine("library_root")
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
            .map(PathBuf::from)
            .ok_or_else(|| AppError::Usage("pass --library-root or library bootstrap".into()))?,
    };
    let ds_text =
        std::fs::read_to_string(&desired_state).map_err(|err| AppError::Runtime(err.into()))?;
    let ds = DesiredState::from_toml(&ds_text)
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let nvenc_cap = store
        .get_machine("nvenc_cap")
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let hevc = nvenc_cap > 0;
    let cap = session_cap(ds.max_nvenc(), nvenc_cap, hevc);
    let paused = store
        .get_machine("encode_pause")
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
        .as_deref()
        == Some("1");
    let ffmpeg = store
        .get_machine("ffmpeg_path")
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "ffmpeg".into());

    if let Some(raw) = title {
        let title_id = TitleId::parse(&raw).map_err(|err| AppError::Usage(err.to_string()))?;
        if cap == 0 {
            return Err(AppError::Policy("no NVENC capacity".into()));
        }
        let path = resolve_path(&store, &library_root, &title_id).await?;
        let media = probe_media(&SystemExec, &path)
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        match classify(title_id.kind(), &media) {
            EncodeDecision::Refuse => {
                return Err(AppError::Policy(
                    "encode refused by policy (HDR/DV/2160p)".into(),
                ));
            }
            EncodeDecision::Keep => {
                let data = EncodeRunData {
                    ran: 0,
                    skipped: 1,
                    paused,
                };
                return if json {
                    serde_json::to_string(&Envelope::ok(data))
                        .map_err(|e| AppError::Runtime(e.into()))
                } else {
                    Ok("encode keep".into())
                };
            }
            EncodeDecision::NvencH264 => {}
        }
        if paused {
            let data = EncodeRunData {
                ran: 0,
                skipped: 1,
                paused: true,
            };
            return if json {
                serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
            } else {
                Ok("encode paused".into())
            };
        }
        let (_, placement) = parse_placement(&path.strip_prefix(&library_root).unwrap_or(&path))
            .or_else(|_| parse_placement(&path))
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        let spec = TranscodeSpec {
            library_root: &library_root,
            title_id: &title_id,
            placement: &placement,
            ffmpeg: &ffmpeg,
        };
        let converting = encode_to_converting(&SystemExec, spec)
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        let (_dest, digest) = replace_converting(spec, converting)
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        store
            .record_replace(&title_id, &digest)
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        let data = EncodeRunData {
            ran: 1,
            skipped: 0,
            paused: false,
        };
        return if json {
            serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
        } else {
            Ok("encode ran 1".into())
        };
    }

    if cap == 0 {
        let data = EncodeRunData {
            ran: 0,
            skipped: 0,
            paused,
        };
        return if json {
            serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
        } else {
            Ok("encode cap 0".into())
        };
    }

    let jobs = store
        .list_jobs()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let mut ran = 0usize;
    let mut skipped = 0usize;
    for job in jobs {
        if !matches!(job.state(), JobState::Encode(_)) {
            continue;
        }
        if !should_start_next(paused, cap) {
            skipped += 1;
            continue;
        }
        let parent = match job.parent_job_id() {
            Some(id) => store
                .get_job(id)
                .await
                .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?,
            None => None,
        };
        let indexed = store
            .get_title(job.title_id())
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
            .is_some();
        let retry = matches!(
            job.state(),
            JobState::Encode(mediaops_core::EncodeState::Encoding)
                | JobState::Encode(mediaops_core::EncodeState::Replacing)
        );
        if !retry && !encode_ready(&job, parent.as_ref(), indexed) {
            continue;
        }
        match encode_one(&store, &library_root, &job, &ffmpeg).await {
            Ok(()) => ran += 1,
            Err(AppError::Policy(_)) => skipped += 1,
            Err(err) => return Err(err),
        }
    }
    let data = EncodeRunData {
        ran,
        skipped,
        paused,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!("encode ran {ran} skipped {skipped}"))
    }
}

pub async fn after_install(
    store: &Store,
    library_root: &std::path::Path,
    title_id: &TitleId,
    dest: &std::path::Path,
    pull: &Job,
    ffmpeg: &str,
    cap: u32,
    paused: bool,
) -> Result<(), AppError> {
    if paused || cap == 0 {
        return Ok(());
    }
    let media = match probe_media(&SystemExec, dest).await {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };
    match classify(title_id.kind(), &media) {
        EncodeDecision::NvencH264 => {}
        EncodeDecision::Keep | EncodeDecision::Refuse => return Ok(()),
    }
    let encode = store
        .create_job(JobKind::Encode, title_id, Some(pull.id()))
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    if !encode_ready(&encode, Some(pull), true) {
        return Ok(());
    }
    encode_one(store, library_root, &encode, ffmpeg).await
}

async fn encode_one(
    store: &Store,
    library_root: &std::path::Path,
    job: &Job,
    ffmpeg: &str,
) -> Result<(), AppError> {
    let path = resolve_path(store, library_root, job.title_id()).await?;
    let rel = path.strip_prefix(library_root).unwrap_or(path.as_path());
    let (_, placement) =
        parse_placement(rel).map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let spec = TranscodeSpec {
        library_root,
        title_id: job.title_id(),
        placement: &placement,
        ffmpeg,
    };
    let mut state = job.state();
    if matches!(state, JobState::Encode(mediaops_core::EncodeState::Queued)) {
        let started = store
            .advance(job.id(), JobEvent::Encode(EncodeEvent::Start))
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        state = started.state();
    }
    if matches!(
        state,
        JobState::Encode(mediaops_core::EncodeState::Encoding)
    ) {
        let converting = encode_to_converting(&SystemExec, spec)
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        let _ = converting;
        let next = store
            .advance(job.id(), JobEvent::Encode(EncodeEvent::FinishEncode))
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        state = next.state();
    }
    if matches!(
        state,
        JobState::Encode(mediaops_core::EncodeState::Replacing)
    ) {
        let filename = placement_filename(&placement)?;
        let converting = mediaops_encode::converting_path(library_root, job.title_id(), &filename)
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        let (_dest, digest) = replace_converting(spec, converting)
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        store
            .record_replace(job.title_id(), &digest)
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        store
            .advance(job.id(), JobEvent::Encode(EncodeEvent::Replace))
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    }
    Ok(())
}

fn placement_filename(placement: &mediaops_core::Placement) -> Result<String, AppError> {
    match placement {
        mediaops_core::Placement::Movie {
            title,
            year,
            extension,
        } => Ok(format!("{title}.({year}).{extension}")),
        mediaops_core::Placement::Episode {
            title,
            year,
            season,
            episode,
            extension,
        } => Ok(format!(
            "{title}.({year}).S{season:02}E{episode:02}.{extension}"
        )),
        mediaops_core::Placement::Track {
            track,
            title,
            year,
            extension,
            ..
        } => Ok(format!("{track:02}.{title}.({year}).{extension}")),
    }
}

async fn resolve_path(
    store: &Store,
    library_root: &std::path::Path,
    title_id: &TitleId,
) -> Result<PathBuf, AppError> {
    if let Some(entry) = store
        .get_title(title_id)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
    {
        if !entry.path_missing() {
            return Ok(library_root.join(entry.path()));
        }
    }
    let found = scan_schema_files(library_root)
        .map_err(runtime_display)?
        .into_iter()
        .find(|(id, _, _)| id == title_id)
        .map(|(_, rel, _)| library_root.join(rel));
    found.ok_or_else(|| AppError::Usage(format!("no library file for {}", title_id.render())))
}

fn map_bootstrap(err: bootstrap::BootstrapError) -> AppError {
    match err.exit_code() {
        mediaops_core::ExitCode::Usage => AppError::Usage(err.to_string()),
        mediaops_core::ExitCode::PolicyRefusal => AppError::Policy(err.to_string()),
        mediaops_core::ExitCode::LockConflict => AppError::LockConflict(err.to_string()),
        _ => AppError::Runtime(anyhow::anyhow!("{err}")),
    }
}

fn runtime_display(err: impl std::fmt::Display) -> AppError {
    AppError::Runtime(anyhow::anyhow!("{err}"))
}
