use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use mediaops_core::{
    Envelope, Kind, Spec, StatusBody, TitleFileStatus, TitleId, TitleIndexEntry, TitleSpec,
    TitleStatus,
};
use mediaops_ssh::refuse_git_work_tree;
use mediaops_sync::ensure_layout;
use serde::Serialize;

use crate::AppError;
use crate::bootstrap;

const BUNDLE_DS: &str = "config.toml";
const BUNDLE_INDEX: &str = "title-index.json";
const BUNDLE_TLS: &str = "tls";
const BUNDLE_CLUSTER: &str = "cluster.json";
const BUNDLE_SECRET: &str = "secret.json";

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
    let state_db = crate::home_library::state_db_path(state_db);
    let _lock =
        bootstrap::exclusive_lock(&bootstrap::lock_path(&state_db)).map_err(map_bootstrap)?;
    refuse_bundle_git(&out, "export")?;

    let ds_bytes = fs::read(&desired_state).map_err(|err| AppError::Runtime(err.into()))?;
    let home = crate::home_library::HomeLibrary::load().await?;
    let rows = home.rows(true).await?;
    let secret = match home
        .api
        .get(mediaops_core::Kind::Secret, mediaops_core::SECRET_NAME)
        .await
    {
        Ok(secret) => Some(secret),
        Err(err) if err.is_not_found() => None,
        Err(err) => return Err(crate::home_library::error(err)),
    };
    let index_json = serde_json::to_string(&rows).map_err(|err| AppError::Runtime(err.into()))?;

    private_directory(&out)?;
    write_private(&out.join(BUNDLE_DS), &ds_bytes)?;
    write_private(&out.join(BUNDLE_INDEX), index_json.as_bytes())?;
    write_private(
        &out.join(BUNDLE_CLUSTER),
        &serde_json::to_vec_pretty(&home.cluster).map_err(crate::home_library::error)?,
    )?;
    if let Some(secret) = secret {
        write_private(
            &out.join(BUNDLE_SECRET),
            &serde_json::to_vec_pretty(&secret).map_err(crate::home_library::error)?,
        )?;
    } else if out.join(BUNDLE_SECRET).exists() {
        fs::remove_file(out.join(BUNDLE_SECRET)).map_err(crate::home_library::error)?;
    }
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
    let state_db = crate::home_library::state_db_path(state_db);
    let _lock =
        bootstrap::exclusive_lock(&bootstrap::lock_path(&state_db)).map_err(map_bootstrap)?;
    refuse_import_git(&config_dir, &desired_state, &tls_dir)?;
    crate::library::refuse_library_root(&library_root)?;

    let ds_bytes = fs::read(from.join(BUNDLE_DS)).map_err(|err| AppError::Runtime(err.into()))?;
    mediaops_core::DesiredState::from_toml_bytes(&ds_bytes).map_err(crate::home_library::error)?;
    let index_bytes =
        fs::read(from.join(BUNDLE_INDEX)).map_err(|err| AppError::Runtime(err.into()))?;
    let rows: Vec<TitleIndexEntry> =
        serde_json::from_slice(&index_bytes).map_err(|err| AppError::Runtime(err.into()))?;
    refuse_non_schema_relative(&rows)?;

    import_home(
        json,
        &from,
        library_root,
        &config_dir,
        &desired_state,
        &tls_dir,
        &ds_bytes,
        &rows,
    )
    .await
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
    if !src.is_dir() {
        return Err(AppError::Usage(format!(
            "TLS source directory is missing: {}",
            src.display()
        )));
    }
    private_directory(dest)?;
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
            if entry
                .file_type()
                .map_err(crate::home_library::error)?
                .is_symlink()
            {
                return Err(AppError::Policy(format!(
                    "refusing symlink in TLS bundle: {}",
                    path.display()
                )));
            }
            write_private(
                &dest.join(&name),
                &fs::read(&path).map_err(crate::home_library::error)?,
            )?;
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

fn private_directory(path: &Path) -> Result<(), AppError> {
    if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(AppError::Policy(format!(
            "refusing symlink directory: {}",
            path.display()
        )));
    }
    fs::create_dir_all(path).map_err(crate::home_library::error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(crate::home_library::error)?;
    }
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(crate::home_library::error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(crate::home_library::error)?;
    }
    file.write_all(bytes).map_err(crate::home_library::error)?;
    file.sync_all().map_err(crate::home_library::error)
}

