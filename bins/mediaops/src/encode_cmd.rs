use std::path::PathBuf;

use mediaops_core::{
    DesiredState, EncodeEvent, Envelope, ExecPort, Job, JobEvent, JobKind, JobState, TitleId,
    encode_ready, parse_placement, render_placement,
};
use mediaops_encode::{
    EncodeDecision, TranscodeSpec, classify, encode_to_converting, probe_media, replace_converting,
    session_cap, should_start_next,
};
use mediaops_store::Store;
use mediaops_sync::scan_schema_files;
use serde::{Deserialize, Serialize};

use crate::AppError;
use crate::bootstrap;
use crate::out::{
    Style, Tone, finish, hints_from_index, hints_from_jobs, human_from_path, human_title_id,
    human_title_id_str, merge_hints, resolve_title, row,
};

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
    let library_root = if crate::api_legacy::use_home(&state_db) {
        crate::api_legacy::HomeLibrary::load()
            .await?
            .root(library_root)?
    } else {
        let store = Store::open(state_db.unwrap_or_else(bootstrap::default_state_db))
            .await
            .map_err(crate::api_legacy::error)?;
        match library_root {
            Some(root) => root,
            None => store
                .get_machine("library_root")
                .await
                .map_err(crate::api_legacy::error)?
                .map(PathBuf::from)
                .ok_or_else(|| {
                    AppError::Usage("pass --library-root or library bootstrap".into())
                })?,
        }
    };
    let movies = library_root.join("movies");
    let mut files = Vec::new();
    if movies.is_dir() {
        for file in scan_schema_files(&library_root).map_err(runtime_display)? {
            if file.title_id.kind() != mediaops_core::TitleKind::Movie {
                continue;
            }
            let path = library_root.join(&file.path);
            let media = probe_media(exec, &path)
                .await
                .map_err(|err| AppError::Runtime(anyhow::anyhow!("probe_error: {err}")))?;
            let decision = classify(file.title_id.kind(), &media);
            files.push(ScanFile {
                path: file.path.clone(),
                title_id: file.title_id.render(),
                decision: decision.as_str().to_string(),
            });
        }
    }
    let data = ScanData { files };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format_scan(&data.files))
    }
}

fn format_scan(files: &[ScanFile]) -> String {
    if files.is_empty() {
        return "nothing to encode".into();
    }
    let style = Style::stdout();
    let mut lines = Vec::new();
    for file in files {
        let title =
            human_from_path(&file.path).unwrap_or_else(|| human_title_id_str(&file.title_id));
        let (verb, tone, meta) = match file.decision.as_str() {
            "nvenc_h264" => ("encode", Tone::Go, ""),
            "keep" => ("keep", Tone::Quiet, ""),
            "refuse" => ("skip", Tone::Quiet, "hdr"),
            other => ("scan", Tone::Quiet, other),
        };
        lines.push(row(style, verb, tone, &title, meta));
    }
    finish(lines)
}

