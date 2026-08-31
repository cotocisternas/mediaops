use std::path::PathBuf;

use mediaops_core::{DesiredState, Envelope, ExecCommand, ExecPort};
use mediaops_encode::probe_nvenc;
use mediaops_ssh::SystemExec;
use mediaops_store::Store;
use mediaops_sync::{
    ensure_layout, media_server_warnings, refuse_below_watermark, systemd_exec_start, write_user_units,
};
use serde::Serialize;

use crate::bootstrap;
use crate::AppError;

#[derive(Debug, Serialize)]
struct BootstrapData {
    library_root: String,
    nvenc_cap: u32,
    dirs: Vec<String>,
    warnings: Vec<String>,
}

pub async fn bootstrap_library(
    json: bool,
    library_root: PathBuf,
    desired_state: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    state_db: Option<PathBuf>,
    enable_timer: bool,
    unit_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let desired_state =
        desired_state.unwrap_or_else(|| bootstrap::default_desired_state(&config_dir));
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let lock_path = state_db
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("mediaops.lock");
    let _lock = bootstrap::exclusive_lock(&lock_path).map_err(map_bootstrap)?;
    let ds_text =
        std::fs::read_to_string(&desired_state).map_err(|err| AppError::Runtime(err.into()))?;
    let ds = DesiredState::from_toml(&ds_text).map_err(|err| AppError::Runtime(anyhow_err(err)))?;

    let watermark_path = if library_root.exists() {
        library_root.clone()
    } else {
        library_root
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| library_root.clone())
    };
    refuse_below_watermark(&watermark_path, ds.min_free()).map_err(|err| match err {
        mediaops_sync::LibraryError::Watermark { .. } => AppError::Policy(err.to_string()),
        other => AppError::Runtime(anyhow_err(other)),
    })?;
    ensure_layout(&library_root).map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    let library_root = std::fs::canonicalize(&library_root)
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("canonicalize library-root: {err}")))?;

    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    store
        .put_machine("library_root", &library_root.display().to_string())
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;

    let nvenc = probe_nvenc(&SystemExec)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    store
        .put_machine("nvenc_cap", &nvenc.cap.to_string())
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    if !nvenc.ffmpeg_path.is_empty() {
        store
            .put_machine("ffmpeg_path", &nvenc.ffmpeg_path)
            .await
            .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    }

    let unit_dir = unit_dir.unwrap_or_else(bootstrap::default_unit_dir);
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("mediaops"));
    let state_db_arg = state_db.display().to_string();
    let exec_start = systemd_exec_start(&exe, &["--state-db", &state_db_arg, "run"]);
    write_user_units(&unit_dir, &exec_start).map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    if enable_timer {
        enable_user_timer(&SystemExec).await?;
    }

    let mut search = Vec::new();
    if let Some(home) = directories::BaseDirs::new() {
        search.push(home.config_dir().join("jellyfin"));
        search.push(home.config_dir().join("plex"));
        search.push(home.data_dir().join("jellyfin"));
    }
    let warnings = media_server_warnings(&search);
    for w in &warnings {
        tracing::warn!("{w}");
    }

    let data = BootstrapData {
        library_root: library_root.display().to_string(),
        nvenc_cap: nvenc.cap,
        dirs: mediaops_sync::SCHEMA_DIRS
            .iter()
            .map(|s| s.to_string())
            .collect(),
        warnings,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!(
            "library {} nvenc_cap {}",
            data.library_root, data.nvenc_cap
        ))
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

async fn enable_user_timer(exec: &impl ExecPort) -> Result<(), AppError> {
    let reload = ExecCommand::new(
        "systemctl",
        vec!["--user".into(), "daemon-reload".into()],
    );
    let enable = ExecCommand::new(
        "systemctl",
        vec![
            "--user".into(),
            "enable".into(),
            "--now".into(),
            "mediaops-run.timer".into(),
        ],
    );
    for cmd in [reload, enable] {
        let out = exec
            .run(&cmd)
            .await
            .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
        if out.status != 0 {
            return Err(AppError::Runtime(anyhow::anyhow!(
                "{} exited {}",
                cmd.program_name(),
                out.status
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{ExecError, ExecOutput};
    use std::sync::Mutex;

    struct FakeExec {
        calls: Mutex<Vec<(String, Vec<String>)>>,
        status: i32,
    }

    impl ExecPort for FakeExec {
        async fn run(&self, command: &ExecCommand) -> Result<ExecOutput, ExecError> {
            self.calls
                .lock()
                .expect("calls")
                .push((command.program.clone(), command.args.clone()));
            Ok(ExecOutput {
                status: self.status,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
    }

    #[tokio::test]
    async fn enable_timer_runs_systemctl_user_enable_now() {
        let fake = FakeExec {
            calls: Mutex::new(Vec::new()),
            status: 0,
        };
        enable_user_timer(&fake)
            .await
            .unwrap_or_else(|err| panic!("enable: {err}"));
        let calls = fake.calls.lock().expect("calls").clone();
        assert_eq!(calls[0].0, "systemctl");
        assert_eq!(calls[0].1, vec!["--user", "daemon-reload"]);
        assert_eq!(
            calls[1].1,
            vec!["--user", "enable", "--now", "mediaops-run.timer"]
        );
    }

    #[tokio::test]
    async fn enable_timer_fails_on_nonzero_status() {
        let fake = FakeExec {
            calls: Mutex::new(Vec::new()),
            status: 1,
        };
        let err = enable_user_timer(&fake)
            .await
            .err()
            .unwrap_or_else(|| panic!("expected error"));
        assert!(matches!(err, AppError::Runtime(_)));
    }
}
