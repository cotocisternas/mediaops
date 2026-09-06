//! Staging layout: exact names, writer lock, verified cleanup.

use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use mediaops_core::staging_path;

use crate::TransferError;
use crate::pull::PullSpec;
use crate::sidecar::{self, Sidecar};

pub(crate) struct StagingLayout {
    pub staged: PathBuf,
    pub partial: PathBuf,
    pub sidecar: PathBuf,
    pub lock: PathBuf,
}

impl StagingLayout {
    pub(crate) fn from_spec(spec: &PullSpec) -> Result<Self, TransferError> {
        let rel = staging_path(&spec.title_id, &spec.final_name)
            .map_err(|err| TransferError::Path(err.to_string()))?;
        Ok(Self::from_staged(spec.library_root.join(rel)))
    }

    pub(crate) fn from_staged(staged: PathBuf) -> Self {
        let mut partial = staged.clone();
        partial.as_mut_os_string().push(".partial");
        let mut sidecar = staged.clone();
        sidecar.as_mut_os_string().push(".partial.b3");
        let lock = staged.with_extension("pull.lock");
        Self {
            staged,
            partial,
            sidecar,
            lock,
        }
    }
}

pub(crate) fn check_source(sidecar: &Sidecar, spec: &PullSpec) -> Result<(), TransferError> {
    if sidecar.file_len != spec.file_len
        || ((!sidecar.remote_root.is_empty() || !sidecar.remote_path.is_empty())
            && (sidecar.remote_root != spec.remote.root_id()
                || Path::new(&sidecar.remote_path) != spec.remote.rel_path()))
    {
        return Err(TransferError::Sidecar(
            "staging proof belongs to another remote file".into(),
        ));
    }
    Ok(())
}

pub(crate) fn source_is_known(sidecar: &Sidecar) -> bool {
    !sidecar.remote_root.is_empty() && !sidecar.remote_path.is_empty()
}

pub(crate) fn acquire_writer_lock(path: &Path, create: bool) -> Result<File, TransferError> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|err| TransferError::io(path, err))?;
    lock.try_lock()
        .map_err(|err| TransferError::Path(format!("staging file already has a writer: {err}")))?;
    Ok(lock)
}

fn missing(path: &Path) -> Result<bool, TransferError> {
    match fs::symlink_metadata(path) {
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(true),
        Err(err) => Err(TransferError::io(path, err)),
        Ok(_) => Ok(false),
    }
}

fn remove_if_present(path: &Path) -> Result<(), TransferError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(TransferError::io(path, err)),
    }
}

fn sync_parent(path: &Path) -> Result<(), TransferError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|err| TransferError::io(parent, err))
}

/// Remove the owned staged source and `.partial.b3` after dest matches `verified_b3`.
///
/// Only the exact `staging_path` name is considered. The writer lock and any
/// unknown files are retained. Missing staged or sidecar files succeed.
pub fn cleanup_verified_staging(spec: &PullSpec) -> Result<(), TransferError> {
    let layout = StagingLayout::from_spec(spec)?;
    let lock = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(false)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&layout.lock)
    {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            if missing(&layout.staged)? && missing(&layout.sidecar)? {
                return Ok(());
            }
            return Err(TransferError::Path("staging writer lock is missing".into()));
        }
        Err(err) => return Err(TransferError::io(&layout.lock, err)),
    };
    lock.try_lock()
        .map_err(|err| TransferError::Path(format!("staging file already has a writer: {err}")))?;
    match fs::symlink_metadata(&layout.staged) {
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(TransferError::io(&layout.staged, err)),
        Ok(meta) if meta.file_type().is_file() => {}
        Ok(_) => {
            return Err(TransferError::Path(
                "staged source must be a regular file".into(),
            ));
        }
    }
    if let Some(proof) = sidecar::load(&layout.sidecar)? {
        check_source(&proof, spec)?;
        if !source_is_known(&proof) {
            return Err(TransferError::Sidecar(
                "legacy staged file has no remote identity; refusing to delete its bytes".into(),
            ));
        }
    }
    remove_if_present(&layout.staged)?;
    remove_if_present(&layout.sidecar)?;
    sync_parent(&layout.staged)?;
    drop(lock);
    Ok(())
}
