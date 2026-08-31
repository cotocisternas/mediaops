//! Install gate: the only library-path writers.
//!
//! `install(TitleId, verified staging handle) -> installed path + whole-file BLAKE3`
//! `replace(TitleId, verified .converting handle, backup destination) -> installed path`
//!
//! Destination is always via PathSchema. This gate is filesystem-only and
//! returns the installed path. Callers record digests through
//! [`crate::TitleIndexRepo`] after a successful place: `record_install` on
//! first install (`install_b3` and `current_b3`), `record_replace` after
//! encode's [`replace`] (the only later writer of `current_b3`). The gate
//! does not take a repository and does not touch sqlite.

use std::fs;
use std::path::{Path, PathBuf};

use crate::digest::Blake3Hex;
use crate::pathschema::{self, PathSchemaError, Placement};
use crate::title_id::TitleId;

/// Result of [`install`]: schema path plus the digest hashed at this gate (AD-11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub path: PathBuf,
    pub whole_file_b3: Blake3Hex,
}

impl AsRef<Path> for InstallOutcome {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

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
    #[error("backup destination `{0}` is inside the library root")]
    BackupInsideLibrary(String),
    /// `replace` moved the live file aside, then could neither place the new
    /// file nor put the old one back. Both failures are reported, and the live
    /// bytes are named so an operator can recover them by hand.
    #[error(
        "replace failed ({cause}) and the live file could not be restored ({restore}); \
         the live file is now at `{live_now_at}`"
    )]
    RollbackFailed {
        cause: String,
        restore: String,
        live_now_at: String,
    },
    #[error(transparent)]
    PathSchema(#[from] PathSchemaError),
    /// Carries the failing path and the `ErrorKind`, so callers can tell `EXDEV`
    /// (staging on another filesystem) from `ENOSPC` from `EACCES`.
    #[error("io error at `{path}`: {message}")]
    Io {
        path: String,
        kind: std::io::ErrorKind,
        message: String,
    },
}

impl InstallError {
    fn io(path: &Path, err: &std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            kind: err.kind(),
            message: err.to_string(),
        }
    }

    /// `ErrorKind` for an io failure, `None` for every policy refusal.
    pub fn io_kind(&self) -> Option<std::io::ErrorKind> {
        match self {
            Self::Io { kind, .. } => Some(*kind),
            _ => None,
        }
    }
}

impl VerifiedStagingHandle {
    /// Accept a staged file only when it is `library_root` plus the exact
    /// `staging_path` tail for `title_id`. A matching tail outside the library
    /// root is not staging.
    pub fn verify(
        library_root: impl AsRef<Path>,
        title_id: &TitleId,
        source: PathBuf,
        placement: &Placement,
    ) -> Result<Self, InstallError> {
        let dest_rel = pathschema::render(title_id, placement)?;
        if pathschema::parse(&dest_rel)? != *title_id {
            return Err(InstallError::TitleMismatch);
        }
        let final_name = dest_rel
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| InstallError::NotStaging(source.display().to_string()))?;
        let expected_tail = pathschema::staging_path(title_id, final_name)?;
        let Ok(rel) = source.strip_prefix(library_root.as_ref()) else {
            return Err(InstallError::NotStaging(source.display().to_string()));
        };
        if rel != expected_tail.as_path() || !is_under(library_root.as_ref(), &source) {
            return Err(InstallError::NotStaging(source.display().to_string()));
        }
        reject_symlink_file(
            &source,
            InstallError::NotStaging(source.display().to_string()),
        )?;
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
        let dest_rel = pathschema::render(title_id, placement)?;
        if pathschema::parse(&dest_rel)? != *title_id {
            return Err(InstallError::TitleMismatch);
        }
        // `<rendered file name>.converting`, not merely *some* `.converting`
        // file: otherwise an unrelated encode output can replace a live title.
        let expected = dest_rel
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| format!("{n}.converting"))
            .ok_or_else(|| InstallError::NotConverting(source.display().to_string()))?;
        if name != expected {
            return Err(InstallError::NotConverting(source.display().to_string()));
        }
        reject_symlink_file(
            &source,
            InstallError::NotConverting(source.display().to_string()),
        )?;
        Ok(Self { source, dest_rel })
    }

    pub fn dest_rel(&self) -> &Path {
        &self.dest_rel
    }
}

