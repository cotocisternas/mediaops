use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use mediaops_core::{Envelope, TitleIndexEntry, TitleIndexError};
use mediaops_ssh::refuse_git_work_tree;
use mediaops_store::{Store, StoreError};
use mediaops_sync::ensure_layout;
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;

const BUNDLE_DS: &str = "config.toml";
const BUNDLE_INDEX: &str = "title-index.json";
const BUNDLE_TLS: &str = "tls";

#[derive(Debug, Serialize)]
struct ExportData {
    out: String,
    titles: usize,
}

#[derive(Debug, Serialize)]
struct ImportData {
    config_dir: String,
    library_root: String,
    titles: usize,
    dirs: Vec<String>,
}

pub async fn export_machine(
    json: bool,
    out: PathBuf,
    config_dir: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    state_db: Option<PathBuf>,
) -> Result<String, AppError> {
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let desired_state =
        desired_state.unwrap_or_else(|| bootstrap::default_desired_state(&config_dir));
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let _lock =
        bootstrap::exclusive_lock(&bootstrap::lock_path(&state_db)).map_err(map_bootstrap)?;
    refuse_bundle_git(&out, "export")?;

    let ds_bytes = fs::read(&desired_state).map_err(|err| AppError::Runtime(err.into()))?;
    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    let rows = store
        .list_titles()
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    let index_json = serde_json::to_string(&rows).map_err(|err| AppError::Runtime(err.into()))?;

    fs::create_dir_all(&out).map_err(|err| AppError::Runtime(err.into()))?;
    fs::write(out.join(BUNDLE_DS), ds_bytes).map_err(|err| AppError::Runtime(err.into()))?;
    fs::write(out.join(BUNDLE_INDEX), index_json).map_err(|err| AppError::Runtime(err.into()))?;
    copy_tls_dir(&tls_dir, &out.join(BUNDLE_TLS))?;

    let data = ExportData {
        out: out.display().to_string(),
        titles: rows.len(),
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!(
            "new-machine export {} titles {}",
            data.out, data.titles
        ))
    }
}

pub async fn import_machine(
    json: bool,
    from: PathBuf,
    library_root: PathBuf,
    config_dir: Option<PathBuf>,
    desired_state: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    state_db: Option<PathBuf>,
) -> Result<String, AppError> {
    let config_dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let desired_state =
        desired_state.unwrap_or_else(|| bootstrap::default_desired_state(&config_dir));
    let tls_dir = tls_dir.unwrap_or_else(|| bootstrap::default_tls_dir(&config_dir));
    let state_db = state_db.unwrap_or_else(bootstrap::default_state_db);
    let _lock =
        bootstrap::exclusive_lock(&bootstrap::lock_path(&state_db)).map_err(map_bootstrap)?;
    refuse_import_git(&config_dir, &desired_state, &tls_dir)?;
    crate::library::refuse_library_root(&library_root)?;

    let ds_bytes = fs::read(from.join(BUNDLE_DS)).map_err(|err| AppError::Runtime(err.into()))?;
    let index_bytes =
        fs::read(from.join(BUNDLE_INDEX)).map_err(|err| AppError::Runtime(err.into()))?;
    let rows: Vec<TitleIndexEntry> =
        serde_json::from_slice(&index_bytes).map_err(|err| AppError::Runtime(err.into()))?;
    refuse_non_schema_relative(&rows)?;

    let store = Store::open(&state_db)
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    let existing = store
        .list_titles()
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    if !existing.is_empty() {
        return Err(map_store(StoreError::TitleIndex(TitleIndexError::NotEmpty)));
    }

    if let Some(parent) = desired_state.parent() {
        fs::create_dir_all(parent).map_err(|err| AppError::Runtime(err.into()))?;
    }
    fs::write(&desired_state, ds_bytes).map_err(|err| AppError::Runtime(err.into()))?;
    copy_tls_dir(&from.join(BUNDLE_TLS), &tls_dir)?;

    ensure_layout(&library_root).map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    let library_root = fs::canonicalize(&library_root)
        .map_err(|err| AppError::Runtime(anyhow::anyhow!("canonicalize library-root: {err}")))?;
    store
        .put_machine("library_root", &library_root.display().to_string())
        .await
        .map_err(|err| AppError::Runtime(anyhow_err(err)))?;
    store.import_rows(&rows).await.map_err(map_store)?;

    let data = ImportData {
        config_dir: config_dir.display().to_string(),
        library_root: library_root.display().to_string(),
        titles: rows.len(),
        dirs: mediaops_sync::SCHEMA_DIRS
            .iter()
            .map(|s| s.to_string())
            .collect(),
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(|e| AppError::Runtime(e.into()))
    } else {
        Ok(format!(
            "new-machine import {} titles {}",
            data.library_root, data.titles
        ))
    }
}

