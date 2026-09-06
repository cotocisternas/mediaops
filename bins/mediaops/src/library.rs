use std::path::{Component, Path, PathBuf};

use mediaops_core::{DesiredState, Envelope, ExecCommand, ExecPort};
use mediaops_encode::probe_nvenc;
use mediaops_ssh::SystemExec;
use mediaops_store::Store;
use mediaops_sync::{
    ensure_layout, media_server_warnings, refuse_below_watermark, systemd_exec_start,
    write_home_unit,
};
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;

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
    let state_db = crate::home_library::state_db_path(state_db);
    let _lock =
        bootstrap::exclusive_lock(&bootstrap::lock_path(&state_db)).map_err(map_bootstrap)?;
    if !enable_timer {
        crate::home_library::connect().await.map_err(|err| {
            AppError::Runtime(anyhow::anyhow!("Home API must be running for library bootstrap; use --enable-service to start mediaops-home: {err}"))
        })?;
    }
    let ds_text =
        std::fs::read_to_string(&desired_state).map_err(|err| AppError::Runtime(err.into()))?;
    let ds = DesiredState::from_toml(&ds_text).map_err(|err| AppError::Runtime(anyhow_err(err)))?;

    let library_root = layout_canonical_root(library_root, ds.min_free())?;

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
    write_library_units(&unit_dir)?;
    if enable_timer {
        enable_user_timer(&SystemExec).await?;
    }
    {
        let encode_pause = store
            .get_machine("encode_pause")
            .await
            .map_err(crate::home_library::error)?
            .as_deref()
            == Some("1");
        bootstrap_home(&ds, &library_root, enable_timer, encode_pause).await.map_err(|err| {
            AppError::Runtime(anyhow::anyhow!("library layout and service unit are prepared, but Home state publication failed: {err}; after restoring API availability, rerun library bootstrap{}", if enable_timer { " --enable-service" } else { "" }))
        })?;
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

#[derive(Debug, Serialize)]
struct RelocateData {
    library_root: String,
    dirs: Vec<String>,
    rewritten_absolute: u64,
}

pub async fn relocate_library(
    json: bool,
    library_root: PathBuf,
    state_db: Option<PathBuf>,
    enable_timer: bool,
    unit_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    refuse_library_root(&library_root)?;
    relocate_home(json, library_root, state_db, enable_timer, unit_dir).await
}

#[derive(Debug, Serialize)]
struct ReindexData {
    indexed: usize,
}

pub async fn reindex_library(
    json: bool,
    library_root: Option<PathBuf>,
    state_db: Option<PathBuf>,
) -> Result<String, AppError> {
    let mut progress = crate::progress::OperationProgress::new(!json, "reindex");
    progress.stage("connecting to Home API", "");
    let state_db = crate::home_library::state_db_path(state_db);
    let _lock =
        bootstrap::exclusive_lock(&bootstrap::lock_path(&state_db)).map_err(map_bootstrap)?;
    let mut home = crate::home_library::HomeLibrary::load().await?;
    let root = home.root(library_root)?;
    progress.stage("pausing scheduling", root.display().to_string());
    let was_locked = home.begin_maintenance().await?;
    let indexed = home.reindex(&progress).await.map_err(maintenance_error)?;
    progress.stage("restoring scheduling", "");
    home.finish_maintenance(was_locked).await?;
    progress.finish();
    if json {
        serde_json::to_string(&Envelope::ok(ReindexData { indexed }))
            .map_err(|err| AppError::Runtime(err.into()))
    } else {
        Ok(format_reindex(indexed, &root, progress.elapsed()))
    }
}

fn maintenance_error(err: AppError) -> AppError {
    crate::home_library::maintenance_failure(err)
}

fn format_reindex(indexed: usize, root: &Path, elapsed: std::time::Duration) -> String {
    let noun = if indexed == 1 { "file" } else { "files" };
    let root: String = root
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut result = format!(
        "indexed   {indexed} {noun}\nlibrary   {root}\nelapsed   {}",
        crate::progress::duration(elapsed)
    );
    if indexed == 0 {
        result.push_str("\n          no schema media files under movies/, series/, or music/");
    }
    result
}

async fn bootstrap_home(
    ds: &DesiredState,
    root: &Path,
    wait: bool,
    encode_pause: bool,
) -> Result<(), AppError> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let api = loop {
        match crate::home_library::connect().await {
            Ok(api) => break api,
            Err(_) if wait && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            Err(err) => return Err(err),
        }
    };
    let mut cluster = match api
        .get(mediaops_core::Kind::Cluster, mediaops_core::CLUSTER_NAME)
        .await
    {
        Ok(cluster) => cluster,
        Err(err) if err.is_not_found() => {
            let mut cluster = crate::home_library::cluster_from_config(ds, root);
            if let mediaops_core::Spec::Cluster(spec) = &mut cluster.spec {
                spec.encode_pause = encode_pause;
            }
            cluster
        }
        Err(err) => return Err(crate::home_library::error(err)),
    };
    if let mediaops_core::Spec::Cluster(spec) = &mut cluster.spec {
        if !spec.library_root.is_empty() && Path::new(&spec.library_root) != root {
            return Err(AppError::Usage(
                "Cluster already has another library root; use library relocate".into(),
            ));
        }
        spec.library_root = root.display().to_string();
    }
    crate::home_library::apply_spec(&api, cluster).await?;
    if let Some(address) = ds.seedbox_address() {
        match api
            .get(mediaops_core::Kind::Secret, mediaops_core::SECRET_NAME)
            .await
        {
            Ok(_) => {}
            Err(err) if err.is_not_found() => {
                let mut secret = mediaops_core::SecretSpec {
                    seedbox_address: address.to_owned(),
                    ..Default::default()
                };
                if let Some(tls) = ds.tls() {
                    secret.ca_sha256 = tls.ca_sha256.clone();
                    secret.server_sha256 = tls.server_sha256.clone();
                    secret.client_sha256 = tls.client_sha256.clone();
                }
                api.apply(mediaops_core::HomeObject::new(
                    mediaops_core::Kind::Secret,
                    mediaops_core::SECRET_NAME,
                    mediaops_core::Spec::Secret(secret),
                    mediaops_core::StatusBody::Secret,
                ))
                .await
                .map_err(crate::home_library::error)?;
            }
            Err(err) => return Err(crate::home_library::error(err)),
        }
    }
    Ok(())
}

