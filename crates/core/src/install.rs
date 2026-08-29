//! Install gate: the only library-path writers.
//!
//! `install(TitleId, verified staging handle) -> installed path`
//! `replace(TitleId, verified .converting handle, backup destination) -> installed path`
//!
//! Destination is always via PathSchema. `replace` is encode's path and the
//! only writer of `current_b3`; persistence of digests is story 1.3 — this
//! story returns the installed path.

use std::fs;
use std::path::{Path, PathBuf};

use crate::pathschema::{self, PathSchemaError, Placement};
use crate::title_id::TitleId;

/// Verified file under `_incoming/<TitleId>/…`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedStagingHandle {
    source: PathBuf,
    dest_rel: PathBuf,
}

/// Verified `.converting` file used by encode's replace path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedConvertingHandle {
    source: PathBuf,
    dest_rel: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("not a verified staging path: {0}")]
    NotStaging(String),
    #[error("not a .converting file: {0}")]
    NotConverting(String),
    #[error("source file not found: {0}")]
    MissingSource(String),
    #[error("destination already exists: {0}")]
    DestinationExists(String),
    #[error("backup destination already exists: {0}")]
    BackupExists(String),
    #[error("live file not found for replace: {0}")]
    MissingLive(String),
    #[error("title does not match destination")]
    TitleMismatch,
    #[error(transparent)]
    PathSchema(#[from] PathSchemaError),
    #[error("io error: {0}")]
    Io(String),
}

impl From<std::io::Error> for InstallError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

impl VerifiedStagingHandle {
    /// Accept a staged file only when it is the exact `staging_path` tail for `title_id`.
    pub fn verify(
        title_id: &TitleId,
        source: PathBuf,
        placement: &Placement,
    ) -> Result<Self, InstallError> {
        reject_symlink_file(
            &source,
            InstallError::NotStaging(source.display().to_string()),
        )?;
        let dest_rel = pathschema::render(title_id, placement)?;
        if pathschema::parse(&dest_rel)? != *title_id {
            return Err(InstallError::TitleMismatch);
        }
        let final_name = dest_rel
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| InstallError::NotStaging(source.display().to_string()))?;
        let expected_tail = pathschema::staging_path(title_id, final_name)?;
        if !source.ends_with(&expected_tail) {
            return Err(InstallError::NotStaging(source.display().to_string()));
        }
        Ok(Self { source, dest_rel })
    }

    pub fn dest_rel(&self) -> &Path {
        &self.dest_rel
    }
}

impl VerifiedConvertingHandle {
    pub fn verify(
        title_id: &TitleId,
        source: PathBuf,
        placement: &Placement,
    ) -> Result<Self, InstallError> {
        let name = source
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| InstallError::NotConverting(source.display().to_string()))?;
        if !name.ends_with(".converting") {
            return Err(InstallError::NotConverting(source.display().to_string()));
        }
        reject_symlink_file(
            &source,
            InstallError::NotConverting(source.display().to_string()),
        )?;
        let dest_rel = pathschema::render(title_id, placement)?;
        if pathschema::parse(&dest_rel)? != *title_id {
            return Err(InstallError::TitleMismatch);
        }
        Ok(Self { source, dest_rel })
    }

    pub fn dest_rel(&self) -> &Path {
        &self.dest_rel
    }
}