fn refuse_bundle_git(path: &Path, verb: &str) -> Result<(), AppError> {
    refuse_git_work_tree(path).map_err(|err| match err {
        mediaops_ssh::SshError::GitWorkTree(path) => {
            AppError::Policy(format!("refusing to {verb} into a git work tree: {path}"))
        }
        other => AppError::Runtime(anyhow_err(other)),
    })
}

fn refuse_import_git(
    config_dir: &Path,
    desired_state: &Path,
    tls_dir: &Path,
) -> Result<(), AppError> {
    refuse_bundle_git(config_dir, "import")?;
    refuse_bundle_git(desired_state, "import")?;
    if let Some(parent) = desired_state.parent() {
        refuse_bundle_git(parent, "import")?;
    }
    refuse_bundle_git(tls_dir, "import")
}

fn refuse_non_schema_relative(rows: &[TitleIndexEntry]) -> Result<(), AppError> {
    for row in rows {
        if row.path_missing() {
            continue;
        }
        let path = Path::new(row.path());
        if path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(AppError::Usage(format!(
                "title-index path must be schema-relative: {}",
                row.path()
            )));
        }
    }
    Ok(())
}

fn copy_tls_dir(src: &Path, dest: &Path) -> Result<(), AppError> {
    fs::create_dir_all(dest).map_err(|err| AppError::Runtime(err.into()))?;
    let mut keep: HashSet<OsString> = HashSet::new();
    if src.is_dir() {
        let reader = fs::read_dir(src).map_err(|err| AppError::Runtime(err.into()))?;
        for entry in reader {
            let entry = entry.map_err(|err| AppError::Runtime(err.into()))?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name();
            fs::copy(&path, dest.join(&name)).map_err(|err| AppError::Runtime(err.into()))?;
            keep.insert(name);
        }
    }
    let reader = fs::read_dir(dest).map_err(|err| AppError::Runtime(err.into()))?;
    for entry in reader {
        let entry = entry.map_err(|err| AppError::Runtime(err.into()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if !keep.contains(&entry.file_name()) {
            fs::remove_file(&path).map_err(|err| AppError::Runtime(err.into()))?;
        }
    }
    Ok(())
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

fn map_store(err: StoreError) -> AppError {
    match err {
        StoreError::TitleIndex(TitleIndexError::NotEmpty) => AppError::Usage(err.to_string()),
        other => AppError::Runtime(anyhow_err(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{Blake3Hex, TitleId};

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-nm-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    const DS: &[u8] = b"schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n";

    #[tokio::test]
    async fn export_import_restores_both_digests_without_media() {
        let src = scratch("export-src");
        let ds = src.join(BUNDLE_DS);
        fs::write(&ds, DS).expect("ds");
        let tls = src.join(BUNDLE_TLS);
        fs::create_dir_all(&tls).expect("tls");
        fs::write(tls.join("ca.pem"), b"ca").expect("pem");
        fs::write(tls.join("client.key"), b"key").expect("key");
        let db = src.join("state.db");
        let store = Store::open(&db).await.expect("store");
        let title = TitleId::movie_key("The.Matrix", 1999).expect("title");
        let install = Blake3Hex::of_bytes(b"install");
        let current = Blake3Hex::of_bytes(b"current");
        let path = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
        store
            .import_rows(&[TitleIndexEntry::new(
                title.clone(),
                path,
                install.clone(),
                current.clone(),
            )])
            .await
            .expect("seed");
        drop(store);

        let bundle = scratch("bundle");
        export_machine(
            true,
            bundle.clone(),
            Some(src.clone()),
            Some(ds),
            Some(tls),
            Some(db),
        )
        .await
        .expect("export");
        assert!(bundle.join(BUNDLE_DS).is_file());
        assert!(bundle.join(BUNDLE_INDEX).is_file());
        assert!(bundle.join(BUNDLE_TLS).join("ca.pem").is_file());
        assert!(bundle.join(BUNDLE_TLS).join("client.key").is_file());

        let dest = scratch("import-dest");
        let dest_db = dest.join("state.db");
        let lib = dest.join("library");
        fs::create_dir_all(dest.join(BUNDLE_TLS)).expect("dest tls");
        fs::write(dest.join(BUNDLE_TLS).join("stale.key"), b"old").expect("stale");
        let json = import_machine(
            true,
            bundle,
            lib.clone(),
            Some(dest.clone()),
            None,
            None,
            Some(dest_db.clone()),
        )
        .await
        .expect("import");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["ok"], true, "{json}");
        assert_eq!(value["data"]["titles"], 1);
        assert_eq!(fs::read(dest.join(BUNDLE_DS)).expect("ds"), DS);
        assert_eq!(
            fs::read(dest.join(BUNDLE_TLS).join("ca.pem")).expect("pem"),
            b"ca"
        );
        assert_eq!(
            fs::read(dest.join(BUNDLE_TLS).join("client.key")).expect("key"),
            b"key"
        );
        assert!(
            !dest.join(BUNDLE_TLS).join("stale.key").exists(),
            "tls dest must replace, not merge"
        );
        for name in mediaops_sync::SCHEMA_DIRS {
            assert!(lib.join(name).is_dir(), "{name}");
        }
        assert!(
            !lib.join(path).exists(),
            "layout must exist before any media"
        );
        let store = Store::open(&dest_db).await.expect("dest store");
        let entry = store
            .get_title(&title)
            .await
            .expect("get")
            .into_iter()
            .next()
            .expect("row");
        assert_eq!(entry.install_b3(), &install);
        assert_eq!(entry.current_b3(), &current);
        assert_ne!(entry.install_b3(), entry.current_b3());
        let canon = fs::canonicalize(&lib).expect("canon lib");
        assert_eq!(
            store
                .get_machine("library_root")
                .await
                .expect("machine")
                .as_deref(),
            Some(canon.to_str().expect("utf8"))
        );
        let _ = fs::remove_dir_all(src);
        let _ = fs::remove_dir_all(dest);
    }

    #[tokio::test]
    async fn import_git_work_tree_writes_nothing() {
        let bundle = scratch("git-bundle");
        fs::write(bundle.join(BUNDLE_DS), DS).expect("ds");
        fs::write(bundle.join(BUNDLE_INDEX), b"[]").expect("index");
        fs::create_dir_all(bundle.join(BUNDLE_TLS)).expect("tls");
        fs::write(bundle.join(BUNDLE_TLS).join("ca.pem"), b"ca").expect("pem");

        let dest = scratch("git-dest");
        fs::create_dir_all(dest.join(".git")).expect("git");
        let dest_db = dest.join("state.db");
        let err = import_machine(
            true,
            bundle.clone(),
            dest.join("library"),
            Some(dest.clone()),
            None,
            None,
            Some(dest_db.clone()),
        )
        .await
        .expect_err("git");
        assert!(matches!(err, AppError::Policy(_)), "{err}");
        assert!(!dest.join(BUNDLE_DS).exists());
        assert!(!dest.join(BUNDLE_TLS).join("ca.pem").exists());
        let _ = fs::remove_dir_all(bundle);
        let _ = fs::remove_dir_all(dest);
    }

    #[tokio::test]
    async fn import_non_empty_title_index_does_not_clobber() {
        let bundle = scratch("clobber-bundle");
        fs::write(bundle.join(BUNDLE_DS), DS).expect("ds");
        let title = TitleId::movie_key("The.Matrix", 1999).expect("title");
        let incoming = TitleIndexEntry::new(
            title.clone(),
            "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv",
            Blake3Hex::of_bytes(b"new"),
            Blake3Hex::of_bytes(b"new"),
        );
        fs::write(
            bundle.join(BUNDLE_INDEX),
            serde_json::to_vec(&[incoming]).expect("json"),
        )
        .expect("index");
        fs::create_dir_all(bundle.join(BUNDLE_TLS)).expect("tls");

        let dest = scratch("clobber-dest");
        let dest_db = dest.join("state.db");
        let store = Store::open(&dest_db).await.expect("store");
        let existing = Blake3Hex::of_bytes(b"keep");
        store
            .import_rows(&[TitleIndexEntry::new(
                title.clone(),
                "movies/Keep.(1999)/Keep.(1999).mkv",
                existing.clone(),
                existing.clone(),
            )])
            .await
            .expect("seed");
        drop(store);

        let err = import_machine(
            true,
            bundle.clone(),
            dest.join("library"),
            Some(dest.clone()),
            None,
            None,
            Some(dest_db.clone()),
        )
        .await
        .expect_err("non-empty");
        assert!(matches!(err, AppError::Usage(_)), "{err}");
        assert!(!dest.join(BUNDLE_DS).exists(), "must not clobber dest DS");
        assert!(!dest.join(BUNDLE_TLS).join("ca.pem").exists());
        let store = Store::open(&dest_db).await.expect("reopen");
        let entry = store
            .get_title(&title)
            .await
            .expect("get")
            .into_iter()
            .next()
            .expect("row");
        assert_eq!(entry.install_b3(), &existing);
        assert_eq!(entry.path(), "movies/Keep.(1999)/Keep.(1999).mkv");
        let _ = fs::remove_dir_all(bundle);
        let _ = fs::remove_dir_all(dest);
    }

    #[tokio::test]
    async fn import_retries_when_dest_layout_exists_and_index_is_empty() {
        let bundle = scratch("retry-bundle");
        fs::write(bundle.join(BUNDLE_DS), DS).expect("ds");
        let title = TitleId::movie_key("The.Matrix", 1999).expect("title");
        let digest = Blake3Hex::of_bytes(b"orig");
        let path = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
        fs::write(
            bundle.join(BUNDLE_INDEX),
            serde_json::to_vec(&[TitleIndexEntry::new(
                title.clone(),
                path,
                digest.clone(),
                digest.clone(),
            )])
            .expect("json"),
        )
        .expect("index");
        fs::create_dir_all(bundle.join(BUNDLE_TLS)).expect("tls");
        fs::write(bundle.join(BUNDLE_TLS).join("ca.pem"), b"ca").expect("pem");
        fs::write(bundle.join(BUNDLE_TLS).join("client.key"), b"key").expect("key");

        let dest = scratch("retry-dest");
        fs::write(dest.join(BUNDLE_DS), b"stale").expect("old ds");
        ensure_layout(&dest.join("library")).expect("layout");
        let dest_db = dest.join("state.db");
        let _ = Store::open(&dest_db).await.expect("empty index");

        import_machine(
            true,
            bundle.clone(),
            dest.join("library"),
            Some(dest.clone()),
            None,
            None,
            Some(dest_db.clone()),
        )
        .await
        .expect("retry import");
        let store = Store::open(&dest_db).await.expect("reopen");
        let entry = store
            .get_title(&title)
            .await
            .expect("get")
            .into_iter()
            .next()
            .expect("row");
        assert_eq!(entry.install_b3(), &digest);
        assert_eq!(fs::read(dest.join(BUNDLE_DS)).expect("ds"), DS);
        let _ = fs::remove_dir_all(bundle);
        let _ = fs::remove_dir_all(dest);
    }

    #[tokio::test]
    async fn import_tls_dir_git_work_tree_writes_no_pem() {
        let bundle = scratch("tls-git-bundle");
        fs::write(bundle.join(BUNDLE_DS), DS).expect("ds");
        fs::write(bundle.join(BUNDLE_INDEX), b"[]").expect("index");
        fs::create_dir_all(bundle.join(BUNDLE_TLS)).expect("tls");
        fs::write(bundle.join(BUNDLE_TLS).join("ca.pem"), b"ca").expect("pem");

        let dest = scratch("tls-git-dest");
        let git_root = scratch("tls-git-tree");
        fs::create_dir_all(git_root.join(".git")).expect("git");
        let tls_dir = git_root.join("tls");
        fs::create_dir_all(&tls_dir).expect("tls dest");
        let err = import_machine(
            true,
            bundle.clone(),
            dest.join("library"),
            Some(dest.clone()),
            None,
            Some(tls_dir.clone()),
            Some(dest.join("state.db")),
        )
        .await
        .expect_err("git tls");
        assert!(matches!(err, AppError::Policy(_)), "{err}");
        assert!(!tls_dir.join("ca.pem").exists());
        assert!(!dest.join(BUNDLE_DS).exists());
        let _ = fs::remove_dir_all(bundle);
        let _ = fs::remove_dir_all(dest);
        let _ = fs::remove_dir_all(git_root);
    }

    #[tokio::test]
    async fn import_absolute_title_index_path_is_usage_and_writes_no_row() {
        let bundle = scratch("abs-bundle");
        fs::write(bundle.join(BUNDLE_DS), DS).expect("ds");
        let title = TitleId::movie_key("The.Matrix", 1999).expect("title");
        let digest = Blake3Hex::of_bytes(b"x");
        fs::write(
            bundle.join(BUNDLE_INDEX),
            serde_json::to_vec(&[TitleIndexEntry::new(
                title.clone(),
                "/data/old/movies/The.Matrix.(1999)/The.Matrix.(1999).mkv",
                digest.clone(),
                digest,
            )])
            .expect("json"),
        )
        .expect("index");
        fs::create_dir_all(bundle.join(BUNDLE_TLS)).expect("tls");

        let dest = scratch("abs-dest");
        let dest_db = dest.join("state.db");
        let err = import_machine(
            true,
            bundle.clone(),
            dest.join("library"),
            Some(dest.clone()),
            None,
            None,
            Some(dest_db.clone()),
        )
        .await
        .expect_err("absolute");
        assert!(matches!(err, AppError::Usage(_)), "{err}");
        if dest_db.exists() {
            let store = Store::open(&dest_db).await.expect("open");
            assert!(store.get_title(&title).await.expect("get").is_empty());
            assert!(store.list_titles().await.expect("list").is_empty());
        }
        let _ = fs::remove_dir_all(bundle);
        let _ = fs::remove_dir_all(dest);
    }

    #[tokio::test]
    async fn import_parent_dir_title_index_path_is_usage() {
        let bundle = scratch("dotdot-bundle");
        fs::write(bundle.join(BUNDLE_DS), DS).expect("ds");
        let title = TitleId::movie_key("The.Matrix", 1999).expect("title");
        let digest = Blake3Hex::of_bytes(b"x");
        fs::write(
            bundle.join(BUNDLE_INDEX),
            serde_json::to_vec(&[TitleIndexEntry::new(
                title,
                "movies/../etc/passwd",
                digest.clone(),
                digest,
            )])
            .expect("json"),
        )
        .expect("index");
        fs::create_dir_all(bundle.join(BUNDLE_TLS)).expect("tls");
        let dest = scratch("dotdot-dest");
        let err = import_machine(
            true,
            bundle.clone(),
            dest.join("library"),
            Some(dest.clone()),
            None,
            None,
            Some(dest.join("state.db")),
        )
        .await
        .expect_err("dotdot");
        assert!(matches!(err, AppError::Usage(_)), "{err}");
        let _ = fs::remove_dir_all(bundle);
        let _ = fs::remove_dir_all(dest);
    }
}
