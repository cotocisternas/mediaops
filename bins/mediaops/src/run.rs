use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mediaops_core::{
    Action, ControlPort, DesiredState, Envelope, ExecPort, ExitCode, HoldDecision, JobState, Plan,
    Probe, WantState, free_bytes,
};
use mediaops_store::Store;
use mediaops_sync::{ApplyCtx, ApplyError, PlanRequest, apply, plan_actions, scan_schema_files};

use crate::out::{
    PullMeter, Style, Tone, finish, fmt_bytes, human_from_path, human_placement, human_title_id,
    human_title_id_str, indent, row,
};
use mediaops_transfer::{
    HomeChannel, configure_pool, connect_home, grpc_source, list_entries, pool_status, probe_range,
};
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;
use crate::encode_cmd::AfterInstall;

#[derive(Debug, Serialize)]
struct PlanData {
    path: String,
    actions: Vec<Action>,
    first_candidate_breaches: bool,
}

#[derive(Debug, Serialize)]
struct EncodeData {
    ran: usize,
    skipped: usize,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct UnmonitorFailureView {
    title_id: String,
    error: String,
}

#[derive(Debug, Serialize)]
struct CopyFailureView {
    title_id: String,
    remote: String,
    error: String,
}

#[derive(Debug, Serialize)]
struct RunData {
    path: String,
    copies: usize,
    skips: usize,
    /// Media files on the box the schema could not place (see the plan).
    reviews: usize,
    installed: Vec<String>,
    /// Copies that did not land this run. Their `.partial` stays; next run
    /// resumes them. Reported, not an exit code: the rest of the plan ran.
    copy_failed: Vec<CopyFailureView>,
    encode: EncodeData,
    /// Grabber refused these Unmonitors. The copies still landed and the next
    /// run re-emits the action, so this is reported, not an exit code.
    unmonitor_failed: Vec<UnmonitorFailureView>,
}

struct PreparedPlan {
    _lock: File,
    planned: mediaops_sync::Planned,
    path: PathBuf,
    library_root: PathBuf,
    store: Store,
    socket: PathBuf,
    tls_dir: PathBuf,
    desired_state: PathBuf,
}

#[allow(clippy::too_many_arguments)]
pub async fn cmd_plan(
    json: bool,
    state_db: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    library_root: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    plans_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    let prepared = prepare(
        state_db,
        desired_state,
        library_root,
        socket,
        tls_dir,
        config_dir,
        plans_dir,
    )
    .await?;
    let data = PlanData {
        path: prepared.path.display().to_string(),
        actions: prepared.planned.actions.clone(),
        first_candidate_breaches: prepared.planned.first_candidate_breaches,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format_plan_human(&data.path, &data.actions))
    }
}

fn format_plan_human(path: &str, actions: &[Action]) -> String {
    let style = Style::stdout();
    let mut lines = Vec::new();
    for action in actions {
        match action {
            Action::Copy {
                file_len,
                placement,
                ..
            } => lines.push(row(
                style,
                "copy",
                Tone::Go,
                &human_placement(placement),
                &fmt_bytes(*file_len),
            )),
            Action::Skip { title_id, reason } => {
                let title = title_id.as_ref().map(human_title_id).unwrap_or_default();
                lines.push(row(style, "skip", Tone::Quiet, &title, reason));
            }
            Action::Review { remote, reason } => {
                let title = match remote {
                    Some(r) => format!("{} / {}", r.root_id(), r.rel_path().display()),
                    None => String::new(),
                };
                lines.push(row(style, "review", Tone::Wait, &title, reason));
            }
            Action::Unmonitor { title_id } => {
                lines.push(row(
                    style,
                    "drop",
                    Tone::Quiet,
                    &human_title_id(title_id),
                    "",
                ));
            }
            Action::DeleteRemote { remote } => lines.push(row(
                style,
                "delete",
                Tone::Wait,
                &format!("{} / {}", remote.root_id(), remote.rel_path().display()),
                "",
            )),
            Action::Encode { title_id } => {
                lines.push(row(
                    style,
                    "encode",
                    Tone::Go,
                    &human_title_id(title_id),
                    "",
                ));
            }
            Action::Reclaim | Action::EdgeApply | Action::GrabApply => {}
        }
    }
    if lines.is_empty() {
        return "nothing to copy".into();
    }
    if !path.is_empty() {
        lines.push(String::new());
        lines.push(style.dim(path));
    }
    finish(lines)
}