#[allow(clippy::too_many_arguments)]
async fn import_home(
    json: bool,
    from: &Path,
    library_root: PathBuf,
    config_dir: &Path,
    desired_state: &Path,
    tls_dir: &Path,
    ds_bytes: &[u8],
    rows: &[TitleIndexEntry],
) -> Result<String, AppError> {
    let cluster_bytes = fs::read(from.join(BUNDLE_CLUSTER)).map_err(|err| {
        AppError::Usage(format!(
            "current bundle requires readable cluster.json: {err}"
        ))
    })?;
    let mut cluster = mediaops_core::HomeObject::from_bytes(&cluster_bytes)
        .map_err(crate::home_library::error)?;
    cluster.validate().map_err(crate::home_library::error)?;
    let originally_locked = match &cluster.spec {
        mediaops_core::Spec::Cluster(spec) => spec.lock,
        _ => {
            return Err(AppError::Usage(
                "bundle cluster.json must contain a Cluster".into(),
            ));
        }
    };
    let secret = imported_secret(from)?;
    let api = crate::home_library::connect().await?;
    let mut previous_home = match api
        .get(mediaops_core::Kind::Cluster, mediaops_core::CLUSTER_NAME)
        .await
    {
        Ok(cluster) => Some(crate::home_library::HomeLibrary {
            api: api.clone(),
            cluster,
        }),
        Err(err) if err.is_not_found() => None,
        Err(err) => return Err(crate::home_library::error(err)),
    };
    // Pause existing scheduling before compatibility preflight. Otherwise a Want
    // can create/bind work between those checks and replacement of the root.
    let previous_lock = match &mut previous_home {
        Some(home) => Some(home.begin_maintenance().await?),
        None => None,
    };
    let preflight = async {
        if !api
            .list(Some(Kind::Job))
            .await
            .map_err(crate::home_library::error)?
            .is_empty()
        {
            return Err(AppError::Usage(
                "new-machine import requires an empty Home Job list".into(),
            ));
        }
        compatible_missing_rows(&api, rows, &library_root).await
    }
    .await;
    let missing = match preflight {
        Ok(missing) => missing,
        Err(err) => {
            if let (Some(home), Some(previous)) = (&mut previous_home, previous_lock) {
                home.finish_maintenance(previous).await?;
            }
            return Err(err);
        }
    };
    ensure_layout(&library_root)
        .map_err(crate::home_library::error)
        .map_err(crate::home_library::maintenance_failure)?;
    let library_root = fs::canonicalize(&library_root)
        .map_err(crate::home_library::error)
        .map_err(crate::home_library::maintenance_failure)?;
    if let mediaops_core::Spec::Cluster(spec) = &mut cluster.spec {
        spec.lock = true;
        spec.library_root = library_root.display().to_string();
    }
    cluster.status = mediaops_core::StatusBody::empty(mediaops_core::Kind::Cluster);
    // Bind the write to the exact Cluster we paused (or create-only when the
    // destination had none), rather than adopting a concurrently replaced one.
    if let Some(home) = &previous_home {
        cluster.metadata = home.cluster.metadata.clone();
    } else {
        cluster.metadata.uid.clear();
        cluster.metadata.resource_version = 0;
        cluster.metadata.generation = 0;
    }
    let cluster = api
        .apply(cluster)
        .await
        .map_err(crate::home_library::error)
        .map_err(crate::home_library::maintenance_failure)?;
    let mut home = crate::home_library::HomeLibrary { api, cluster };
    home.begin_maintenance().await?;

    let outcome: Result<(), AppError> = async {
        if let Some(parent) = desired_state.parent() {
            private_directory(parent)?;
        }
        write_private(desired_state, ds_bytes)?;
        copy_tls_dir(&from.join(BUNDLE_TLS), tls_dir)?;
        if let Some(secret) = secret {
            crate::home_library::apply_spec(&home.api, secret).await?;
        }
        if !missing.is_empty() {
            home.publish_rows(&missing, true).await?;
        }
        Ok(())
    }
    .await;
    outcome.map_err(crate::home_library::maintenance_failure)?;
    home.finish_maintenance(originally_locked).await?;
    let data = ImportData {
        config_dir: config_dir.display().to_string(),
        library_root: library_root.display().to_string(),
        titles: rows.len(),
        dirs: mediaops_sync::SCHEMA_DIRS
            .iter()
            .map(|name| name.to_string())
            .collect(),
    };
    if json {
        serde_json::to_string(&Envelope::ok(data)).map_err(crate::home_library::error)
    } else {
        Ok(format!(
            "new-machine import {} titles {}",
            data.library_root, data.titles
        ))
    }
}