/// `candidate` is under `root` without walking out via `..`.
fn is_under(root: &Path, candidate: &Path) -> bool {
    let Ok(rel) = candidate.strip_prefix(root) else {
        return false;
    };
    let mut depth = 0_i32;
    for component in rel.components() {
        match component {
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

/// Free bytes on the filesystem that holds `path` (watermark checks).
pub fn free_bytes(path: &Path) -> Result<u64, InstallError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let cstr = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            InstallError::Io {
                path: path.display().to_string(),
                kind: std::io::ErrorKind::InvalidInput,
                message: "path contains NUL".into(),
            }
        })?;
        let mut vfs = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        let rc = unsafe { libc::statvfs(cstr.as_ptr(), vfs.as_mut_ptr()) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            return Err(InstallError::io(path, &err));
        }
        let vfs = unsafe { vfs.assume_init() };
        Ok(vfs.f_bavail as u64 * vfs.f_frsize as u64)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(InstallError::Io {
            path: path.display().to_string(),
            kind: std::io::ErrorKind::Unsupported,
            message: "df is unix-only in v1".into(),
        })
    }
}

fn reject_symlink_file(path: &Path, on_symlink: InstallError) -> Result<(), InstallError> {
    let meta = fs::symlink_metadata(path).map_err(|err| match err.kind() {
        // Only a genuine absence is "not found"; permissions and symlink loops
        // are their own diagnosis.
        std::io::ErrorKind::NotFound => InstallError::MissingSource(path.display().to_string()),
        _ => InstallError::io(path, &err),
    })?;
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
) -> Result<InstallOutcome, InstallError> {
    let dest = library_root.as_ref().join(&handle.dest_rel);
    if pathschema::parse(&handle.dest_rel)? != *title_id {
        return Err(InstallError::TitleMismatch);
    }
    // `exists()` follows symlinks, so a dangling symlink squatting the library
    // path would report `false`. `symlink_metadata` sees the entry itself.
    if fs::symlink_metadata(&dest).is_ok() {
        return Err(InstallError::DestinationExists(dest.display().to_string()));
    }
    let file = fs::File::open(&handle.source).map_err(|err| InstallError::io(&handle.source, &err))?;
    let whole_file_b3 =
        Blake3Hex::of_reader(file).map_err(|err| InstallError::io(&handle.source, &err))?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| InstallError::io(parent, &err))?;
    }
    fs::rename(&handle.source, &dest).map_err(|err| InstallError::io(&handle.source, &err))?;
    Ok(InstallOutcome {
        path: dest,
        whole_file_b3,
    })
}

