use std::path::{Component, Path, PathBuf};

use mediaops_core::{DesiredState, Envelope, ExecCommand, ExecPort};
use mediaops_encode::probe_nvenc;
use mediaops_ssh::SystemExec;
use mediaops_store::Store;
use mediaops_sync::{
    ensure_layout, media_server_warnings, refuse_below_watermark, reindex_schema,
    systemd_exec_start, write_home_unit, write_user_units,
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
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let _lock =
        bootstrap::exclusive_lock(&bootstrap::lock_path(&state_db)).map_err(map_bootstrap)?;
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
    write_library_units(&unit_dir, &state_db, &config_dir, &desired_state)?;
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

#[derive(Debug, Serialize)]
struct RelocateData {
    library_root: String,
    dirs: Vec<String>,
    rewritten_absolute: u64,
}

pub async fn relocate_library(
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
    let lock_path = bootstrap::lock_path(&state_db);
    let _lock = bootstrap::exclusive_lock(&lock_path).map_err(map_bootstrap)?;
    let ds_text =
        std::fs::read_to_string(&desired_state).map_err(|err| AppError::Runtime(err.into()))?;
    let ds = DesiredState::from_toml(&ds_text).map_err(|err| AppError::Runtime(anyhow_err(err)))?;

    let library_root = layout_canonical_root(library_root, ds.min_free())?;

    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    let new_root = library_root.display().to_string();
    let rewritten_absolute = match store
        .get_machine("library_root")
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?
    {
        Some(old_root) => store
            .rewrite_absolute_prefix(&old_root, &new_root)
            .await
            .map_err(|err| AppError::Runtime(anyhow_err(err)))?,
        None => 0,
    };
    store
        .put_machine("library_root", &new_root)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;

    let unit_dir = unit_dir.unwrap_or_else(bootstrap::default_unit_dir);
    write_library_units(&unit_dir, &state_db, &config_dir, &desired_state)?;
    if enable_timer {
        enable_user_timer(&SystemExec).await?;
    }

    let data = RelocateData {
        library_root: new_root,
        dirs: mediaops_sync::SCHEMA_DIRS
            .iter()
            .map(|s| s.to_string())
            .collect(),
        rewritten_absolute,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!("library {}", data.library_root))
    }
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
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let lock_path = bootstrap::lock_path(&state_db);
    let _lock = bootstrap::exclusive_lock(&lock_path).map_err(map_bootstrap)?;
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
    let report = reindex_schema(&library_root, &store)
        .await
        .map_err(map_reindex)?;
    let data = ReindexData {
        indexed: report.indexed,
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!("reindex {}", data.indexed))
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

fn write_library_units(
    unit_dir: &Path,
    state_db: &std::path::Path,
    config_dir: &std::path::Path,
    desired_state: &std::path::Path,
) -> Result<(), AppError> {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("mediaops"));
    let state_db_arg = state_db.display().to_string();
    // `--state-db` is a `run` option, not a global one: it must follow the verb.
    let exec_start = systemd_exec_start(&exe, &["run", "--state-db", &state_db_arg]);
    write_user_units(unit_dir, &exec_start).map_err(|err| AppError::Runtime(anyhow_err(err)))?;

    let mut daemon = exe;
    daemon.set_file_name("mediaopsd");
    let tls_dir = bootstrap::default_tls_dir(config_dir);
    let socket = bootstrap::default_socket();
    let tls_arg = tls_dir.display().to_string();
    let ds_arg = desired_state.display().to_string();
    let sock_arg = socket.display().to_string();
    let home_exec = systemd_exec_start(
        &daemon,
        &[
            "serve",
            "--role",
            "home",
            "--tls-dir",
            &tls_arg,
            "--desired-state",
            &ds_arg,
            "--socket",
            &sock_arg,
        ],
    );
    write_home_unit(unit_dir, &home_exec).map_err(|err| AppError::Runtime(anyhow_err(err)))
}

fn anyhow_err(err: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}