pub async fn pause(json: bool, off: bool, state_db: Option<PathBuf>) -> Result<String, AppError> {
    if crate::api_legacy::use_home(&state_db) {
        let mut home = crate::api_legacy::HomeLibrary::load().await?;
        if let mediaops_core::Spec::Cluster(spec) = &mut home.cluster.spec {
            spec.encode_pause = !off;
        }
        home.api
            .patch(home.cluster, "spec")
            .await
            .map_err(crate::api_legacy::error)?;
        return if json {
            serde_json::to_string(&Envelope::ok(PauseData { encode_pause: !off }))
                .map_err(crate::api_legacy::error)
        } else {
            Ok(if off {
                "encode    running"
            } else {
                "encode    paused"
            }
            .into())
        };
    }
    let state_db = crate::api_legacy::state_db_path(state_db);
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
        Ok(if data.encode_pause {
            "encode    paused".into()
        } else {
            "encode    running".into()
        })
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
    if crate::api_legacy::use_home(&state_db) {
        return run_home(
            exec,
            json,
            title,
            state_db,
            library_root,
            desired_state,
            config_dir,
        )
        .await;
    }
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let state_db = crate::api_legacy::state_db_path(state_db);
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
        let title_id = resolve_encode_title(&store, &raw).await?;
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
                    Ok(row(
                        Style::stdout(),
                        "keep",
                        Tone::Quiet,
                        &human_title_id(&title_id),
                        "",
                    ))
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
                Ok("encode    paused".into())
            };
        }
        let (_, placement) = parse_placement(path.strip_prefix(&library_root).unwrap_or(&path))
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
        let (dest, digest) = replace_converting(spec, converting)
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        store
            .record_replace(&library_rel(&library_root, &dest), &digest)
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
            Ok(row(
                Style::stdout(),
                "encoded",
                Tone::Go,
                &human_title_id(&title_id),
                "",
            ))
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
            Ok("nothing to encode".into())
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
        let indexed = !store
            .get_title(job.title_id())
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
            .is_empty();
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
        Ok(format_encode_run(&data))
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_home(
    exec: &impl ExecPort,
    json: bool,
    title: Option<String>,
    state_db: Option<PathBuf>,
    library_root: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    config_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    let state_db = crate::api_legacy::state_db_path(state_db);
    let _lock =
        bootstrap::exclusive_lock(&bootstrap::lock_path(&state_db)).map_err(map_bootstrap)?;
    let mut home = crate::api_legacy::HomeLibrary::load().await?;
    let root = home.root(library_root)?;
    recover_encode_proofs(&home, &root).await?;
    let rows = home.rows(false).await?;
    let selected = title
        .as_deref()
        .map(|raw| {
            TitleId::parse(raw)
                .map_err(|_| ())
                .or_else(|_| resolve_title(raw, &hints_from_index(&rows)).map_err(|_| ()))
                .map_err(|_| AppError::Usage(format!("unknown library title: {raw}")))
        })
        .transpose()?;
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|row| {
            selected.as_ref().map_or(
                row.title_id().kind() == mediaops_core::TitleKind::Movie,
                |id| row.title_id() == id,
            )
        })
        .collect();
    if selected.is_some() && rows.is_empty() {
        return Err(AppError::Usage(
            "no verified Home Title file for this title".into(),
        ));
    }
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let config = desired_state.unwrap_or_else(|| bootstrap::default_desired_state(&config_dir));
    let ds = DesiredState::from_toml(&std::fs::read_to_string(config).map_err(runtime_display)?)
        .map_err(runtime_display)?;
    // The legacy database retains local GPU capabilities and ffmpeg discovery;
    // all library proofs and the pause flag above come from Home.
    let capabilities = Store::open(state_db).await.map_err(runtime_display)?;
    let nvenc_cap = capabilities
        .get_machine("nvenc_cap")
        .await
        .map_err(runtime_display)?
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let cap = session_cap(ds.max_nvenc(), nvenc_cap, nvenc_cap > 0);
    if selected.is_some() && cap == 0 {
        return Err(AppError::Policy("no NVENC capacity".into()));
    }
    let ffmpeg = capabilities
        .get_machine("ffmpeg_path")
        .await
        .map_err(runtime_display)?
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "ffmpeg".into());
    let mut data = EncodeRunData {
        ran: 0,
        skipped: 0,
        paused: home.spec()?.encode_pause,
    };
    let mut maintenance = None;
    let outcome: Result<(), AppError> = async {
    for row in rows {
        let cluster = home
            .api
            .get(mediaops_core::Kind::Cluster, mediaops_core::CLUSTER_NAME)
            .await
            .map_err(runtime_display)?;
        let mediaops_core::Spec::Cluster(spec) = &cluster.spec else {
            return Err(runtime_display("invalid Cluster"));
        };
        data.paused = spec.encode_pause;
        if !should_start_next(data.paused, cap) {
            data.skipped += 1;
            continue;
        }
        let path = root.join(row.path());
        let digest = mediaops_core::Blake3Hex::of_reader(
            std::fs::File::open(&path).map_err(runtime_display)?,
        )
        .map_err(runtime_display)?;
        if &digest != row.current_b3() {
            return Err(AppError::DriftVerify(format!(
                "library file changed: {}",
                row.path()
            )));
        }
        let media = probe_media(exec, &path).await.map_err(runtime_display)?;
        match classify(row.title_id().kind(), &media) {
            EncodeDecision::Keep => {
                data.skipped += 1;
                continue;
            }
            EncodeDecision::Refuse if selected.is_some() => {
                return Err(AppError::Policy(
                    "encode refused by policy (HDR/DV/2160p)".into(),
                ));
            }
            EncodeDecision::Refuse => {
                data.skipped += 1;
                continue;
            }
            EncodeDecision::NvencH264 => {}
        }
        if maintenance.is_none() {
            home.cluster = cluster;
            maintenance = Some(home.begin_maintenance().await?);
        }
        let (_, placement) = parse_placement(row.path()).map_err(runtime_display)?;
        let spec = TranscodeSpec {
            library_root: &root,
            title_id: row.title_id(),
            placement: &placement,
            ffmpeg: &ffmpeg,
        };
        let converting = encode_to_converting(exec, spec)
            .await
            .map_err(runtime_display)?;
        let converted_digest = mediaops_core::Blake3Hex::of_reader(
            std::fs::File::open(&converting).map_err(runtime_display)?,
        )
        .map_err(runtime_display)?;
        let proof = PendingEncodeProof {
            title_id: row.title_id().render(),
            path: row.path().into(),
            install_b3: row.install_b3().clone(),
            previous_b3: row.current_b3().clone(),
            current_b3: converted_digest,
        };
        let journal = persist_encode_proof(&root, &proof)?;
        let (dest, current) = replace_converting(spec, converting).map_err(runtime_display)?;
        home.record_replace(&library_rel(&root, &dest), &current).await.map_err(|err| {
            runtime_display(format!("encoded file retained but Home proof publication failed: {err}; Cluster remains locked"))
        })?;
        std::fs::remove_file(journal).map_err(runtime_display)?;
        data.ran += 1;
    }
    Ok(())
    }.await;
    if let Err(err) = outcome {
        return Err(if maintenance.is_some() {
            crate::api_legacy::maintenance_failure(err)
        } else {
            err
        });
    }
    if let Some(was_locked) = maintenance {
        home.finish_maintenance(was_locked).await?;
    }
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(runtime_display)
    } else if let Some(id) = selected {
        Ok(if data.paused {
            "encode    paused".into()
        } else {
            row(
                Style::stdout(),
                if data.ran > 0 { "encoded" } else { "keep" },
                if data.ran > 0 { Tone::Go } else { Tone::Quiet },
                &human_title_id(&id),
                "",
            )
        })
    } else {
        Ok(format_encode_run(&data))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingEncodeProof {
    title_id: String,
    path: String,
    install_b3: mediaops_core::Blake3Hex,
    previous_b3: mediaops_core::Blake3Hex,
    current_b3: mediaops_core::Blake3Hex,
}

fn persist_encode_proof(
    root: &std::path::Path,
    proof: &PendingEncodeProof,
) -> Result<PathBuf, AppError> {
    use std::io::Write;
    let directory = root.join("_incoming").join("encode-proofs");
    std::fs::create_dir_all(&directory).map_err(runtime_display)?;
    let key = mediaops_core::Blake3Hex::of_bytes(proof.path.as_bytes());
    let journal = directory.join(format!("{}.json", key.as_str()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&journal)
        .map_err(runtime_display)?;
    file.write_all(&serde_json::to_vec(proof).map_err(runtime_display)?)
        .map_err(runtime_display)?;
    file.sync_all().map_err(runtime_display)?;
    std::fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(runtime_display)?;
    Ok(journal)
}

async fn recover_encode_proofs(
    home: &crate::api_legacy::HomeLibrary,
    root: &std::path::Path,
) -> Result<(), AppError> {
    let directory = root.join("_incoming").join("encode-proofs");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(runtime_display(err)),
    };
    let rows = home.rows(true).await?;
    for entry in entries {
        let entry = entry.map_err(runtime_display)?;
        if !entry.file_type().map_err(runtime_display)?.is_file() {
            continue;
        }
        let proof: PendingEncodeProof =
            serde_json::from_slice(&std::fs::read(entry.path()).map_err(runtime_display)?)
                .map_err(runtime_display)?;
        let path = crate::api_legacy::schema_relative(root, &proof.path)?;
        let row = rows
            .iter()
            .find(|row| row.path() == path && row.title_id().render() == proof.title_id)
            .ok_or_else(|| runtime_display("pending encode has no matching Home Title proof"))?;
        if row.install_b3() != &proof.install_b3
            || (row.current_b3() != &proof.previous_b3 && row.current_b3() != &proof.current_b3)
        {
            return Err(AppError::DriftVerify(format!(
                "pending encode conflicts with Home proof: {path}"
            )));
        }
        let actual = mediaops_core::Blake3Hex::of_reader(
            std::fs::File::open(root.join(&path)).map_err(runtime_display)?,
        )
        .map_err(runtime_display)?;
        if actual == proof.current_b3 {
            home.publish_rows(
                &[mediaops_core::TitleIndexEntry::new(
                    row.title_id().clone(),
                    &path,
                    proof.install_b3,
                    proof.current_b3,
                )],
                false,
            )
            .await?;
        } else if actual != proof.previous_b3 {
            return Err(AppError::DriftVerify(format!(
                "pending encode file changed: {path}"
            )));
        }
        std::fs::remove_file(entry.path()).map_err(runtime_display)?;
    }
    Ok(())
}

fn format_encode_run(data: &EncodeRunData) -> String {
    if data.paused && data.ran == 0 {
        return "encode    paused".into();
    }
    if data.ran == 0 {
        return "nothing to encode".into();
    }
    row(
        Style::stdout(),
        "encoded",
        Tone::Go,
        "",
        &data.ran.to_string(),
    )
}

async fn resolve_encode_title(store: &Store, raw: &str) -> Result<TitleId, AppError> {
    if let Ok(id) = TitleId::parse(raw) {
        return Ok(id);
    }
    let titles = store
        .list_titles()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let jobs = store
        .list_jobs()
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    resolve_title(
        raw,
        &merge_hints(
            hints_from_index(&titles)
                .into_iter()
                .chain(hints_from_jobs(&jobs))
                .collect(),
        ),
    )
    .map_err(AppError::Usage)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AfterInstall {
    Ran,
    Skipped,
}

/// Post-install encode trigger. Deleting the `run` verb removed its only
/// caller and `mediaops-pull` cannot take it over: `mediaops-encode` is barred
/// from that binary's workspace closure. Kept, tested, and unwired until the
/// new pipeline grows a place for it.
#[allow(dead_code)]
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
) -> Result<AfterInstall, AppError> {
    if paused || cap == 0 {
        return Ok(AfterInstall::Skipped);
    }
    let media = probe_media(exec, dest)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("probe_error: {err}")))?;
    match classify(title_id.kind(), &media) {
        EncodeDecision::NvencH264 => {}
        EncodeDecision::Keep | EncodeDecision::Refuse => return Ok(AfterInstall::Skipped),
    }
    let encode = store
        .create_job(JobKind::Encode, title_id, Some(pull.id()))
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    if !encode_ready(&encode, Some(pull), true) {
        return Ok(AfterInstall::Skipped);
    }
    encode_one(exec, store, library_root, &encode, ffmpeg).await?;
    Ok(AfterInstall::Ran)
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
        let (dest, digest) = replace_converting(spec, converting)
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        store
            .record_replace(&library_rel(library_root, &dest), &digest)
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
        store
            .advance(job.id(), JobEvent::Encode(EncodeEvent::Replace))
            .await
            .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    }
    Ok(())
}

