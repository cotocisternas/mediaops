use std::path::PathBuf;

use crate::bootstrap;
use crate::AppError;

pub fn run_stub(state_db: Option<PathBuf>) -> Result<String, AppError> {
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let lock_path = state_db
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("mediaops.lock");
    let _lock = bootstrap::exclusive_lock(&lock_path).map_err(map_bootstrap)?;
    Err(AppError::Policy(
        "plan/apply waits for Epic 4; timer must not look successful".into(),
    ))
}

fn map_bootstrap(err: bootstrap::BootstrapError) -> AppError {
    match err.exit_code() {
        mediaops_core::ExitCode::Usage => AppError::Usage(err.to_string()),
        mediaops_core::ExitCode::PolicyRefusal => AppError::Policy(err.to_string()),
        mediaops_core::ExitCode::LockConflict => AppError::LockConflict(err.to_string()),
        _ => AppError::Runtime(anyhow::anyhow!("{err}")),
    }
}