/// Move the live schema file to `backup_destination`, then place the converting file.
///
/// Encode's path. Callers persist the new `current_b3` through
/// [`crate::TitleIndexRepo::record_replace`] after this returns.
pub fn replace(
    library_root: impl AsRef<Path>,
    title_id: &TitleId,
    handle: &VerifiedConvertingHandle,
    backup_destination: impl AsRef<Path>,
) -> Result<PathBuf, InstallError> {
    let library_root = library_root.as_ref();
    let dest = library_root.join(&handle.dest_rel);
    if pathschema::parse(&handle.dest_rel)? != *title_id {
        return Err(InstallError::TitleMismatch);
    }
    let backup_destination = backup_destination.as_ref();
    // The library is written only through the schema path; a backup landing
    // inside it would be a second, unschema'd writer.
    if backup_destination.starts_with(library_root) {
        return Err(InstallError::BackupInsideLibrary(
            backup_destination.display().to_string(),
        ));
    }
    let live = fs::symlink_metadata(&dest);
    match &live {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
            return Err(InstallError::MissingLive(dest.display().to_string()));
        }
        Err(_) => return Err(InstallError::MissingLive(dest.display().to_string())),
        Ok(_) => {}
    }
    if fs::symlink_metadata(backup_destination).is_ok() {
        return Err(InstallError::BackupExists(
            backup_destination.display().to_string(),
        ));
    }
    if let Some(parent) = backup_destination.parent() {
        fs::create_dir_all(parent).map_err(|err| InstallError::io(parent, &err))?;
    }
    fs::rename(&dest, backup_destination).map_err(|err| InstallError::io(&dest, &err))?;
    if let Err(err) = fs::rename(&handle.source, &dest) {
        // Put the live file back. If that also fails, both failures are
        // reported -- the cause is never dropped for the rollback's error.
        if let Err(restore) = fs::rename(backup_destination, &dest) {
            return Err(InstallError::RollbackFailed {
                cause: err.to_string(),
                restore: restore.to_string(),
                live_now_at: backup_destination.display().to_string(),
            });
        }
        return Err(InstallError::io(&handle.source, &err));
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
            "_incoming/movie-tmdb-603/The.Matrix.(1999).mkv"
        );
        let lib = tmp.path.join("library");
        let staged = lib.join(&staged_rel);
        write_file(&staged, b"matrix-bytes");

        let handle = VerifiedStagingHandle::verify(&lib, &title_id, staged.clone(), &placement)
            .expect("verify staging");
        let installed = install(&lib, &title_id, &handle).expect("install");
        let expected = lib.join(render(&title_id, &placement).expect("render"));
        assert_eq!(installed.path, expected);
        assert!(installed.path.starts_with(&lib));
        assert_eq!(fs::read(&installed.path).expect("read"), b"matrix-bytes");
        assert_eq!(installed.whole_file_b3, crate::Blake3Hex::of_bytes(b"matrix-bytes"));
        assert!(!staged.exists());
        assert_eq!(
            pathschema::parse(installed.path.strip_prefix(&lib).expect("strip")).expect("parse"),
            title_id
        );
    }

    #[test]
    fn replace_moves_live_file_to_backup_and_writes_schema_path() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let lib = tmp.path.join("library");
        let staged = lib.join(staging_path(&title_id, "The.Matrix.(1999).mkv").expect("staging"));
        write_file(&staged, b"original");
        let handle =
            VerifiedStagingHandle::verify(&lib, &title_id, staged, &placement).expect("verify");
        let installed = install(&lib, &title_id, &handle).expect("install");

        let converting = tmp
            .path
            .join("work")
            .join("The.Matrix.(1999).mkv.converting");
        write_file(&converting, b"encoded");
        let converting_handle =
            VerifiedConvertingHandle::verify(&title_id, converting.clone(), &placement)
                .expect("verify converting");
        let backup = tmp.path.join("backup").join("The.Matrix.(1999).mkv");
        let replaced = replace(&lib, &title_id, &converting_handle, &backup).expect("replace");

        assert_eq!(replaced, installed.path);
        assert_eq!(fs::read(&replaced).expect("new"), b"encoded");
        assert_eq!(fs::read(&backup).expect("backup"), b"original");
        assert!(!converting.exists());
        assert_eq!(
            pathschema::parse(replaced.strip_prefix(&lib).expect("strip")).expect("parse"),
            title_id
        );
    }

    #[test]
    fn verify_rejects_files_outside_staging_path() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let lib = tmp.path.join("library");
        let rogue = tmp.path.join("movies").join("rogue.mkv");
        write_file(&rogue, b"nope");
        assert!(matches!(
            VerifiedStagingHandle::verify(&lib, &title_id, rogue, &placement),
            Err(InstallError::NotStaging(_))
        ));
    }

    #[test]
    fn verify_rejects_matching_tail_outside_library_root() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let lib = tmp.path.join("library");
        let impostor = tmp
            .path
            .join("elsewhere")
            .join(staging_path(&title_id, "The.Matrix.(1999).mkv").expect("staging"));
        write_file(&impostor, b"nope");
        assert!(matches!(
            VerifiedStagingHandle::verify(&lib, &title_id, impostor, &placement),
            Err(InstallError::NotStaging(_))
        ));
    }

    #[test]
    fn verify_rejects_parent_dir_escape_with_matching_staging_tail() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let lib = tmp.path.join("library");
        fs::create_dir_all(&lib).expect("lib");
        let tail = staging_path(&title_id, "The.Matrix.(1999).mkv").expect("staging");
        let impostor = lib.join("..").join("elsewhere").join(&tail);
        write_file(&impostor, b"nope");
        assert!(matches!(
            VerifiedStagingHandle::verify(&lib, &title_id, impostor, &placement),
            Err(InstallError::NotStaging(_))
        ));
    }

    #[test]
    fn verify_rejects_other_title_id_staging_dir() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let other = TitleId::movie("604").expect("other");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let lib = tmp.path.join("library");
        let staged = lib.join(staging_path(&other, "The.Matrix.(1999).mkv").expect("staging"));
        write_file(&staged, b"wrong-id");
        assert!(matches!(
            VerifiedStagingHandle::verify(&lib, &title_id, staged, &placement),
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

        let lib = tmp.path.join("library");
        let staged = lib.join(staging_path(&title_id, "The.Matrix.(1999).mkv").expect("staging"));
        fs::create_dir_all(staged.parent().expect("parent")).expect("mkdir");
        std::os::unix::fs::symlink(&target, &staged).expect("staging symlink");
        assert!(matches!(
            VerifiedStagingHandle::verify(&lib, &title_id, staged, &placement),
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
        let lib = tmp.path.join("library");
        let staged = lib.join(&staged_rel);
        write_file(&staged, b"original");
        let handle =
            VerifiedStagingHandle::verify(&lib, &title_id, staged, &placement).expect("verify");
        let installed = install(&lib, &title_id, &handle).expect("install");

        write_file(&lib.join(&staged_rel), b"other-bytes");
        let again =
            VerifiedStagingHandle::verify(&lib, &title_id, lib.join(&staged_rel), &placement)
                .expect("verify again");
        let err = install(&lib, &title_id, &again).expect_err("second");
        assert!(matches!(err, InstallError::DestinationExists(_)));
        assert_eq!(fs::read(&installed.path).expect("read"), b"original");
        assert_eq!(
            fs::read(lib.join(&staged_rel)).expect("staging remains"),
            b"other-bytes"
        );
    }

    #[test]
    fn replace_refuses_existing_backup_and_restores_on_converting_rename_failure() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let lib = tmp.path.join("library");
        let staged = lib.join(staging_path(&title_id, "The.Matrix.(1999).mkv").expect("staging"));
        write_file(&staged, b"original");
        let handle =
            VerifiedStagingHandle::verify(&lib, &title_id, staged, &placement).expect("verify");
        let installed = install(&lib, &title_id, &handle).expect("install");

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
        let err = replace(&lib, &title_id, &converting_handle, &backup).expect_err("backup exists");
        assert!(matches!(err, InstallError::BackupExists(_)));
        assert_eq!(fs::read(&installed).expect("live"), b"original");
        assert_eq!(fs::read(&backup).expect("backup"), b"keep-me");
        assert_eq!(fs::read(&converting).expect("converting"), b"encoded");

        fs::remove_file(&backup).expect("clear backup");
        fs::remove_file(&converting).expect("drop converting");
        let err =
            replace(&lib, &title_id, &converting_handle, &backup).expect_err("converting gone");
        assert_eq!(err.io_kind(), Some(std::io::ErrorKind::NotFound));
        assert_eq!(fs::read(&installed).expect("restored"), b"original");
        assert!(!backup.exists());
    }

    /// Build a verified staging handle for `title_id` under `tmp`.
    fn staged_handle(
        lib: &Path,
        title_id: &TitleId,
        placement: &Placement,
    ) -> VerifiedStagingHandle {
        let name = render(title_id, placement)
            .expect("render")
            .file_name()
            .and_then(|n| n.to_str())
            .expect("name")
            .to_string();
        let staged = lib.join(staging_path(title_id, &name).expect("staging"));
        write_file(&staged, b"bytes");
        VerifiedStagingHandle::verify(lib, title_id, staged, placement).expect("verify")
    }

    #[test]
    fn install_and_replace_refuse_a_handle_built_for_another_title() {
        let tmp = TempTree::new();
        let lib = tmp.path.join("library");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let title_603 = TitleId::movie("603").expect("id");
        let title_604 = TitleId::movie("604").expect("other");

        let handle = staged_handle(&lib, &title_603, &placement);
        assert!(matches!(
            install(&lib, &title_604, &handle),
            Err(InstallError::TitleMismatch)
        ));
        let dest = lib.join(render(&title_603, &placement).expect("render"));
        assert!(
            !dest.exists(),
            "a refused install must not create the schema path"
        );

        let converting = tmp
            .path
            .join("work")
            .join("The.Matrix.(1999).mkv.converting");
        write_file(&converting, b"encoded");
        let converting_handle =
            VerifiedConvertingHandle::verify(&title_603, converting, &placement).expect("verify");
        let backup = tmp.path.join("backup").join("old.mkv");
        assert!(matches!(
            replace(&lib, &title_604, &converting_handle, &backup),
            Err(InstallError::TitleMismatch)
        ));
    }

    #[test]
    fn replace_without_a_live_file_is_missing_live() {
        let tmp = TempTree::new();
        let lib = tmp.path.join("library");
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let converting = tmp
            .path
            .join("work")
            .join("The.Matrix.(1999).mkv.converting");
        write_file(&converting, b"encoded");
        let handle =
            VerifiedConvertingHandle::verify(&title_id, converting, &placement).expect("verify");
        let backup = tmp.path.join("backup").join("old.mkv");
        assert!(matches!(
            replace(&lib, &title_id, &handle, &backup),
            Err(InstallError::MissingLive(_))
        ));

        // A symlink standing in for the live file is not a live file either.
        let dest = lib.join(render(&title_id, &placement).expect("render"));
        fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir");
        let target = tmp.path.join("elsewhere.mkv");
        write_file(&target, b"not-in-library");
        std::os::unix::fs::symlink(&target, &dest).expect("symlink");
        assert!(matches!(
            replace(&lib, &title_id, &handle, &backup),
            Err(InstallError::MissingLive(_))
        ));
        assert_eq!(fs::read(&target).expect("target intact"), b"not-in-library");
    }

    #[test]
    fn verify_on_a_missing_path_is_missing_source() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let lib = tmp.path.join("library");
        let absent = lib.join(staging_path(&title_id, "The.Matrix.(1999).mkv").expect("staging"));
        assert!(matches!(
            VerifiedStagingHandle::verify(&lib, &title_id, absent, &placement),
            Err(InstallError::MissingSource(_))
        ));
    }

    #[test]
    fn unreadable_source_is_not_reported_as_missing() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let lib = tmp.path.join("library");
        let staged = lib.join(staging_path(&title_id, "The.Matrix.(1999).mkv").expect("staging"));
        write_file(&staged, b"bytes");
        let parent = staged.parent().expect("parent").to_path_buf();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o000)).expect("chmod");

        let got = VerifiedStagingHandle::verify(&lib, &title_id, staged, &placement);
        let _ = fs::set_permissions(&parent, fs::Permissions::from_mode(0o755));

        let err = got.expect_err("unreadable");
        assert_eq!(
            err.io_kind(),
            Some(std::io::ErrorKind::PermissionDenied),
            "a permissions failure must not masquerade as `source file not found`, got {err}"
        );
    }

    #[test]
    fn converting_handle_must_name_the_file_it_replaces() {
        let tmp = TempTree::new();
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");

        // Right suffix, wrong title: this would have replaced a live file.
        let unrelated = tmp.path.join("work").join("Some.Other.Encode.converting");
        write_file(&unrelated, b"encoded");
        assert!(matches!(
            VerifiedConvertingHandle::verify(&title_id, unrelated, &placement),
            Err(InstallError::NotConverting(_))
        ));

        // Not a converting file at all.
        let plain = tmp.path.join("work").join("The.Matrix.(1999).mkv");
        write_file(&plain, b"encoded");
        assert!(matches!(
            VerifiedConvertingHandle::verify(&title_id, plain, &placement),
            Err(InstallError::NotConverting(_))
        ));

        // The real shape is accepted.
        let ok = tmp
            .path
            .join("work")
            .join("The.Matrix.(1999).mkv.converting");
        write_file(&ok, b"encoded");
        assert!(VerifiedConvertingHandle::verify(&title_id, ok, &placement).is_ok());
    }

    #[test]
    fn a_symlink_squatting_the_library_path_is_destination_exists() {
        let tmp = TempTree::new();
        let lib = tmp.path.join("library");
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let handle = staged_handle(&lib, &title_id, &placement);

        let dest = lib.join(render(&title_id, &placement).expect("render"));
        fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir");
        // Dangling: `exists()` reports false for this, `symlink_metadata` does not.
        std::os::unix::fs::symlink(tmp.path.join("nowhere"), &dest).expect("symlink");

        assert!(matches!(
            install(&lib, &title_id, &handle),
            Err(InstallError::DestinationExists(_))
        ));
        assert!(
            fs::symlink_metadata(&dest)
                .expect("still there")
                .file_type()
                .is_symlink(),
            "the refused install must not have replaced the entry"
        );
    }

    #[test]
    fn a_backup_inside_the_library_root_is_refused() {
        let tmp = TempTree::new();
        let lib = tmp.path.join("library");
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let handle = staged_handle(&lib, &title_id, &placement);
        let installed = install(&lib, &title_id, &handle).expect("install");

        let converting = tmp
            .path
            .join("work")
            .join("The.Matrix.(1999).mkv.converting");
        write_file(&converting, b"encoded");
        let converting_handle =
            VerifiedConvertingHandle::verify(&title_id, converting, &placement).expect("verify");

        let inside = lib.join("_backup").join("The.Matrix.(1999).mkv");
        assert!(matches!(
            replace(&lib, &title_id, &converting_handle, &inside),
            Err(InstallError::BackupInsideLibrary(_))
        ));
        assert_eq!(fs::read(&installed).expect("live untouched"), b"bytes");
    }
}
