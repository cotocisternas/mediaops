//! Bridge the remaining operator commands to the authoritative Home API.
//!
//! An explicit `--state-db` selects the offline legacy workflow. The default
//! workflow must connect to Home; an outage must never become a successful
//! mutation of an unused sqlite database.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mediaops_core::{
    Actor, Blake3Hex, CLUSTER_NAME, ClusterSpec, DesiredState, HomeObject, Kind, RootKinds, Spec,
    StatusBody, TitleFileStatus, TitleId, TitleIndexEntry, TitleSpec, TitleStatus,
};
use mediaops_home_client::{HomeApi, default_api_socket};

use crate::AppError;

pub(crate) struct HomeLibrary {
    pub api: HomeApi,
    pub cluster: HomeObject,
}

pub(crate) fn error(err: impl std::fmt::Display) -> AppError {
    AppError::Runtime(anyhow::anyhow!("{err}"))
}

pub(crate) fn maintenance_failure(err: AppError) -> AppError {
    let message = format!(
        "{err}; Cluster remains locked; resolve the error and clear Cluster.spec.lock to resume copying"
    );
    match err {
        AppError::Usage(_) => AppError::Usage(message),
        AppError::Policy(_) => AppError::Policy(message),
        AppError::LockConflict(_) => AppError::LockConflict(message),
        AppError::DriftVerify(_) => AppError::DriftVerify(message),
        AppError::Runtime(_) => error(message),
        AppError::Emitted(code) => AppError::Emitted(code),
    }
}

pub(crate) fn use_home(state_db: &Option<PathBuf>) -> bool {
    match state_db {
        None => true,
        Some(path) => {
            let path = resolved_state_path(path);
            path == resolved_state_path(&crate::bootstrap::default_state_db())
                || path
                    .parent()
                    .is_some_and(|parent| parent.join("api.db").exists())
        }
    }
}

fn resolved_state_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => std::fs::canonicalize(parent)
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    })
}

/// Every operation using the default Home API shares the same legacy flock and
/// local capability store, including callers that supplied an alias path.
pub(crate) fn state_db_path(requested: Option<PathBuf>) -> PathBuf {
    if use_home(&requested) {
        crate::bootstrap::default_state_db()
    } else {
        requested.expect("offline state requires an explicit path")
    }
}

pub(crate) async fn connect() -> Result<HomeApi, AppError> {
    HomeApi::connect(default_api_socket(), Actor::Import)
        .await
        .map_err(error)
}

pub(crate) fn cluster_from_config(ds: &DesiredState, root: &Path) -> HomeObject {
    HomeObject::new(
        Kind::Cluster,
        CLUSTER_NAME,
        Spec::Cluster(ClusterSpec {
            max_copy: ds.max_copy(),
            min_free: ds.min_free(),
            range_len: ds.range_len(),
            range_concurrency: ds.range_concurrency(),
            grabber: ds.grabber(),
            lock: ds.lock(),
            library_root: root.display().to_string(),
            roots: ds.paths().to_vec(),
            ..ClusterSpec::default()
        }),
        StatusBody::empty(Kind::Cluster),
    )
}

pub(crate) async fn apply_spec(
    api: &HomeApi,
    mut object: HomeObject,
) -> Result<HomeObject, AppError> {
    // Runtime observations from an exported object are not desired state.
    // Apply preserves the destination's status; new objects start unobserved.
    object.status = StatusBody::empty(object.kind);
    match api.get(object.kind, &object.metadata.name).await {
        Ok(previous) => object.metadata = previous.metadata,
        Err(err) if err.is_not_found() => {
            object.metadata.uid.clear();
            object.metadata.resource_version = 0;
            object.metadata.generation = 0;
        }
        Err(err) => return Err(error(err)),
    }
    api.apply(object).await.map_err(error)
}

impl HomeLibrary {
    pub async fn load() -> Result<Self, AppError> {
        let api = connect().await?;
        let cluster = api.get(Kind::Cluster, CLUSTER_NAME).await.map_err(error)?;
        Ok(Self { api, cluster })
    }

    pub fn spec(&self) -> Result<&ClusterSpec, AppError> {
        match &self.cluster.spec {
            Spec::Cluster(spec) => Ok(spec),
            _ => Err(error("Cluster has no Cluster spec")),
        }
    }

    pub fn root(&self, requested: Option<PathBuf>) -> Result<PathBuf, AppError> {
        let root = PathBuf::from(&self.spec()?.library_root);
        crate::library::refuse_library_root(&root)?;
        if let Some(requested) = requested {
            let requested = std::fs::canonicalize(requested).map_err(error)?;
            let configured = std::fs::canonicalize(&root).map_err(error)?;
            if requested != configured {
                return Err(AppError::Usage(
                    "--library-root must match Cluster.spec.libraryRoot; use library relocate to change it".into(),
                ));
            }
        }
        Ok(root)
    }

