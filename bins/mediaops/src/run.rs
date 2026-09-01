use std::fs::File;
use std::path::PathBuf;

use mediaops_core::{Action, DesiredState, Envelope, JobState, Plan, Probe, WantState, free_bytes};
use mediaops_store::Store;
use mediaops_sync::{ApplyCtx, ApplyError, PlanRequest, apply, plan_actions};
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
    plan: Plan,
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
    if copies == 0 && prepared.planned.first_candidate_breaches {
        return Err(AppError::Policy(
            "watermark/max_copy: first candidate alone would breach; refusing empty apply".into(),
        ));
    }

    let channel = connect_home(&prepared.socket, &prepared.tls_dir)
        .await
        .map_err(runtime_display)?;
    let n = configure_from_probes(&prepared.store, channel.clone()).await?;
    let active =
        std::fs::read(&prepared.desired_state).map_err(|err| AppError::Runtime(err.into()))?;
    let report = apply(
        &prepared.plan,
        &active,
        ApplyCtx {
            jobs: &prepared.store,
            titles: &prepared.store,
            source: grpc_source(channel),
            library_root: &prepared.library_root,
            concurrency: n as usize,
        },
    )
    .await
    .map_err(map_apply)?;
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
    let jobs = store.list_jobs().await.map_err(runtime_display)?;
    let open_wants: Vec<_> = jobs
        .into_iter()
        .filter(|j| matches!(j.state(), JobState::Want(WantState::Open)))
        .collect();
    let planned = plan_actions(PlanRequest {
        listings: &listings,
        title_index: &title_index,
        open_wants: &open_wants,
        desired: &ds,
        free_bytes: free,
    });
    let plan = Plan::from_toml_bytes(toml_bytes)
        .map_err(runtime_display)?
        .with_actions(planned.actions.clone());
    std::fs::create_dir_all(&plans_dir).map_err(|err| AppError::Runtime(err.into()))?;
    let name = format!(
        "{}-{}.json",
        bootstrap::utc_compact(),
        &plan.desired_state_b3().as_str()[..12]
    );
    let path = plans_dir.join(name);
    let json = serde_json::to_vec_pretty(&plan).map_err(|e| AppError::Runtime(e.into()))?;
    std::fs::write(&path, json).map_err(|err| AppError::Runtime(err.into()))?;
    Ok(PreparedPlan {
        _lock: lock,
        plan,
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