fn map_reindex(err: mediaops_sync::ReindexError) -> AppError {
    AppError::Runtime(anyhow_err(err))
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
            "mediaopsd-home.service".into(),
        ],
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
    for cmd in [reload, enable_home, enable] {
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
            vec!["--user", "enable", "--now", "mediaopsd-home.service"]
        );
        assert_eq!(
            calls[2].1,
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

    const DS: &str = "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n";

    #[tokio::test]
    async fn relocate_rewrites_root_and_units_without_copying_media() {
        let dir = scratch("relocate");
        let ds = dir.join("desired-state.toml");
        std::fs::write(&ds, DS).expect("ds");
        let old = dir.join("old");
        mediaops_sync::ensure_layout(&old).expect("old layout");
        let rel = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
        let media = old.join(rel);
        std::fs::create_dir_all(media.parent().expect("parent")).expect("mkdir");
        std::fs::write(&media, b"orig").expect("media");
        let db = dir.join("state.db");
        let store = Store::open(&db).await.expect("store");
        let title = mediaops_core::TitleId::movie_key("The.Matrix", 1999).expect("title");
        let abs_title = mediaops_core::TitleId::movie_key("Other", 2000).expect("abs");
        let digest = mediaops_core::Blake3Hex::of_bytes(b"orig");
        let old_canon = std::fs::canonicalize(&old).expect("canon old");
        store
            .import_rows(&[
                mediaops_core::TitleIndexEntry::new(
                    title.clone(),
                    rel,
                    digest.clone(),
                    digest.clone(),
                ),
                mediaops_core::TitleIndexEntry::new(
                    abs_title.clone(),
                    old_canon.join(rel).display().to_string(),
                    digest.clone(),
                    digest,
                ),
            ])
            .await
            .expect("index rows");
        store
            .put_machine("library_root", &old_canon.display().to_string())
            .await
            .expect("old root");
        drop(store);
        let units = dir.join("units");
        let neu = dir.join("new");
        let json = relocate_library(
            true,
            neu.clone(),
            Some(ds),
            Some(dir.clone()),
            Some(db.clone()),
            false,
            Some(units.clone()),
        )
        .await
        .expect("relocate");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true, "{json}");
        let root = value["data"]["library_root"].as_str().expect("root");
        assert!(std::path::Path::new(root).is_absolute(), "{root}");
        for name in mediaops_sync::SCHEMA_DIRS {
            assert!(neu.join(name).is_dir(), "{name}");
        }
        assert!(media.is_file(), "relocate must not move media");
        assert!(!neu.join(rel).exists(), "relocate must not copy media");
        assert!(units.join("mediaops-run.service").is_file());
        assert!(units.join("mediaops-run.timer").is_file());
        assert!(units.join("mediaopsd-home.service").is_file());
        let timer = std::fs::read_to_string(units.join("mediaops-run.timer")).expect("timer");
        assert!(timer.contains("OnUnitInactiveSec="));
        assert!(!timer.contains("OnCalendar"));
        let store = Store::open(&db).await.expect("reopen");
        assert_eq!(
            store
                .get_machine("library_root")
                .await
                .expect("get")
                .as_deref(),
            Some(root)
        );
        let entry = store
            .get_title(&title)
            .await
            .expect("get")
            .into_iter()
            .next()
            .expect("row");
        assert_eq!(
            entry.path(),
            rel,
            "relative index path must stay schema-relative"
        );
        let abs_entry = store
            .get_title(&abs_title)
            .await
            .expect("get")
            .into_iter()
            .next()
            .expect("abs row");
        assert!(
            abs_entry.path().starts_with(root),
            "absolute path under old root must rewrite: {}",
            abs_entry.path()
        );
        assert!(
            !abs_entry
                .path()
                .starts_with(old_canon.to_str().expect("utf8")),
            "old absolute prefix must be gone: {}",
            abs_entry.path()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn relocate_watermark_does_not_write_store_or_units() {
        let dir = scratch("relocate-water");
        let ds = dir.join("desired-state.toml");
        std::fs::write(
            &ds,
            "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 999999999\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n",
        )
        .expect("ds");
        let db = dir.join("state.db");
        let store = Store::open(&db).await.expect("store");
        store
            .put_machine("library_root", "/old/lib")
            .await
            .expect("seed");
        drop(store);
        let units = dir.join("units");
        let neu = dir.join("new");
        let err = relocate_library(
            true,
            neu,
            Some(ds),
            Some(dir.clone()),
            Some(db.clone()),
            false,
            Some(units.clone()),
        )
        .await
        .expect_err("watermark");
        assert!(matches!(err, AppError::Policy(_)), "{err}");
        let store = Store::open(&db).await.expect("reopen");
        assert_eq!(
            store
                .get_machine("library_root")
                .await
                .expect("get")
                .as_deref(),
            Some("/old/lib")
        );
        assert!(!units.join("mediaops-run.service").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn relocate_refuses_filesystem_root_and_empty() {
        let dir = scratch("relocate-root");
        let ds = dir.join("desired-state.toml");
        std::fs::write(&ds, DS).expect("ds");
        let db = dir.join("state.db");
        let units = dir.join("units");
        for root in [PathBuf::from("/"), PathBuf::new()] {
            let err = relocate_library(
                true,
                root.clone(),
                Some(ds.clone()),
                Some(dir.clone()),
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