fn format_run_human(data: &RunData) -> String {
    let style = Style::stdout();
    let mut lines = Vec::new();
    for path in &data.installed {
        let title = human_from_path(path).unwrap_or_else(|| path.clone());
        lines.push(row(style, "copied", Tone::Go, &title, ""));
    }
    for fail in &data.copy_failed {
        lines.push(row(
            style,
            "failed",
            Tone::Bad,
            &human_title_id_str(&fail.title_id),
            "",
        ));
        if !fail.remote.is_empty() {
            lines.push(indent(style, &fail.remote));
        }
        lines.push(indent(style, &fail.error));
    }
    for fail in &data.unmonitor_failed {
        lines.push(row(
            style,
            "drop",
            Tone::Bad,
            &human_title_id_str(&fail.title_id),
            &fail.error,
        ));
    }
    if data.encode.ran > 0 {
        lines.push(row(
            style,
            "encoded",
            Tone::Go,
            "",
            &data.encode.ran.to_string(),
        ));
    }
    if let Some(err) = &data.encode.error {
        lines.push(row(style, "encode", Tone::Bad, "failed", err));
    }
    if lines.is_empty() {
        return "nothing to copy".into();
    }
    finish(lines)
}

#[allow(clippy::too_many_arguments)]
pub async fn cmd_run(
    exec: &impl ExecPort,
    json: bool,
    state_db: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    library_root: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    plans_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    let prepared = prepare(
        state_db,
        desired_state,
        library_root,
        socket,
        tls_dir,
        config_dir,
        plans_dir,
    )
    .await?;
    let copies = prepared
        .planned
        .actions
        .iter()
        .filter(|a| matches!(a, Action::Copy { .. }))
        .count();
    if prepared
        .planned
        .actions
        .iter()
        .any(|a| matches!(a, Action::EdgeApply))
    {
        return Err(AppError::Policy(
            "panel fingerprint freeze; run mediaops repair edge --repair --confirm".into(),
        ));
    }
    refuse_empty_apply(copies, prepared.planned.first_candidate_breaches)?;

    let channel = connect_home(&prepared.socket, &prepared.tls_dir)
        .await
        .map_err(runtime_display)?;
    let active =
        std::fs::read(&prepared.desired_state).map_err(|err| AppError::Runtime(err.into()))?;
    let bytes = std::fs::read(&prepared.path).map_err(|err| AppError::Runtime(err.into()))?;
    let plan = Plan::from_json_slice(&bytes).map_err(runtime_display)?;
    let pinned = plan
        .desired_state()
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?
        .range_concurrency();
    let n = configure_from_probes(&prepared.store, channel.clone(), pinned).await?;
    let control = mediaops_proto::ControlPortClient::new(
        mediaops_proto::control_client::ControlClient::new(channel.clone()),
    );
    let meter = Arc::new(Mutex::new(None::<(String, PullMeter)>));
    let on_pull_progress = if json {
        None
    } else {
        let meter = meter.clone();
        Some(Arc::new(move |label: String, done, total| {
            let label = crate::out::humanize_schema_label(&label);
            let mut slot = meter.lock().unwrap_or_else(|e| e.into_inner());
            match slot.as_mut() {
                Some((current, m)) if *current == label => m.update(done, total),
                _ => {
                    if let Some((_, mut old)) = slot.take() {
                        old.finish();
                    }
                    let mut m = PullMeter::new(label.clone());
                    m.update(done, total);
                    *slot = Some((label, m));
                }
            }
        }) as mediaops_sync::PullProgress)
    };
    let report = apply(
        &plan,
        &active,
        ApplyCtx {
            jobs: &prepared.store,
            titles: &prepared.store,
            source: grpc_source(channel),
            library_root: &prepared.library_root,
            concurrency: n as usize,
            control: Some(&control),
            on_pull_progress,
        },
    )
    .await
    .map_err(map_apply)?;
    if let Ok(mut slot) = meter.lock()
        && let Some((_, mut m)) = slot.take()
    {
        m.finish();
    }

    let ds = plan
        .desired_state()
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("{err}")))?;
    let nvenc_cap = prepared
        .store
        .get_machine("nvenc_cap")
        .await
        .map_err(runtime_display)?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let paused = prepared
        .store
        .get_machine("encode_pause")
        .await
        .map_err(runtime_display)?
        .as_deref()
        == Some("1");
    let ffmpeg = prepared
        .store
        .get_machine("ffmpeg_path")
        .await
        .map_err(runtime_display)?
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "ffmpeg".into());
    let cap = mediaops_encode::session_cap(ds.max_nvenc(), nvenc_cap, nvenc_cap > 0);
    let mut encode = EncodeData {
        ran: 0,
        skipped: 0,
        error: None,
    };
    for inst in &report.installed {
        // Only movies are encode candidates; a FLAC has no video stream to
        // probe and a show is left alone by policy.
        if inst.title_id.kind() != mediaops_core::TitleKind::Movie {
            encode.skipped += 1;
            continue;
        }
        if let Some(pull) = prepared
            .store
            .get_job(inst.pull_job_id)
            .await
            .map_err(runtime_display)?
        {
            match crate::encode_cmd::after_install(
                exec,
                &prepared.store,
                &prepared.library_root,
                &inst.title_id,
                &inst.path,
                &pull,
                &ffmpeg,
                cap,
                paused,
            )
            .await
            {
                Ok(AfterInstall::Ran) => encode.ran += 1,
                Ok(AfterInstall::Skipped) => encode.skipped += 1,
                // The copies already landed and are indexed; an encode that
                // cannot start is data in the report, not a lost run. The
                // Encode job (when one was created) stays for `encode run`.
                Err(err) => {
                    tracing::warn!(
                        title = %inst.title_id,
                        path = %inst.path.display(),
                        error = %err,
                        "post-install encode failed; continuing"
                    );
                    encode.skipped += 1;
                    encode.error = Some(match encode.error.take() {
                        Some(prev) => format!("{prev}; {err}"),
                        None => err.to_string(),
                    });
                }
            }
        } else {
            encode.skipped += 1;
        }
    }

    let _ = std::fs::remove_file(&prepared.path);
    let data = RunData {
        path: prepared.path.display().to_string(),
        copies: report.copies,
        skips: report.skips,
        reviews: report.reviews,
        installed: report
            .installed
            .iter()
            .map(|i| i.path.display().to_string())
            .collect(),
        copy_failed: report
            .copy_failed
            .iter()
            .map(|f| CopyFailureView {
                title_id: f.title_id.render(),
                remote: f.remote.rel_path().display().to_string(),
                error: f.error.clone(),
            })
            .collect(),
        encode,
        unmonitor_failed: report
            .unmonitor_failed
            .iter()
            .map(|f| UnmonitorFailureView {
                title_id: f.title_id.render(),
                error: f.error.clone(),
            })
            .collect(),
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format_run_human(&data))
    }
}

