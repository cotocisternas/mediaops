use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use mediaops_core::{
    Action, DesiredState, Envelope, JobState, Plan, Probe, TitleId, WantState, free_bytes,
};
use mediaops_ssh::SystemExec;
use mediaops_store::Store;
use mediaops_sync::{ApplyCtx, ApplyError, PlanRequest, apply, plan_actions, scan_schema_files};
use mediaops_transfer::{
    HomeChannel, configure_pool, connect_home, grpc_source, list_entries, pool_status, probe_range,
};
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;

#[derive(Debug, Serialize)]
struct PlanData {
    path: String,
    actions: Vec<Action>,
    first_candidate_breaches: bool,
}

#[derive(Debug, Serialize)]
struct RunData {
    path: String,
    copies: usize,
    skips: usize,
    installed: Vec<String>,
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
        Ok(format!("plan {} actions {}", data.path, data.actions.len()))
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn cmd_run(
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
    refuse_empty_apply(copies, prepared.planned.first_candidate_breaches)?;

    let channel = connect_home(&prepared.socket, &prepared.tls_dir)
        .await
        .map_err(runtime_display)?;
    let n = configure_from_probes(&prepared.store, channel.clone()).await?;
    let control = mediaops_proto::ControlPortClient::new(
        mediaops_proto::control_client::ControlClient::new(channel.clone()),
    );
    let active =
        std::fs::read(&prepared.desired_state).map_err(|err| AppError::Runtime(err.into()))?;
    let bytes = std::fs::read(&prepared.path).map_err(|err| AppError::Runtime(err.into()))?;
    let plan = Plan::from_json_slice(&bytes).map_err(runtime_display)?;
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
        },
    )
    .await
    .map_err(map_apply)?;

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
    for inst in &report.installed {
        if let Some(pull) = prepared
            .store
            .get_job(inst.pull_job_id)
            .await
            .map_err(runtime_display)?
        {
            let _ = crate::encode_cmd::after_install(
                &SystemExec,
                &prepared.store,
                &prepared.library_root,
                &inst.title_id,
                &inst.path,
                &pull,
                &ffmpeg,
                cap,
                paused,
            )
            .await;
        }
    }

    let _ = std::fs::remove_file(&prepared.path);
    let data = RunData {
        path: prepared.path.display().to_string(),
        copies: report.copies,
        skips: report.skips,
        installed: report
            .installed
            .iter()
            .map(|i| i.path.display().to_string())
            .collect(),
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!(
            "run copies {} skips {} installed {}",
            data.copies,
            data.skips,
            data.installed.len()
        ))
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
    let on_disk: Vec<TitleId> = scan_schema_files(&library_root)
        .map_err(runtime_display)?
        .into_iter()
        .map(|(id, _, _)| id)
        .collect();
    let jobs = store.list_jobs().await.map_err(runtime_display)?;
    let open_wants: Vec<_> = jobs
        .into_iter()
        .filter(|j| matches!(j.state(), JobState::Want(WantState::Open)))
        .collect();
    let planned = plan_actions(PlanRequest {
        listings: &listings,
        title_index: &title_index,
        on_disk: &on_disk,
        open_wants: &open_wants,
        desired: &ds,
        free_bytes: free,
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

async fn configure_from_probes(store: &Store, channel: HomeChannel) -> Result<u32, AppError> {
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
        AppError::DriftVerify(err.to_string())
    } else {
        AppError::Runtime(anyhow::anyhow!("{err}"))
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
            .get_title(&TitleId::movie("603").expect("id"))
            .await
            .expect("title")
            .expect("indexed");
        assert!(!title.path_missing());
        let _ = std::fs::remove_dir_all(dir);
    }
}