/// The rendered file name of a placement: the one PathSchema produces.
fn placement_filename(placement: &mediaops_core::Placement) -> Result<String, AppError> {
    let rendered = render_placement(placement).map_err(runtime_display)?;
    rendered
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .ok_or_else(|| AppError::Runtime(anyhow::anyhow!("placement renders no file name")))
}

/// Library-relative form of an absolute path under `library_root` (what the
/// title index is keyed on).
fn library_rel(library_root: &std::path::Path, abs: &std::path::Path) -> String {
    abs.strip_prefix(library_root)
        .unwrap_or(abs)
        .to_string_lossy()
        .into_owned()
}

/// The library file for a title. Encode only ever targets movies, so a title
/// is one file; the first indexed row wins, then the disk.
async fn resolve_path(
    store: &Store,
    library_root: &std::path::Path,
    title_id: &TitleId,
) -> Result<PathBuf, AppError> {
    if let Some(entry) = store
        .get_title(title_id)
        .await
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
        .into_iter()
        .find(|entry| !entry.path_missing())
    {
        return Ok(library_root.join(entry.path()));
    }
    let found = scan_schema_files(library_root)
        .map_err(runtime_display)?
        .into_iter()
        .find(|file| &file.title_id == title_id)
        .map(|file| library_root.join(file.path));
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

    #[tokio::test]
    async fn interrupted_proof_publication_recovers_without_encoding_again() {
        let (home, dir, server) = crate::api_legacy::test_home("encode-proof-recovery").await;
        let root = home.root(None).expect("root");
        let relative = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        std::fs::write(&path, b"original").expect("original");
        let original = Blake3Hex::of_bytes(b"original");
        let encoded = Blake3Hex::of_bytes(b"encoded");
        let id = TitleId::movie("603").expect("id");
        home.publish_rows(
            &[mediaops_core::TitleIndexEntry::new(
                id.clone(),
                relative,
                original.clone(),
                original.clone(),
            )],
            false,
        )
        .await
        .expect("initial proof");
        let journal = persist_encode_proof(
            &root,
            &PendingEncodeProof {
                title_id: id.render(),
                path: relative.into(),
                install_b3: original.clone(),
                previous_b3: original.clone(),
                current_b3: encoded.clone(),
            },
        )
        .expect("journal");
        std::fs::write(&path, b"encoded").expect("replace completed");
        recover_encode_proofs(&home, &root).await.expect("recover");
        assert!(!journal.exists());
        let rows = home.rows(false).await.expect("proofs");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].install_b3(), &original);
        assert_eq!(rows[0].current_b3(), &encoded);
        server.abort();
        let _ = std::fs::remove_dir_all(dir);
    }

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
        assert_eq!(off, "encode    running");
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
            "series/The.Wire.(2002)/Season.01/The.Wire.(2002).S01E01.mkv",
        );
        let scanned = scan(
            &probe(HEVC10),
            true,
            Some(library.clone()),
            Some(db.clone()),
        )
        .await
        .expect("scan");
        let value: serde_json::Value = serde_json::from_str(&scanned).expect("json");
        let files = value["data"]["files"].as_array().expect("files");
        assert_eq!(files.len(), 1, "scan walks movies/ only: {files:?}");
        assert_eq!(files[0]["title_id"], "movie:key:thematrix.1999");
        assert_eq!(files[0]["decision"], "nvenc_h264");
        let human = scan(&probe(HEVC10), false, Some(library), Some(db))
            .await
            .expect("human");
        assert_eq!(human, "encode    The Matrix (1999)");
        let _ = std::fs::remove_dir_all(dir);
    }

    struct FailExec;

    impl ExecPort for FailExec {
        async fn run(&self, command: &ExecCommand) -> Result<ExecOutput, ExecError> {
            Err(ExecError::Failed {
                program: command.program.clone(),
                message: "boom".into(),
            })
        }
    }

    #[tokio::test]
    async fn scan_ffprobe_error_is_not_keep() {
        let dir = crate::test_support::scratch("encode-scan-probe-err");
        let library = crate::test_support::library_root(&dir);
        write_schema(&library, crate::test_support::MOVIE_REL);
        let db = dir.join("state.db");
        let err = scan(&FailExec, true, Some(library), Some(db))
            .await
            .expect_err("probe");
        let msg = err.to_string();
        assert!(msg.contains("probe_error"), "{msg}");
        assert!(!msg.to_ascii_lowercase().contains("keep"), "{msg}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn after_install_ffprobe_error_is_not_silent_ok() {
        let dir = crate::test_support::scratch("encode-after-probe-err");
        let library = crate::test_support::library_root(&dir);
        write_schema(&library, crate::test_support::MOVIE_REL);
        let store = Store::open(dir.join("state.db")).await.expect("store");
        let title_id = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let pull = store
            .create_job(JobKind::Pull, &title_id, None)
            .await
            .expect("pull");
        let dest = library.join(crate::test_support::MOVIE_REL);
        let err = after_install(
            &FailExec, &store, &library, &title_id, &dest, &pull, "ffmpeg", 1, false,
        )
        .await
        .expect_err("probe");
        let msg = err.to_string();
        assert!(msg.contains("probe_error"), "{msg}");
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
        let title_id = TitleId::movie_key("The.Matrix", 1999).expect("id");
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
        let keep = seeded_run(
            &dir,
            &probe(H264),
            Some("movie:key:thematrix.1999"),
            "1",
            false,
        )
        .await
        .expect("keep");
        let value: serde_json::Value = serde_json::from_str(&keep).expect("json");
        assert_eq!(value["data"]["skipped"], 1);

        let err = seeded_run(
            &dir,
            &probe(HDR),
            Some("movie:key:thematrix.1999"),
            "1",
            false,
        )
        .await
        .expect_err("refuse");
        assert!(matches!(err, AppError::Policy(_)), "{err}");

        let paused = seeded_run(
            &dir,
            &probe(HEVC10),
            Some("movie:key:thematrix.1999"),
            "1",
            true,
        )
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
        let json = seeded_run(
            &dir,
            &ffmpeg(HEVC10),
            Some("movie:key:thematrix.1999"),
            "1",
            false,
        )
        .await
        .expect("nvenc");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["data"]["ran"], 1);
        let store = Store::open(dir.join("state.db")).await.expect("store");
        let entry = store
            .get_title(&TitleId::movie_key("The.Matrix", 1999).expect("id"))
            .await
            .expect("title")
            .into_iter()
            .next()
            .expect("row");
        assert_ne!(
            entry.current_b3().as_str(),
            Blake3Hex::of_bytes(b"original-hevc").as_str()
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