fn index_bundle_rows(
    rows: &[TitleIndexEntry],
) -> Result<HashMap<(TitleId, String), &TitleIndexEntry>, AppError> {
    let mut index: HashMap<(TitleId, String), &TitleIndexEntry> = HashMap::new();
    for row in rows {
        if row.path_missing() {
            return Err(AppError::Usage(format!(
                "title-index path must be schema-relative: {}",
                row.path()
            )));
        }
        let key = (row.title_id().clone(), row.path().to_string());
        if let Some(previous) = index.get(&key)
            && (previous.install_b3() != row.install_b3()
                || previous.current_b3() != row.current_b3())
        {
            return Err(AppError::Usage(format!(
                "inconsistent duplicate title-index row: {}",
                row.path()
            )));
        }
        index.entry(key).or_insert(row);
    }
    Ok(index)
}

fn title_observations(status: &TitleStatus) -> Result<Vec<TitleFileStatus>, AppError> {
    if *status == TitleStatus::default() {
        return Ok(Vec::new());
    }
    let mut files = status.files.clone();
    let has_legacy =
        !status.path.is_empty() || status.install_b3.is_some() || status.current_b3.is_some();
    if has_legacy {
        let (Some(install_b3), Some(current_b3)) = (&status.install_b3, &status.current_b3) else {
            return Err(AppError::Usage("Title observation requires digests".into()));
        };
        if status.path.is_empty() {
            return Err(AppError::Usage(
                "Title observation requires a schema-relative path".into(),
            ));
        }
        match files.iter().find(|file| file.path == status.path) {
            Some(existing)
                if &existing.install_b3 != install_b3 || &existing.current_b3 != current_b3 =>
            {
                return Err(AppError::Usage("Title observation is inconsistent".into()));
            }
            Some(_) => {}
            None => files.push(TitleFileStatus {
                path: status.path.clone(),
                install_b3: install_b3.clone(),
                current_b3: current_b3.clone(),
                drifted: status.drifted,
            }),
        }
    } else if files.is_empty() {
        return Err(AppError::Usage(
            "Title status is not a complete observation".into(),
        ));
    }
    if files.iter().any(|file| file.path.is_empty()) {
        return Err(AppError::Usage(
            "Title observation requires a schema-relative path".into(),
        ));
    }
    Ok(files)
}

fn expected_title_spec(name: &str) -> TitleSpec {
    TitleSpec {
        title_id: name.to_string(),
        desired_present: true,
    }
}

async fn compatible_missing_rows(
    api: &mediaops_home_client::HomeApi,
    rows: &[TitleIndexEntry],
    library_root: &Path,
) -> Result<Vec<TitleIndexEntry>, AppError> {
    let bundle = index_bundle_rows(rows)?;
    let titles = api
        .list(Some(Kind::Title))
        .await
        .map_err(crate::home_library::error)?;
    if !titles.is_empty() {
        let cluster = api
            .get(Kind::Cluster, mediaops_core::CLUSTER_NAME)
            .await
            .map_err(crate::home_library::error)?;
        let Spec::Cluster(spec) = &cluster.spec else {
            return Err(AppError::Usage(
                "nonempty Title index requires a Cluster library root".into(),
            ));
        };
        let configured =
            fs::canonicalize(&spec.library_root).map_err(crate::home_library::error)?;
        let requested = fs::canonicalize(library_root).map_err(crate::home_library::error)?;
        if configured != requested {
            return Err(AppError::Usage(
                "--library-root must match Cluster.spec.libraryRoot; use library relocate to change it".into(),
            ));
        }
    }
    let mut existing = HashSet::new();
    for title in &titles {
        let Spec::Title(spec) = &title.spec else {
            return Err(AppError::Usage("invalid Title response".into()));
        };
        if spec != &expected_title_spec(&title.metadata.name) {
            return Err(AppError::Usage(format!(
                "existing Title spec is not the bundle Title: {}",
                title.metadata.name
            )));
        }
        let id = TitleId::parse(&spec.title_id).map_err(crate::home_library::error)?;
        if !bundle.keys().any(|(bundle_id, _)| bundle_id == &id) {
            return Err(AppError::Usage(format!(
                "foreign Title is not in the bundle: {}",
                spec.title_id
            )));
        }
        let StatusBody::Title(status) = &title.status else {
            return Err(AppError::Usage("Title has no observed status".into()));
        };
        for file in title_observations(status)? {
            let key = (id.clone(), file.path);
            let Some(row) = bundle.get(&key) else {
                return Err(AppError::Usage(format!(
                    "extra Title path is not in the bundle: {}",
                    key.1
                )));
            };
            if row.install_b3() != &file.install_b3 {
                return Err(AppError::Usage(format!(
                    "install digest is immutable: {}",
                    key.1
                )));
            }
            existing.insert(key);
        }
    }
    let mut missing = Vec::new();
    let mut queued = HashSet::new();
    for row in rows {
        let key = (row.title_id().clone(), row.path().to_string());
        if existing.contains(&key) || !queued.insert(key) {
            continue;
        }
        missing.push(row.clone());
    }
    Ok(missing)
}

