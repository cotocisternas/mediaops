use std::path::PathBuf;

use mediaops_core::{
    DesiredState, EncodeEvent, Envelope, ExecPort, Job, JobEvent, JobKind, JobState, TitleId,
    encode_ready, parse_placement,
};
use mediaops_encode::{
    EncodeDecision, TranscodeSpec, classify, encode_to_converting, probe_media, replace_converting,
    session_cap, should_start_next,
};
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
    exec: &impl ExecPort,
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
            let decision = match probe_media(exec, &path).await {
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
    exec: &impl ExecPort,
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
        let media = probe_media(exec, &path)
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
        let converting = encode_to_converting(exec, spec)
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
        match encode_one(exec, &store, &library_root, &job, &ffmpeg).await {
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
    exec: &impl ExecPort,
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
    let media = match probe_media(exec, dest).await {
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
    encode_one(exec, store, library_root, &encode, ffmpeg).await
}

async fn encode_one(
    exec: &impl ExecPort,
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
        let converting = encode_to_converting(exec, spec)
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

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{Blake3Hex, ExecCommand, ExecError, ExecOutput, Placement, render};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    const HEVC10: &str = r#"{
        "streams": [{
            "codec_type": "video",
            "codec_name": "hevc",
            "width": 1920,
            "height": 1080,
            "bits_per_raw_sample": "10",
            "pix_fmt": "yuv420p10le",
            "color_transfer": "bt709"
        }],
        "format": { "format_name": "mp4" }
    }"#;
    const H264: &str = r#"{
        "streams": [{
            "codec_type": "video",
            "codec_name": "h264",
            "width": 1920,
            "height": 1080,
            "bits_per_raw_sample": "8",
            "pix_fmt": "yuv420p",
            "color_transfer": "bt709"
        }],
        "format": { "format_name": "mp4" }
    }"#;
    const HDR: &str = r#"{
        "streams": [{
            "codec_type": "video",
            "codec_name": "hevc",
            "width": 1920,
            "height": 1080,
            "bits_per_raw_sample": "10",
            "pix_fmt": "yuv420p10le",
            "color_transfer": "smpte2084"
        }],
        "format": { "format_name": "mp4" }
    }"#;

    struct Transcript {
        stdout: String,
        write_converting: bool,
        calls: Mutex<Vec<String>>,
    }

    impl ExecPort for Transcript {
        async fn run(&self, command: &ExecCommand) -> Result<ExecOutput, ExecError> {
            self.calls
                .lock()
                .expect("calls")
                .push(command.program.clone());
            if command.program_name() == "ffmpeg" && self.write_converting {
                if let Some(out) = command.args.last() {
                    let path = PathBuf::from(out);
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).expect("mkdir");
                    }
                    std::fs::write(&path, b"encoded-h264").expect("converting");
                }
            }
            Ok(ExecOutput {
                status: 0,
                stdout: self.stdout.as_bytes().to_vec(),
                stderr: Vec::new(),
            })
        }
    }

    fn probe(json: &str) -> Transcript {
        Transcript {
            stdout: json.into(),
            write_converting: false,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn ffmpeg(json: &str) -> Transcript {
        Transcript {
            stdout: json.into(),
            write_converting: true,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn write_schema(root: &Path, rel: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, b"original-hevc").expect("write");
    }

    #[tokio::test]
    async fn pause_on_and_off_persists_and_json() {
        let dir = crate::test_support::scratch("encode-pause");
        let db = dir.join("state.db");
        let on = pause(true, false, Some(db.clone())).await.expect("on");
        let value: serde_json::Value = serde_json::from_str(&on).expect("json");
        assert_eq!(value["data"]["encode_pause"], true);
        let store = Store::open(&db).await.expect("store");
        assert_eq!(
            store
                .get_machine("encode_pause")
                .await
                .expect("get")
                .as_deref(),
            Some("1")
        );
        let off = pause(false, true, Some(db)).await.expect("off");
        assert!(off.contains("off"), "{off}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn scan_empty_and_movies_only_with_canned_probe() {
        let dir = crate::test_support::scratch("encode-scan");
        let library = crate::test_support::library_root(&dir);
        let db = dir.join("state.db");
        let store = Store::open(&db).await.expect("store");
        store
            .put_machine("library_root", &library.display().to_string())
            .await
            .expect("root");
        let empty = scan(
            &probe(HEVC10),
            true,
            Some(library.clone()),
            Some(db.clone()),
        )
        .await
        .expect("empty");
        let value: serde_json::Value = serde_json::from_str(&empty).expect("json");
        assert_eq!(value["data"]["files"].as_array().expect("files").len(), 0);

        write_schema(&library, crate::test_support::MOVIE_REL);
        write_schema(
            &library,
            "series/The.Wire.(2002).{tvdb-79126}/The.Wire.(2002).S01E01.mkv",
        );
        let scanned = scan(&probe(HEVC10), true, Some(library), Some(db))
            .await
            .expect("scan");
        let value: serde_json::Value = serde_json::from_str(&scanned).expect("json");
        let files = value["data"]["files"].as_array().expect("files");
        assert_eq!(files.len(), 1, "scan walks movies/ only: {files:?}");
        assert_eq!(files[0]["title_id"], "movie:tmdb:603");
        assert_eq!(files[0]["decision"], "nvenc_h264");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn scan_keep_and_refuse_from_probe() {
        let dir = crate::test_support::scratch("encode-scan-decisions");
        let library = crate::test_support::library_root(&dir);
        write_schema(&library, crate::test_support::MOVIE_REL);
        let db = dir.join("state.db");
        let keep = scan(&probe(H264), true, Some(library.clone()), Some(db.clone()))
            .await
            .expect("keep");
        let value: serde_json::Value = serde_json::from_str(&keep).expect("json");
        assert_eq!(value["data"]["files"][0]["decision"], "keep");
        let refuse = scan(&probe(HDR), true, Some(library), Some(db))
            .await
            .expect("refuse");
        let value: serde_json::Value = serde_json::from_str(&refuse).expect("json");
        assert_eq!(value["data"]["files"][0]["decision"], "refuse");
        let _ = std::fs::remove_dir_all(dir);
    }

    async fn seeded_run(
        dir: &Path,
        exec: &Transcript,
        title: Option<&str>,
        nvenc_cap: &str,
        paused: bool,
    ) -> Result<String, AppError> {
        let library = crate::test_support::library_root(dir);
        write_schema(&library, crate::test_support::MOVIE_REL);
        let ds = crate::test_support::write_ds(dir, crate::test_support::DS_UNLOCKED);
        let store = Store::open(dir.join("state.db")).await.expect("store");
        store
            .put_machine("library_root", &library.display().to_string())
            .await
            .expect("root");
        store
            .put_machine("nvenc_cap", nvenc_cap)
            .await
            .expect("cap");
        if paused {
            store.put_machine("encode_pause", "1").await.expect("pause");
        }
        let title_id = TitleId::movie("603").expect("id");
        let rel = render(&title_id, &Placement::movie("The.Matrix", 1999, "mkv")).expect("rel");
        store
            .record_install(
                &title_id,
                &Blake3Hex::of_bytes(b"original-hevc"),
                rel.to_str().unwrap(),
            )
            .await
            .expect("index");
        run(
            exec,
            true,
            title.map(str::to_string),
            Some(dir.join("state.db")),
            Some(library),
            Some(ds),
            Some(dir.to_path_buf()),
        )
        .await
    }

    #[tokio::test]
    async fn run_cap_zero_without_title_is_ok() {
        let dir = crate::test_support::scratch("encode-cap0");
        let json = seeded_run(&dir, &probe(HEVC10), None, "0", false)
            .await
            .expect("cap0");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["data"]["ran"], 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn run_title_keep_refuse_paused_and_missing() {
        let dir = crate::test_support::scratch("encode-title");
        let keep = seeded_run(&dir, &probe(H264), Some("movie:tmdb:603"), "1", false)
            .await
            .expect("keep");
        let value: serde_json::Value = serde_json::from_str(&keep).expect("json");
        assert_eq!(value["data"]["skipped"], 1);

        let err = seeded_run(&dir, &probe(HDR), Some("movie:tmdb:603"), "1", false)
            .await
            .expect_err("refuse");
        assert!(matches!(err, AppError::Policy(_)), "{err}");

        let paused = seeded_run(&dir, &probe(HEVC10), Some("movie:tmdb:603"), "1", true)
            .await
            .expect("paused");
        let value: serde_json::Value = serde_json::from_str(&paused).expect("json");
        assert_eq!(value["data"]["paused"], true);
        assert_eq!(value["data"]["skipped"], 1);

        let missing = run(
            &probe(HEVC10),
            true,
            Some("movie:tmdb:999".into()),
            Some(dir.join("state.db")),
            Some(crate::test_support::library_root(&dir)),
            Some(crate::test_support::write_ds(
                &dir,
                crate::test_support::DS_UNLOCKED,
            )),
            Some(dir.to_path_buf()),
        )
        .await
        .expect_err("missing");
        assert!(matches!(missing, AppError::Usage(_)), "{missing}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn run_title_nvenc_replaces_without_ffmpeg_binary() {
        let dir = crate::test_support::scratch("encode-nvenc");
        let json = seeded_run(&dir, &ffmpeg(HEVC10), Some("movie:tmdb:603"), "1", false)
            .await
            .expect("nvenc");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["data"]["ran"], 1);
        let store = Store::open(dir.join("state.db")).await.expect("store");
        let entry = store
            .get_title(&TitleId::movie("603").expect("id"))
            .await
            .expect("title")
            .expect("row");
        assert_ne!(
            entry.current_b3().as_str(),
            Blake3Hex::of_bytes(b"original-hevc").as_str()
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