async fn relocate_home(
    json: bool,
    root: PathBuf,
    state_db: Option<PathBuf>,
    enable_timer: bool,
    unit_dir: Option<PathBuf>,
) -> Result<String, AppError> {
    let state_db = crate::home_library::state_db_path(state_db);
    let _lock =
        bootstrap::exclusive_lock(&bootstrap::lock_path(&state_db)).map_err(map_bootstrap)?;
    let mut home = crate::home_library::HomeLibrary::load().await?;
    let root = layout_canonical_root(root, home.spec()?.min_free)?;
    let was_locked = home.begin_maintenance().await?;
    if let mediaops_core::Spec::Cluster(spec) = &mut home.cluster.spec {
        spec.library_root = root.display().to_string();
    }
    home.cluster = home
        .api
        .patch(home.cluster.clone(), "spec")
        .await
        .map_err(crate::home_library::error)
        .map_err(maintenance_error)?;
    write_library_units(&unit_dir.unwrap_or_else(bootstrap::default_unit_dir))
        .map_err(maintenance_error)?;
    if enable_timer {
        enable_user_timer(&SystemExec)
            .await
            .map_err(maintenance_error)?;
    }
    home.finish_maintenance(was_locked).await?;
    let data = RelocateData {
        library_root: root.display().to_string(),
        dirs: mediaops_sync::SCHEMA_DIRS
            .iter()
            .map(|name| name.to_string())
            .collect(),
        rewritten_absolute: 0,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|err| AppError::Runtime(err.into()))
    } else {
        Ok(format!("library {}", data.library_root))
    }
}