    pub fn root_kinds(&self) -> Result<RootKinds, AppError> {
        Ok(self
            .spec()?
            .roots
            .iter()
            .map(|root| (root.id.clone(), root.kind))
            .collect())
    }

    pub async fn begin_maintenance(&mut self) -> Result<bool, AppError> {
        let previously_locked = self.set_lock(true).await?;
        let active = self
            .api
            .list(Some(Kind::Job))
            .await
            .map_err(error)
            .map_err(maintenance_failure)?
            .iter()
            .any(|job| {
                matches!((&job.spec, &job.status), (Spec::Job(spec), StatusBody::Job(status))
                if !spec.node_name.is_empty() && !status.phase.is_terminal())
            });
        if active {
            self.finish_maintenance(previously_locked).await?;
            return Err(AppError::Policy(
                "wait for bound Pull Jobs to finish before library maintenance".into(),
            ));
        }
        Ok(previously_locked)
    }

    pub async fn finish_maintenance(&mut self, previously_locked: bool) -> Result<(), AppError> {
        self.set_lock(previously_locked)
            .await
            .map_err(maintenance_failure)?;
        Ok(())
    }

    async fn set_lock(&mut self, locked: bool) -> Result<bool, AppError> {
        for attempt in 0..5 {
            let mut cluster = self
                .api
                .get(Kind::Cluster, CLUSTER_NAME)
                .await
                .map_err(error)?;
            let Spec::Cluster(spec) = &mut cluster.spec else {
                return Err(error("invalid Cluster"));
            };
            let previous = spec.lock;
            spec.lock = locked;
            match self.api.patch(cluster, "spec").await {
                Ok(written) => {
                    self.cluster = written;
                    return Ok(previous);
                }
                Err(err) if err.is_conflict() && attempt < 4 => continue,
                Err(err) => return Err(error(err)),
            }
        }
        Err(error("Cluster remained busy during maintenance"))
    }

    pub async fn rows(&self, include_drifted: bool) -> Result<Vec<TitleIndexEntry>, AppError> {
        let titles = self.api.list(Some(Kind::Title)).await.map_err(error)?;
        let root = self.root(None)?;
        rows_from_objects(&titles, include_drifted)?
            .into_iter()
            .map(|row| {
                Ok(TitleIndexEntry::new(
                    row.title_id().clone(),
                    schema_relative(&root, row.path())?,
                    row.install_b3().clone(),
                    row.current_b3().clone(),
                ))
            })
            .collect()
    }

    /// Keep the immutable install digest when encoding changes current bytes.
    pub async fn record_replace(&self, path: &str, digest: &Blake3Hex) -> Result<(), AppError> {
        let rows = self.rows(true).await?;
        let row = rows
            .iter()
            .find(|row| row.path() == path)
            .ok_or_else(|| error("no Home Title proof for encoded file"))?;
        self.publish_rows(
            &[TitleIndexEntry::new(
                row.title_id().clone(),
                path,
                row.install_b3().clone(),
                digest.clone(),
            )],
            false,
        )
        .await
    }