#[allow(clippy::too_many_arguments)]
async fn prepare(
    state_db: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    library_root: Option<PathBuf>,
    socket: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    plans_dir: Option<PathBuf>,
) -> Result<PreparedPlan, AppError> {
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let socket = socket.unwrap_or_else(bootstrap::default_socket);
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let lock_path = bootstrap::lock_path(&state_db);
    let lock = bootstrap::exclusive_lock(&lock_path).map_err(map_bootstrap)?;
    let desired_state =
        desired_state.unwrap_or_else(|| bootstrap::default_desired_state(&config_dir));
    let plans_dir = plans_dir.unwrap_or_else(bootstrap::default_plans_dir);
    let toml_bytes = std::fs::read(&desired_state).map_err(|err| AppError::Runtime(err.into()))?;
    let ds = DesiredState::from_toml_bytes(&toml_bytes).map_err(runtime_display)?;
    let store = Store::open(&state_db).await.map_err(runtime_display)?;
    let library_root = match library_root {
        Some(p) => p,
        None => store
            .get_machine("library_root")
            .await
            .map_err(runtime_display)?
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
    let free = free_bytes(&library_root).map_err(runtime_display)?;
    let channel = connect_home(&socket, &tls_dir)
        .await
        .map_err(runtime_display)?;
    let listings = list_entries(channel.clone())
        .await
        .map_err(runtime_display)?;
    let title_index = store.list_titles().await.map_err(runtime_display)?;
    let on_disk = scan_schema_files(&library_root).map_err(runtime_display)?;
    let jobs = store.list_jobs().await.map_err(runtime_display)?;
    let open_wants: Vec<_> = jobs
        .into_iter()
        .filter(|j| matches!(j.state(), JobState::Want(WantState::Open)))
        .collect();
    let control = mediaops_proto::ControlPortClient::new(
        mediaops_proto::control_client::ControlClient::new(channel.clone()),
    );
    // Only a desired-state with an `[edge]` table has an nginx edge to freeze
    // on. A plain folder on a box (or a non-Swizzin box) planned fine before
    // the panel existed and must keep planning.
    let edge_frozen = if ds.has_edge() {
        let edge = control.edge_check().await.map_err(runtime_display)?;
        let last = store
            .get_machine(crate::doctor::EDGE_FINGERPRINT_KEY)
            .await
            .map_err(runtime_display)?;
        crate::doctor::is_frozen(&edge, last.as_deref())
    } else {
        false
    };
    let live = control.hold_list().await.map_err(runtime_display)?;
    let mut approved = Vec::new();
    for item in &live {
        if store.get_hold(&item.key).await.map_err(runtime_display)? == Some(HoldDecision::Approved)
        {
            approved.push(item.clone());
        }
    }
    let wanted_missing = control.wanted_missing().await.map_err(runtime_display)?;
    let root_kinds = ds.root_kinds();
    let planned = plan_actions(PlanRequest {
        listings: &listings,
        root_kinds: &root_kinds,
        title_index: &title_index,
        on_disk: &on_disk,
        open_wants: &open_wants,
        desired: &ds,
        free_bytes: free,
        edge_frozen,
        holds: &live,
        approved: &approved,
        wanted_missing: &wanted_missing,
    });
    let plan = Plan::from_toml_bytes(toml_bytes)
        .map_err(runtime_display)?
        .with_actions(planned.actions.clone());
    std::fs::create_dir_all(&plans_dir).map_err(|err| AppError::Runtime(err.into()))?;
    let path = unique_plan_path(&plans_dir, &plan)?;
    Ok(PreparedPlan {
        _lock: lock,
        planned,
        path,
        library_root,
        store,
        socket,
        tls_dir,
        desired_state,
    })
}

async fn configure_from_probes(
    store: &Store,
    channel: HomeChannel,
    pinned: Option<u32>,
) -> Result<u32, AppError> {
    // An operator-pinned `range_concurrency` wins over the probe: the probe
    // measures 1 MiB bursts through TLS handshakes and is a poor judge of a
    // WAN, and the pin is the desired-state way to say "I know my link".
    if let Some(n) = pinned {
        configure_pool(channel, n).await.map_err(runtime_display)?;
        return Ok(n);
    }
    let (fingerprint, _) = pool_status(channel.clone())
        .await
        .map_err(runtime_display)?;
    let n = match store
        .get_probe(&fingerprint)
        .await
        .map_err(runtime_display)?
    {
        Some(probe) => probe.range_concurrency,
        None => {
            let n = probe_range(channel.clone(), 32)
                .await
                .map_err(runtime_display)?;
            store
                .put_probe(&Probe {
                    endpoint_fingerprint: fingerprint,
                    range_concurrency: n,
                })
                .await
                .map_err(runtime_display)?;
            n
        }
    };
    configure_pool(channel, n).await.map_err(runtime_display)?;
    Ok(n)
}

fn map_apply(err: ApplyError) -> AppError {
    if err.is_snapshot_mismatch() {
        return AppError::DriftVerify(err.to_string());
    }
    match err {
        ApplyError::Control(ctrl) => match ctrl.exit_code {
            ExitCode::PolicyRefusal => AppError::Policy(ctrl.message),
            ExitCode::DriftVerify => AppError::DriftVerify(ctrl.message),
            ExitCode::LockConflict => AppError::LockConflict(ctrl.message),
            ExitCode::Usage => AppError::Usage(ctrl.message),
            _ => AppError::Runtime(anyhow::anyhow!("{}", ctrl.message)),
        },
        other => AppError::Runtime(anyhow::anyhow!("{other}")),
    }
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

fn refuse_empty_apply(copies: usize, first_candidate_breaches: bool) -> Result<(), AppError> {
    if copies == 0 && first_candidate_breaches {
        Err(AppError::Policy(
            "watermark/max_copy: first candidate alone would breach; refusing empty apply".into(),
        ))
    } else {
        Ok(())
    }
}

fn unique_plan_path(plans_dir: &Path, plan: &Plan) -> Result<PathBuf, AppError> {
    let stamp = bootstrap::utc_compact();
    let b3 = &plan.desired_state_b3().as_str()[..12];
    let pid = std::process::id();
    let json = serde_json::to_vec_pretty(plan).map_err(|e| AppError::Runtime(e.into()))?;
    for n in 0u32..1000 {
        let name = if n == 0 {
            format!("{stamp}-{b3}-{pid}.json")
        } else {
            format!("{stamp}-{b3}-{pid}-{n}.json")
        };
        let path = plans_dir.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(&json)
                    .map_err(|err| AppError::Runtime(err.into()))?;
                return Ok(path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(AppError::Runtime(err.into())),
        }
    }
    Err(AppError::Runtime(anyhow::anyhow!(
        "plan filename collision"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{Placement, RemoteRef, TitleId};

    #[test]
    fn plan_human_lists_the_copy_not_just_the_json_path() {
        let remote = RemoteRef::from_wire_parts(
            "usenet_movies".into(),
            "Hearts.of.Darkness-GERMAN/Hearts.mkv".into(),
        )
        .expect("remote");
        let text = format_plan_human(
            "/tmp/plan.json",
            &[
                Action::Copy {
                    title_id: TitleId::movie_key("Hearts of Darkness", 1991).expect("id"),
                    remote,
                    file_len: 7_250_189_951,
                    placement: Placement::movie(
                        "Hearts.of.Darkness.A.Filmmaker's.Apocalypse",
                        1991,
                        "mkv",
                    ),
                },
                Action::GrabApply,
            ],
        );
        assert_eq!(
            text,
            "\
copy      Hearts of Darkness A Filmmaker's Apocalypse (1991)  6.8 GiB

/tmp/plan.json"
        );
    }

    #[test]
    fn plan_human_empty_is_english() {
        assert_eq!(format_plan_human("/tmp/plan.json", &[]), "nothing to copy");
        assert_eq!(
            format_plan_human("/tmp/plan.json", &[Action::GrabApply]),
            "nothing to copy"
        );
    }

    #[test]
    fn run_human_lists_what_landed() {
        let text = format_run_human(&RunData {
            path: "/tmp/plan.json".into(),
            copies: 1,
            skips: 0,
            reviews: 0,
            installed: vec![
                "movies/Hearts.of.Darkness.A.Filmmaker's.Apocalypse.(1991)/Hearts.of.Darkness.A.Filmmaker's.Apocalypse.(1991).mkv"
                    .into(),
            ],
            copy_failed: Vec::new(),
            encode: EncodeData {
                ran: 0,
                skipped: 1,
                error: None,
            },
            unmonitor_failed: Vec::new(),
        });
        assert_eq!(
            text,
            "copied    Hearts of Darkness A Filmmaker's Apocalypse (1991)"
        );
    }

    #[test]
    fn map_apply_preserves_control_usage() {
        let err = map_apply(ApplyError::Control(mediaops_core::ControlError::usage(
            "grabber is none; unmonitor requires a grabber",
        )));
        assert!(matches!(err, AppError::Usage(_)), "{err}");
    }

    #[test]
    fn empty_apply_with_first_candidate_breach_is_policy_refusal() {
        assert!(
            matches!(refuse_empty_apply(0, true), Err(AppError::Policy(_))),
            "empty apply that breaches must be policy_refusal"
        );
        assert!(refuse_empty_apply(0, false).is_ok());
        assert!(refuse_empty_apply(1, true).is_ok());
    }

    #[test]
    fn unique_plan_path_does_not_overwrite() {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-plans-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let plan = Plan::from_toml_bytes(
            b"schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n",
        )
        .expect("plan");
        let a = unique_plan_path(&dir, &plan).unwrap_or_else(|e| panic!("{e}"));
        let b = unique_plan_path(&dir, &plan).unwrap_or_else(|e| panic!("{e}"));
        assert_ne!(a, b);
        assert!(a.exists());
        assert!(b.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn cmd_plan_writes_artifact_and_lock_skips_copy() {
        let _serial = crate::test_support::serial_net();
        let lb =
            crate::test_support::start_pair(Some(crate::test_support::MOVIE_REL), b"abcdefghij")
                .await;
        let dir = crate::test_support::scratch("plan");
        let library = crate::test_support::library_root(&dir);
        let _store = crate::test_support::open_store(&dir).await;
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let json = cmd_plan(
            true,
            Some(dir.join("state.db")),
            Some(ds),
            Some(library.clone()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            None,
            Some(dir.join("plans")),
        )
        .await
        .expect("plan");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true);
        assert!(
            value["data"]["actions"]
                .as_array()
                .expect("actions")
                .iter()
                .any(|a| a["type"] == "copy"),
            "plan should copy schema listing: {}",
            value["data"]["actions"]
        );
        let path = value["data"]["path"].as_str().expect("path");
        assert!(std::path::Path::new(path).is_file());

        let locked = crate::test_support::write_ds(&dir, crate::test_support::DS_LOCKED);
        let json = cmd_plan(
            true,
            Some(dir.join("state.db")),
            Some(locked),
            Some(library),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            None,
            Some(dir.join("plans")),
        )
        .await
        .expect("locked plan");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        let actions = value["data"]["actions"].as_array().expect("actions");
        assert!(
            actions
                .iter()
                .any(|a| a["type"] == "skip" && a["reason"] == "lock"),
            "{actions:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn cmd_plan_emits_unmonitor_for_install_b3_and_wanted_missing() {
        let _serial = crate::test_support::serial_net();
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let lb = crate::test_support::start_pair_with_grab_ops(
            mediaops_core::Grabber::Servarr,
            Some(std::sync::Arc::new(HoldGrabOps {
                items: Vec::new(),
                wanted: vec![title.clone()],
            })),
        )
        .await;
        let dir = crate::test_support::scratch("plan-unmonitor");
        let library = crate::test_support::library_root(&dir);
        // The index row is the install proof, the file is the still-there proof:
        // Unmonitor needs both.
        let installed = library.join(crate::test_support::MOVIE_REL);
        std::fs::create_dir_all(installed.parent().expect("parent")).expect("dirs");
        std::fs::write(&installed, b"orig").expect("library file");
        let store = crate::test_support::open_store(&dir).await;
        store
            .record_install(
                &title,
                &mediaops_core::Blake3Hex::of_bytes(b"orig"),
                crate::test_support::MOVIE_REL,
            )
            .await
            .expect("index");
        let ds = crate::test_support::write_ds(
            &dir,
            "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 1\nmax_nvenc = 1\nlock = false\ngrabber = \"servarr\"\n",
        );
        let json = cmd_plan(
            true,
            Some(dir.join("state.db")),
            Some(ds),
            Some(library),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            None,
            Some(dir.join("plans")),
        )
        .await
        .expect("plan");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true, "{json}");
        let actions = value["data"]["actions"].as_array().expect("actions");
        assert!(
            actions.iter().any(|a| {
                a["type"] == "unmonitor" && a["title_id"] == "movie:key:thematrix.1999"
            }),
            "plan JSON must contain unmonitor for the installed title: {actions:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn cmd_run_copies_through_home_socket_and_installs() {
        let _serial = crate::test_support::serial_net();
        let lb =
            crate::test_support::start_pair(Some(crate::test_support::MOVIE_REL), b"abcdefghij")
                .await;
        let dir = crate::test_support::scratch("run");
        let library = crate::test_support::library_root(&dir);
        let store = crate::test_support::open_store(&dir).await;
        crate::test_support::seed_probe(&store, &lb.fingerprint).await;
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let json = cmd_run(
            &mediaops_ssh::TranscriptExec::new(),
            true,
            Some(dir.join("state.db")),
            Some(ds),
            Some(library.clone()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            None,
            Some(dir.join("plans")),
        )
        .await
        .expect("run");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true, "{json}");
        assert_eq!(value["data"]["copies"], 1);
        let installed = value["data"]["installed"].as_array().expect("installed");
        assert_eq!(installed.len(), 1);
        let path = installed[0].as_str().expect("path");
        assert!(std::path::Path::new(path).is_file(), "{path}");
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

    struct EncodeTranscript {
        stdout: String,
        write_converting: bool,
        fail_probe: bool,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl ExecPort for EncodeTranscript {
        async fn run(
            &self,
            command: &mediaops_core::ExecCommand,
        ) -> Result<mediaops_core::ExecOutput, mediaops_core::ExecError> {
            self.calls
                .lock()
                .expect("calls")
                .push(command.program.clone());
            if self.fail_probe && command.program_name() == "ffprobe" {
                return Err(mediaops_core::ExecError::Failed {
                    program: command.program.clone(),
                    message: "boom".into(),
                });
            }
            if command.program_name() == "ffmpeg" && self.write_converting {
                if let Some(out) = command.args.last() {
                    let path = std::path::PathBuf::from(out);
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).expect("mkdir");
                    }
                    std::fs::write(&path, b"encoded-h264").expect("converting");
                }
            }
            Ok(mediaops_core::ExecOutput {
                status: 0,
                stdout: self.stdout.as_bytes().to_vec(),
                stderr: Vec::new(),
            })
        }
    }

    #[tokio::test]
    async fn cmd_run_after_install_reports_encode_and_changes_current_b3() {
        let _serial = crate::test_support::serial_net();
        let lb =
            crate::test_support::start_pair(Some(crate::test_support::MOVIE_REL), b"abcdefghij")
                .await;
        let dir = crate::test_support::scratch("run-encode");
        let library = crate::test_support::library_root(&dir);
        let store = crate::test_support::open_store(&dir).await;
        crate::test_support::seed_probe(&store, &lb.fingerprint).await;
        store.put_machine("nvenc_cap", "1").await.expect("cap");
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let exec = EncodeTranscript {
            stdout: HEVC10.into(),
            write_converting: true,
            fail_probe: false,
            calls: std::sync::Mutex::new(Vec::new()),
        };
        let json = cmd_run(
            &exec,
            true,
            Some(dir.join("state.db")),
            Some(ds),
            Some(library.clone()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            None,
            Some(dir.join("plans")),
        )
        .await
        .expect("run");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true, "{json}");
        assert_eq!(value["data"]["encode"]["ran"], 1, "{json}");
        assert_eq!(value["data"]["encode"]["error"], serde_json::Value::Null);
        let title = store
            .get_title(&TitleId::movie_key("The.Matrix", 1999).expect("id"))
            .await
            .expect("title")
            .into_iter()
            .next()
            .expect("indexed");
        assert_ne!(
            title.current_b3().as_str(),
            title.install_b3().as_str(),
            "current_b3 must change after nvenc replace"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn cmd_run_after_install_ffprobe_error_fails_visibly() {
        let _serial = crate::test_support::serial_net();
        let lb =
            crate::test_support::start_pair(Some(crate::test_support::MOVIE_REL), b"abcdefghij")
                .await;
        let dir = crate::test_support::scratch("run-encode-probe-err");
        let library = crate::test_support::library_root(&dir);
        let store = crate::test_support::open_store(&dir).await;
        crate::test_support::seed_probe(&store, &lb.fingerprint).await;
        store.put_machine("nvenc_cap", "1").await.expect("cap");
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let exec = EncodeTranscript {
            stdout: HEVC10.into(),
            write_converting: false,
            fail_probe: true,
            calls: std::sync::Mutex::new(Vec::new()),
        };
        let json = cmd_run(
            &exec,
            true,
            Some(dir.join("state.db")),
            Some(ds),
            Some(library.clone()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            None,
            Some(dir.join("plans")),
        )
        .await
        .expect("the copy landed; a probe failure is reported, not fatal");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["data"]["copies"], 1, "{json}");
        assert!(
            value["data"]["encode"]["error"]
                .as_str()
                .is_some_and(|e| e.contains("probe_error")),
            "{json}"
        );
        assert!(
            library
                .join("movies/The.Matrix.(1999)/The.Matrix.(1999).mkv")
                .is_file(),
            "installed file stays installed"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn cmd_plan_approved_hold_copies_schema_path_not_scene_name() {
        let _serial = crate::test_support::serial_net();
        let scene = "The.Matrix.1999.REPACK.mkv";
        let mut item = mediaops_core::HoldLiveItem::new(
            mediaops_core::HoldKey::new(
                TitleId::movie_key("The.Matrix", 1999).expect("id"),
                mediaops_core::ReleaseId::parse("deadbeef").expect("id"),
            ),
            1,
            10,
            "blocked",
        );
        item.remote = Some(
            mediaops_core::RemoteRef::from_wire_parts(
                "seedbox".into(),
                std::path::PathBuf::from(scene),
            )
            .expect("ref"),
        );
        item.placement = Some(mediaops_core::Placement::movie("The.Matrix", 1999, "mkv"));
        let lb = crate::test_support::start_pair_with_grab_ops(
            mediaops_core::Grabber::Servarr,
            Some(std::sync::Arc::new(HoldGrabOps {
                items: vec![item.clone()],
                wanted: Vec::new(),
            })),
        )
        .await;
        std::fs::write(lb.remote_root.join(scene), b"abcdefghij").expect("scene file");
        let dir = crate::test_support::scratch("plan-hold");
        let library = crate::test_support::library_root(&dir);
        let store = crate::test_support::open_store(&dir).await;
        store
            .put_hold(&item.key, HoldDecision::Approved)
            .await
            .expect("put");
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let json = cmd_plan(
            true,
            Some(dir.join("state.db")),
            Some(ds),
            Some(library),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            None,
            Some(dir.join("plans")),
        )
        .await
        .expect("plan");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true, "{json}");
        let actions = value["data"]["actions"].as_array().expect("actions");
        let copy = actions
            .iter()
            .find(|a| a["type"] == "copy")
            .expect("copy action");
        assert_eq!(copy["placement"]["title"], "The.Matrix");
        assert_eq!(copy["placement"]["year"], 1999);
        assert_eq!(copy["placement"]["extension"], "mkv");
        let dest = mediaops_core::render(
            &TitleId::movie_key("The.Matrix", 1999).expect("id"),
            &mediaops_core::Placement::movie("The.Matrix", 1999, "mkv"),
        )
        .expect("schema");
        let dest = dest.to_str().expect("utf8");
        assert!(
            dest.contains("The.Matrix.(1999)") && !dest.contains("REPACK"),
            "Copy must land on PathSchema, not the scene name: {dest}"
        );
        assert_eq!(copy["remote"]["rel_path"], scene);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn cmd_run_approved_hold_installs_schema_path_not_scene_name() {
        let _serial = crate::test_support::serial_net();
        let scene = "The.Matrix.1999.REPACK.mkv";
        let mut item = mediaops_core::HoldLiveItem::new(
            mediaops_core::HoldKey::new(
                TitleId::movie_key("The.Matrix", 1999).expect("id"),
                mediaops_core::ReleaseId::parse("deadbeef").expect("id"),
            ),
            1,
            10,
            "blocked",
        );
        item.remote = Some(
            mediaops_core::RemoteRef::from_wire_parts(
                "seedbox".into(),
                std::path::PathBuf::from(scene),
            )
            .expect("ref"),
        );
        item.placement = Some(mediaops_core::Placement::movie("The.Matrix", 1999, "mkv"));
        let dir = crate::test_support::scratch("run-hold");
        let library = crate::test_support::library_root(&dir);
        let schema = mediaops_core::render(
            &TitleId::movie_key("The.Matrix", 1999).expect("id"),
            &mediaops_core::Placement::movie("The.Matrix", 1999, "mkv"),
        )
        .expect("schema");
        assert!(!library.join(&schema).exists(), "approve must not install");
        let lb = crate::test_support::start_pair_with_grab_ops(
            mediaops_core::Grabber::Servarr,
            Some(std::sync::Arc::new(HoldGrabOps {
                items: vec![item.clone()],
                wanted: Vec::new(),
            })),
        )
        .await;
        std::fs::write(lb.remote_root.join(scene), b"abcdefghij").expect("scene file");
        let store = crate::test_support::open_store(&dir).await;
        crate::test_support::seed_probe(&store, &lb.fingerprint).await;
        store.put_machine("nvenc_cap", "0").await.expect("cap");
        store
            .put_hold(&item.key, HoldDecision::Approved)
            .await
            .expect("put");
        assert!(
            !library.join(&schema).exists(),
            "Approved hold must not install until exclusive run"
        );
        let ds = crate::test_support::write_ds(&dir, crate::test_support::DS_UNLOCKED);
        let json = cmd_run(
            &mediaops_ssh::TranscriptExec::new(),
            true,
            Some(dir.join("state.db")),
            Some(ds),
            Some(library.clone()),
            Some(lb.sock.clone()),
            Some(lb.tls_dir.clone()),
            None,
            Some(dir.join("plans")),
        )
        .await
        .expect("run");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true, "{json}");
        let installed = value["data"]["installed"].as_array().expect("installed");
        assert_eq!(installed.len(), 1, "{json}");
        let path = installed[0].as_str().expect("path");
        assert!(
            path.contains("The.Matrix.(1999)") && !path.contains("REPACK"),
            "library path must be PathSchema without leftover scene tag: {path}"
        );
        assert!(!path.contains(scene), "{path}");
        assert!(std::path::Path::new(path).is_file(), "{path}");
        let _ = std::fs::remove_dir_all(dir);
    }

    struct HoldGrabOps {
        items: Vec<mediaops_core::HoldLiveItem>,
        wanted: Vec<mediaops_core::TitleId>,
    }

    impl mediaops_core::GrabOps for HoldGrabOps {
        fn grab_apply<'a>(
            &'a self,
            _: &'a mediaops_core::DesiredState,
        ) -> mediaops_core::BoxFuture<
            'a,
            Result<mediaops_core::GrabApplyReport, mediaops_core::ControlError>,
        > {
            Box::pin(async {
                Ok(mediaops_core::GrabApplyReport {
                    noop: true,
                    diff: String::new(),
                })
            })
        }
        fn key_discovery(
            &self,
        ) -> mediaops_core::BoxFuture<
            '_,
            Result<mediaops_core::KeyPresence, mediaops_core::ControlError>,
        > {
            Box::pin(async { Ok(mediaops_core::KeyPresence::default()) })
        }
        fn edge_api_check(
            &self,
        ) -> mediaops_core::BoxFuture<
            '_,
            Result<mediaops_core::EdgeApiReport, mediaops_core::ControlError>,
        > {
            Box::pin(async {
                Ok(mediaops_core::EdgeApiReport {
                    fingerprint: String::new(),
                    invariant_ok: true,
                    drift: String::new(),
                })
            })
        }
        fn edge_apply<'a>(
            &'a self,
            _: &'a mediaops_core::DesiredState,
        ) -> mediaops_core::BoxFuture<
            'a,
            Result<mediaops_core::GrabApplyReport, mediaops_core::ControlError>,
        > {
            Box::pin(async {
                Ok(mediaops_core::GrabApplyReport {
                    noop: true,
                    diff: String::new(),
                })
            })
        }
        fn hold_list(
            &self,
        ) -> mediaops_core::BoxFuture<
            '_,
            Result<Vec<mediaops_core::HoldLiveItem>, mediaops_core::ControlError>,
        > {
            let items = self.items.clone();
            Box::pin(async move { Ok(items) })
        }
        fn hold_reject<'a>(
            &'a self,
            _: &'a mediaops_core::HoldKey,
        ) -> mediaops_core::BoxFuture<'a, Result<(), mediaops_core::ControlError>> {
            Box::pin(async { Ok(()) })
        }
        fn wanted_missing(
            &self,
        ) -> mediaops_core::BoxFuture<
            '_,
            Result<Vec<mediaops_core::TitleId>, mediaops_core::ControlError>,
        > {
            let wanted = self.wanted.clone();
            Box::pin(async move { Ok(wanted) })
        }
        fn unmonitor<'a>(
            &'a self,
            _: &'a mediaops_core::TitleId,
        ) -> mediaops_core::BoxFuture<'a, Result<(), mediaops_core::ControlError>> {
            Box::pin(async { Ok(()) })
        }
        fn qbit_snapshot(
            &self,
        ) -> mediaops_core::BoxFuture<
            '_,
            Result<Vec<mediaops_core::GuardPreviewItem>, mediaops_core::ControlError>,
        > {
            Box::pin(async { Ok(Vec::new()) })
        }
    }
}