pub(crate) fn refuse_library_root(root: &Path) -> Result<(), AppError> {
    if is_forbidden_library_root(root) {
        return Err(AppError::Usage(format!(
            "refusing library-root `{}`",
            root.display()
        )));
    }
    Ok(())
}

fn is_forbidden_library_root(root: &Path) -> bool {
    if root.as_os_str().is_empty() {
        return true;
    }
    let mut only_root = false;
    for c in root.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => only_root = true,
            Component::CurDir => {}
            _ => return false,
        }
    }
    only_root
}

fn layout_canonical_root(
    library_root: PathBuf,
    min_free: mediaops_core::Bytes,
) -> Result<PathBuf, AppError> {
    refuse_library_root(&library_root)?;
    let watermark_path = if library_root.exists() {
        library_root.clone()
    } else {
        library_root
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| library_root.clone())
    };
    refuse_below_watermark(&watermark_path, min_free).map_err(|err| match err {
        mediaops_sync::LibraryError::Watermark { .. } => AppError::Policy(err.to_string()),
        other => AppError::Runtime(anyhow_err(other)),
    })?;
    ensure_layout(&library_root).map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    std::fs::canonicalize(&library_root)
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("canonicalize library-root: {err}")))
}

/// Units this layout no longer writes. Left in place on an upgraded box they
/// stay enabled and fail: the timer fires a `run` verb that no longer exists,
/// and the old home unit execs `mediaopsd serve --role home`, which now
/// refuses.
const RETIRED_UNITS: &[&str] = &[
    "mediaops-run.service",
    "mediaops-run.timer",
    "mediaopsd-home.service",
];

fn write_library_units(unit_dir: &Path) -> Result<(), AppError> {
    for name in RETIRED_UNITS {
        let path = unit_dir.join(name);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|err| AppError::Runtime(err.into()))?;
        }
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("mediaops"));
    let mut supervisor = exe;
    supervisor.set_file_name("mediaops-home");
    let home_exec = systemd_exec_start(&supervisor, &[]);
    write_home_unit(unit_dir, &home_exec).map_err(|err| AppError::Runtime(anyhow_err(err)))
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
    let reload = ExecCommand::new("systemctl", vec!["--user".into(), "daemon-reload".into()]);
    let enable_home = ExecCommand::new(
        "systemctl",
        vec![
            "--user".into(),
            "enable".into(),
            "--now".into(),
            "mediaops-home.service".into(),
        ],
    );
    for cmd in [reload, enable_home] {
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
            vec!["--user", "enable", "--now", "mediaops-home.service"]
        );
        assert_eq!(calls.len(), 2);
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

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-lib-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[tokio::test]
    async fn relocation_watermark_refuses_before_layout_changes() {
        let dir = scratch("relocate-water");
        let root = dir.join("new");
        let err = layout_canonical_root(root.clone(), mediaops_core::Bytes::new(u64::MAX))
            .expect_err("watermark");
        assert!(matches!(err, AppError::Policy(_)), "{err}");
        assert!(!root.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reindex_exact_screen_explains_file_count_root_and_empty_schema() {
        assert_eq!(
            format_reindex(2, Path::new("/library"), std::time::Duration::from_secs(65)),
            "indexed   2 files\nlibrary   /library\nelapsed   1m 05s"
        );
        assert_eq!(
            format_reindex(0, Path::new("/library"), std::time::Duration::ZERO),
            "indexed   0 files\nlibrary   /library\nelapsed   0s\n          no schema media files under movies/, series/, or music/"
        );
    }

    #[tokio::test]
    async fn relocate_refuses_filesystem_root_and_empty() {
        let dir = scratch("relocate-root");
        let db = dir.join("state.db");
        let units = dir.join("units");
        for root in [PathBuf::from("/"), PathBuf::new()] {
            let err = relocate_library(
                true,
                root.clone(),
                Some(db.clone()),
                false,
                Some(units.clone()),
            )
            .await
            .expect_err("refuse");
            assert!(matches!(err, AppError::Usage(_)), "root={root:?} err={err}");
        }
        assert!(!units.join("mediaops-run.service").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