    /// Import/reindex publishes per-file proof through the status subresource.
    /// The API independently verifies non-drifted files before accepting them.
    pub async fn publish_rows(
        &self,
        rows: &[TitleIndexEntry],
        allow_missing: bool,
    ) -> Result<(), AppError> {
        let root = self.root(None)?;
        let mut grouped: BTreeMap<String, Vec<&TitleIndexEntry>> = BTreeMap::new();
        for row in rows {
            grouped
                .entry(row.title_id().render())
                .or_default()
                .push(row);
        }
        for (title_id, rows) in grouped {
            for attempt in 0..5 {
                let mut object = match self.api.get(Kind::Title, &title_id).await {
                    Ok(object) => object,
                    Err(err) if err.is_not_found() => {
                        let object = HomeObject::new(
                            Kind::Title,
                            &title_id,
                            Spec::Title(TitleSpec {
                                title_id: title_id.clone(),
                                desired_present: true,
                            }),
                            StatusBody::Title(TitleStatus::default()),
                        );
                        match self.api.apply(object).await {
                            Ok(object) => object,
                            Err(err) if err.is_conflict() && attempt < 4 => continue,
                            Err(err) => return Err(error(err)),
                        }
                    }
                    Err(err) => return Err(error(err)),
                };
                let StatusBody::Title(status) = &object.status else {
                    return Err(error("Title has no observed status"));
                };
                let mut files = status.observed_files();
                for row in &rows {
                    let path = schema_relative(&root, row.path())?;
                    let current =
                        std::fs::File::open(root.join(&path)).and_then(Blake3Hex::of_reader);
                    let drifted = match current {
                        Ok(digest) => &digest != row.current_b3(),
                        Err(err) if allow_missing && err.kind() == std::io::ErrorKind::NotFound => {
                            true
                        }
                        Err(err) => return Err(error(err)),
                    };
                    if drifted && !allow_missing {
                        return Err(AppError::DriftVerify(format!(
                            "library file changed: {path}"
                        )));
                    }
                    if let Some(existing) = files.iter().find(|file| file.path == path)
                        && &existing.install_b3 != row.install_b3()
                    {
                        return Err(AppError::DriftVerify(format!(
                            "install digest is immutable: {path}"
                        )));
                    }
                    files.retain(|file| file.path != path);
                    files.push(TitleFileStatus {
                        path,
                        install_b3: row.install_b3().clone(),
                        current_b3: row.current_b3().clone(),
                        drifted,
                    });
                }
                files.sort_by(|a, b| a.path.cmp(&b.path));
                object.status = StatusBody::Title(TitleStatus {
                    drifted: files.iter().any(|file| file.drifted),
                    files,
                    ..TitleStatus::default()
                });
                match self.api.patch(object, "status").await {
                    Ok(_) => break,
                    Err(err) if err.is_conflict() && attempt < 4 => continue,
                    Err(err) => return Err(error(err)),
                }
            }
        }
        Ok(())
    }

    pub async fn reindex(&self) -> Result<usize, AppError> {
        let root = self.root(None)?;
        let existing = self.rows(true).await?;
        let files = mediaops_sync::scan_schema_files(&root).map_err(error)?;
        let mut rows = Vec::with_capacity(files.len());
        for file in files {
            let digest =
                Blake3Hex::of_reader(std::fs::File::open(root.join(&file.path)).map_err(error)?)
                    .map_err(error)?;
            if let Some(previous) = existing.iter().find(|row| row.path() == file.path) {
                if previous.current_b3() != &digest {
                    return Err(AppError::DriftVerify(format!(
                        "library file changed: {}",
                        file.path
                    )));
                }
                rows.push(previous.clone());
            } else {
                rows.push(TitleIndexEntry::new(
                    file.title_id,
                    file.path,
                    digest.clone(),
                    digest,
                ));
            }
        }
        self.publish_rows(&rows, false).await?;
        Ok(rows.len())
    }
}

pub(crate) fn schema_relative(root: &Path, path: &str) -> Result<String, AppError> {
    let path = Path::new(path);
    let relative = if path.is_absolute() {
        path.strip_prefix(root).map_err(error)?
    } else {
        path
    };
    let (_, placement) = mediaops_core::parse_placement(relative).map_err(error)?;
    let canonical = mediaops_core::render_placement(&placement).map_err(error)?;
    if relative != canonical {
        return Err(AppError::Usage(format!(
            "noncanonical schema path: {}",
            relative.display()
        )));
    }
    Ok(canonical.to_string_lossy().into_owned())
}

fn rows_from_objects(
    titles: &[HomeObject],
    include_drifted: bool,
) -> Result<Vec<TitleIndexEntry>, AppError> {
    let mut rows = Vec::new();
    for title in titles {
        let (Spec::Title(spec), StatusBody::Title(status)) = (&title.spec, &title.status) else {
            return Err(error("invalid Title response"));
        };
        let id = TitleId::parse(&spec.title_id).map_err(error)?;
        for file in status.observed_files() {
            if include_drifted || !file.drifted {
                rows.push(TitleIndexEntry::new(
                    id.clone(),
                    file.path,
                    file.install_b3,
                    file.current_b3,
                ));
            }
        }
    }
    Ok(rows)
}