fn imported_secret(from: &Path) -> Result<Option<mediaops_core::HomeObject>, AppError> {
    use mediaops_core::{HomeObject, Kind};
    let secret = match fs::read(from.join(BUNDLE_SECRET)) {
        Ok(bytes) => HomeObject::from_bytes(&bytes).map_err(crate::home_library::error)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(crate::home_library::error(err)),
    };
    if secret.kind != Kind::Secret {
        return Err(AppError::Usage(
            "bundle secret.json must contain a Secret".into(),
        ));
    }
    secret.validate().map_err(crate::home_library::error)?;
    Ok(Some(secret))
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

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{Blake3Hex, TitleId, TitleStatus};

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

    #[test]
    fn bundle_index_refuses_inconsistent_duplicate_paths() {
        let title = TitleId::movie_key("The.Matrix", 1999).expect("title");
        let path = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
        let first = TitleIndexEntry::new(
            title.clone(),
            path,
            Blake3Hex::of_bytes(b"a"),
            Blake3Hex::of_bytes(b"a"),
        );
        let second = TitleIndexEntry::new(
            title,
            path,
            Blake3Hex::of_bytes(b"b"),
            Blake3Hex::of_bytes(b"b"),
        );
        assert!(index_bundle_rows(&[first, second]).is_err());
    }

    #[test]
    fn empty_default_title_status_is_resumable() {
        assert!(
            title_observations(&TitleStatus::default())
                .expect("empty")
                .is_empty()
        );
    }

    #[test]
    fn malformed_partial_title_status_is_refused() {
        assert!(
            title_observations(&TitleStatus {
                path: "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv".into(),
                ..TitleStatus::default()
            })
            .is_err()
        );
        assert!(
            title_observations(&TitleStatus {
                drifted: true,
                ..TitleStatus::default()
            })
            .is_err()
        );
    }

    #[test]
    fn missing_tls_source_does_not_remove_existing_keys() {
        let dir = scratch("missing-tls");
        let destination = dir.join("tls");
        fs::create_dir_all(&destination).expect("tls");
        fs::write(destination.join("client.key"), b"keep").expect("key");
        assert!(copy_tls_dir(&dir.join("missing"), &destination).is_err());
        assert_eq!(
            fs::read(destination.join("client.key")).expect("key retained"),
            b"keep"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn bundle_key_writes_are_private_and_refuse_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = scratch("private-key");
        let key = dir.join("client.key");
        write_private(&key, b"secret").expect("private key");
        assert_eq!(
            fs::metadata(&key).expect("mode").permissions().mode() & 0o777,
            0o600
        );
        let link = dir.join("symlink.key");
        symlink(&key, &link).expect("symlink");
        assert!(write_private(&link, b"replace").is_err());
        assert_eq!(fs::read(&key).expect("key retained"), b"secret");
        let _ = fs::remove_dir_all(dir);
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
        assert!(
            !dest_db.exists(),
            "invalid bundle must not create local state"
        );
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