fn reject_symlink_file(path: &Path, on_symlink: InstallError) -> Result<(), InstallError> {
    let meta = fs::symlink_metadata(path)
        .map_err(|_| InstallError::MissingSource(path.display().to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(on_symlink);
    }
    if !meta.is_file() {
        return Err(InstallError::MissingSource(path.display().to_string()));
    }
    Ok(())
}

/// Atomically place a verified staging file into the schema library path.
pub fn install(
    library_root: impl AsRef<Path>,
    title_id: &TitleId,
    handle: &VerifiedStagingHandle,
) -> Result<PathBuf, InstallError> {
    let dest = library_root.as_ref().join(&handle.dest_rel);
    if pathschema::parse(&handle.dest_rel)? != *title_id {
        return Err(InstallError::TitleMismatch);
    }
    if dest.exists() {
        return Err(InstallError::DestinationExists(dest.display().to_string()));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&handle.source, &dest)?;
    Ok(dest)
}

/// Move the live schema file to `backup_destination`, then place the converting file.
///
/// This is encode's path and the only writer of `current_b3` (digest persistence
/// is story 1.3; the return value is the installed path).
pub fn replace(
    library_root: impl AsRef<Path>,
    title_id: &TitleId,
    handle: &VerifiedConvertingHandle,
    backup_destination: impl AsRef<Path>,
) -> Result<PathBuf, InstallError> {
    let dest = library_root.as_ref().join(&handle.dest_rel);
    if pathschema::parse(&handle.dest_rel)? != *title_id {
        return Err(InstallError::TitleMismatch);
    }
    if !dest.is_file() {
        return Err(InstallError::MissingLive(dest.display().to_string()));
    }
    let backup_destination = backup_destination.as_ref();
    if backup_destination.exists() {
        return Err(InstallError::BackupExists(
            backup_destination.display().to_string(),
        ));
    }
    if let Some(parent) = backup_destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&dest, backup_destination)?;
    if let Err(err) = fs::rename(&handle.source, &dest) {
        fs::rename(backup_destination, &dest)?;
        return Err(err.into());
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathschema::{Placement, render, staging_path};
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::UNIX_EPOCH;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempTree {
        path: PathBuf,
    }

    impl TempTree {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mediaops-install-{}-{}-{}",
                std::process::id(),
                n,
                std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("time")
                    .as_nanos()
            ));
            fs::create_dir_all(&path).expect("mkdir");
            Self { path }
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        let mut f = fs::File::create(path).expect("create");
        f.write_all(bytes).expect("write");
    }

    #[test]
    fn install_writes_only_schema_library_path_from_staging_path() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let final_name = "The.Matrix.(1999).mkv";
        let staged_rel = staging_path(&title_id, final_name).expect("staging_path");
        assert_eq!(
            staged_rel.to_str().expect("utf8"),
            "_incoming/movie:tmdb:603/The.Matrix.(1999).mkv"
        );
        let staged = tmp.path.join(&staged_rel);
        write_file(&staged, b"matrix-bytes");

        let handle = VerifiedStagingHandle::verify(&title_id, staged.clone(), &placement)
            .expect("verify staging");
        let installed = install(&tmp.path, &title_id, &handle).expect("install");
        let expected = tmp
            .path
            .join(render(&title_id, &placement).expect("render"));
        assert_eq!(installed, expected);
        assert!(installed.starts_with(&tmp.path));
        assert_eq!(fs::read(&installed).expect("read"), b"matrix-bytes");
        assert!(!staged.exists());
        assert_eq!(
            pathschema::parse(installed.strip_prefix(&tmp.path).expect("strip")).expect("parse"),
            title_id
        );
    }

    #[test]
    fn replace_moves_live_file_to_backup_and_writes_schema_path() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let staged = tmp
            .path
            .join(staging_path(&title_id, "The.Matrix.(1999).mkv").expect("staging"));
        write_file(&staged, b"original");
        let handle = VerifiedStagingHandle::verify(&title_id, staged, &placement).expect("verify");
        let installed = install(&tmp.path, &title_id, &handle).expect("install");

        let converting = tmp
            .path
            .join("work")
            .join("The.Matrix.(1999).mkv.converting");
        write_file(&converting, b"encoded");
        let converting_handle =
            VerifiedConvertingHandle::verify(&title_id, converting.clone(), &placement)
                .expect("verify converting");
        let backup = tmp.path.join("backup").join("The.Matrix.(1999).mkv");
        let replaced = replace(&tmp.path, &title_id, &converting_handle, &backup).expect("replace");

        assert_eq!(replaced, installed);
        assert_eq!(fs::read(&replaced).expect("new"), b"encoded");
        assert_eq!(fs::read(&backup).expect("backup"), b"original");
        assert!(!converting.exists());
        assert_eq!(
            pathschema::parse(replaced.strip_prefix(&tmp.path).expect("strip")).expect("parse"),
            title_id
        );
    }

    #[test]
    fn verify_rejects_files_outside_staging_path() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let rogue = tmp.path.join("movies").join("rogue.mkv");
        write_file(&rogue, b"nope");
        assert!(matches!(
            VerifiedStagingHandle::verify(&title_id, rogue, &placement),
            Err(InstallError::NotStaging(_))
        ));
    }

    #[test]
    fn verify_rejects_other_title_id_staging_dir() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let other = TitleId::movie("604").expect("other");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let staged = tmp
            .path
            .join(staging_path(&other, "The.Matrix.(1999).mkv").expect("staging"));
        write_file(&staged, b"wrong-id");
        assert!(matches!(
            VerifiedStagingHandle::verify(&title_id, staged, &placement),
            Err(InstallError::NotStaging(_))
        ));
    }

    #[test]
    fn verify_rejects_staging_and_converting_symlinks() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let target = tmp.path.join("target.mkv");
        write_file(&target, b"bytes");

        let staged = tmp
            .path
            .join(staging_path(&title_id, "The.Matrix.(1999).mkv").expect("staging"));
        fs::create_dir_all(staged.parent().expect("parent")).expect("mkdir");
        std::os::unix::fs::symlink(&target, &staged).expect("staging symlink");
        assert!(matches!(
            VerifiedStagingHandle::verify(&title_id, staged, &placement),
            Err(InstallError::NotStaging(_))
        ));

        let converting = tmp
            .path
            .join("work")
            .join("The.Matrix.(1999).mkv.converting");
        fs::create_dir_all(converting.parent().expect("parent")).expect("mkdir");
        std::os::unix::fs::symlink(&target, &converting).expect("converting symlink");
        assert!(matches!(
            VerifiedConvertingHandle::verify(&title_id, converting, &placement),
            Err(InstallError::NotConverting(_))
        ));
    }

    #[test]
    fn second_install_of_same_dest_is_destination_exists() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let staged_rel = staging_path(&title_id, "The.Matrix.(1999).mkv").expect("staging");
        let staged = tmp.path.join(&staged_rel);
        write_file(&staged, b"original");
        let handle = VerifiedStagingHandle::verify(&title_id, staged, &placement).expect("verify");
        let installed = install(&tmp.path, &title_id, &handle).expect("install");

        write_file(&tmp.path.join(&staged_rel), b"other-bytes");
        let again =
            VerifiedStagingHandle::verify(&title_id, tmp.path.join(&staged_rel), &placement)
                .expect("verify again");
        let err = install(&tmp.path, &title_id, &again).expect_err("second");
        assert!(matches!(err, InstallError::DestinationExists(_)));
        assert_eq!(fs::read(&installed).expect("read"), b"original");
        assert_eq!(
            fs::read(tmp.path.join(&staged_rel)).expect("staging remains"),
            b"other-bytes"
        );
    }

    #[test]
    fn replace_refuses_existing_backup_and_restores_on_converting_rename_failure() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let staged = tmp
            .path
            .join(staging_path(&title_id, "The.Matrix.(1999).mkv").expect("staging"));
        write_file(&staged, b"original");
        let handle = VerifiedStagingHandle::verify(&title_id, staged, &placement).expect("verify");
        let installed = install(&tmp.path, &title_id, &handle).expect("install");

        let converting = tmp
            .path
            .join("work")
            .join("The.Matrix.(1999).mkv.converting");
        write_file(&converting, b"encoded");
        let converting_handle =
            VerifiedConvertingHandle::verify(&title_id, converting.clone(), &placement)
                .expect("verify converting");
        let backup = tmp.path.join("backup").join("The.Matrix.(1999).mkv");
        write_file(&backup, b"keep-me");
        let err =
            replace(&tmp.path, &title_id, &converting_handle, &backup).expect_err("backup exists");
        assert!(matches!(err, InstallError::BackupExists(_)));
        assert_eq!(fs::read(&installed).expect("live"), b"original");
        assert_eq!(fs::read(&backup).expect("backup"), b"keep-me");
        assert_eq!(fs::read(&converting).expect("converting"), b"encoded");

        fs::remove_file(&backup).expect("clear backup");
        fs::remove_file(&converting).expect("drop converting");
        let err = replace(&tmp.path, &title_id, &converting_handle, &backup)
            .expect_err("converting gone");
        assert!(matches!(err, InstallError::Io(_)));
        assert_eq!(fs::read(&installed).expect("restored"), b"original");
        assert!(!backup.exists());
    }
}