#[cfg(test)]
pub(crate) async fn test_home(tag: &str) -> (HomeLibrary, PathBuf, tokio::task::JoinHandle<()>) {
    let dir = crate::test_support::scratch(tag);
    let root = dir.join("library");
    mediaops_sync::ensure_layout(&root).expect("layout");
    let socket = dir.join("api.sock");
    let config = mediaops_apiserver::ApiConfig {
        socket: socket.clone(),
        api_db: dir.join("api.db"),
    };
    let server = tokio::spawn(async move {
        mediaops_apiserver::serve_api(config).await.expect("api");
    });
    let mut api = None;
    for _ in 0..100 {
        match HomeApi::connect(&socket, Actor::Import).await {
            Ok(connected) => {
                api = Some(connected);
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
        }
    }
    let api = api.expect("API ready");
    let ds = DesiredState::from_toml("schema_version=1\nmax_copy_gib=1\nmin_free_gib=0\nrange_len_mib=1\nmax_nvenc=1\nlock=false\n").expect("config");
    let cluster = api
        .apply(cluster_from_config(&ds, &root))
        .await
        .expect("cluster");
    (HomeLibrary { api, cluster }, dir, server)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_track_keeps_both_digests_and_drift_is_not_reclaim_proof() {
        let original = Blake3Hex::of_bytes(b"original");
        let encoded = Blake3Hex::of_bytes(b"encoded");
        let title = HomeObject::new(
            Kind::Title,
            "album:key:tool.lateralus",
            Spec::Title(TitleSpec {
                title_id: "album:key:tool.lateralus".into(),
                desired_present: true,
            }),
            StatusBody::Title(TitleStatus {
                files: vec![
                    TitleFileStatus {
                        path: "one.flac".into(),
                        install_b3: original.clone(),
                        current_b3: encoded.clone(),
                        drifted: false,
                    },
                    TitleFileStatus {
                        path: "two.flac".into(),
                        install_b3: original.clone(),
                        current_b3: original.clone(),
                        drifted: true,
                    },
                ],
                ..TitleStatus::default()
            }),
        );
        let safe = rows_from_objects(std::slice::from_ref(&title), false).expect("rows");
        assert_eq!(safe.len(), 1);
        assert_eq!(safe[0].install_b3(), &original);
        assert_eq!(safe[0].current_b3(), &encoded);
        assert_eq!(
            rows_from_objects(&[title], true)
                .expect("export rows")
                .len(),
            2
        );
    }

    #[test]
    fn schema_relative_rejects_paths_outside_library() {
        assert!(
            schema_relative(Path::new("/library"), "/other/movies/A.(2000)/A.(2000).mkv").is_err()
        );
        assert!(schema_relative(Path::new("/library"), "../movies/A.(2000)/A.(2000).mkv").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_cannot_select_offline_writes_to_a_migrated_database() {
        let dir = crate::test_support::scratch("api-routing");
        let migrated = dir.join("migrated");
        std::fs::create_dir_all(&migrated).expect("directory");
        std::fs::write(migrated.join("state.db"), b"").expect("state");
        std::fs::write(migrated.join("api.db"), b"").expect("migration marker");
        let link = dir.join("offline.db");
        std::os::unix::fs::symlink(migrated.join("state.db"), &link).expect("symlink");
        assert!(use_home(&Some(link)));
        assert!(!use_home(&Some(dir.join("fixture.db"))));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn reindex_preserves_install_digest_after_encoding() {
        let (home, dir, server) = test_home("api-reindex").await;
        let root = home.root(None).expect("root");
        let path = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
        std::fs::create_dir_all(root.join(path).parent().expect("parent")).expect("parent");
        std::fs::write(root.join(path), b"encoded").expect("file");
        let row = TitleIndexEntry::new(
            TitleId::movie("603").expect("id"),
            path,
            Blake3Hex::of_bytes(b"original"),
            Blake3Hex::of_bytes(b"encoded"),
        );
        home.publish_rows(std::slice::from_ref(&row), false)
            .await
            .expect("import proof");
        assert_eq!(home.reindex().await.expect("reindex"), 1);
        assert_eq!(home.rows(false).await.expect("proofs"), vec![row]);
        server.abort();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn missing_bundle_files_remain_drifted_and_do_not_prove_reclaim() {
        let (home, dir, server) = test_home("api-restore").await;
        let row = TitleIndexEntry::new(
            TitleId::movie("603").expect("id"),
            "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv",
            Blake3Hex::of_bytes(b"original"),
            Blake3Hex::of_bytes(b"encoded"),
        );
        home.publish_rows(std::slice::from_ref(&row), true)
            .await
            .expect("restore missing");
        assert!(home.rows(false).await.expect("reclaim proof").is_empty());
        assert_eq!(home.rows(true).await.expect("export proof"), vec![row]);
        server.abort();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn maintenance_restores_preexisting_lock_value() {
        let (mut home, dir, server) = test_home("api-maintenance").await;
        let previous = home.begin_maintenance().await.expect("pause");
        assert!(!previous);
        assert!(home.spec().expect("spec").lock);
        home.finish_maintenance(previous).await.expect("resume");
        assert!(!home.spec().expect("spec").lock);
        server.abort();
        let _ = std::fs::remove_dir_all(dir);
    }
}
