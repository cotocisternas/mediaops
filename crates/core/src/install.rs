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
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Instant;

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

/// Verified file under `_incoming/<TitleId::staging_token()>/…`.
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
    #[error("staged bytes no longer match the durable verification digest")]
    DigestMismatch,
    #[error("pull deadline reached before installation completed")]
    DeadlineExceeded,
    #[error("refusing an unowned or unsafe install temporary: {0}")]
    UnsafeTemporary(String),
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
        check_title(title_id, &dest_rel)?;
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
        check_title(title_id, &dest_rel)?;
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

/// The destination must belong to `title_id`. A path carries only its `key`
/// identity, so a key TitleId must parse back exactly; an *arr authority id
/// (tmdb/tvdb/mbid) can only be held to the kind the path encodes.
fn check_title(title_id: &TitleId, dest_rel: &Path) -> Result<(), InstallError> {
    let parsed = pathschema::parse(dest_rel)?;
    let matches = if title_id.is_key() {
        parsed == *title_id
    } else {
        parsed.kind() == title_id.kind()
    };
    if matches {
        Ok(())
    } else {
        Err(InstallError::TitleMismatch)
    }
}

fn backup_in_schema_dir(library_root: &Path, backup: &Path) -> bool {
    ["movies", "series", "music"]
        .iter()
        .any(|dir| is_under(&library_root.join(dir), backup))
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
        let cstr =
            std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| InstallError::Io {
                path: path.display().to_string(),
                kind: std::io::ErrorKind::InvalidInput,
                message: "path contains NUL".into(),
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

/// Bytes still needing allocation on the staging filesystem. Logical length
/// from set_len is sparse and must never be counted as allocated disk space.
pub fn pull_remaining_bytes(spec: &crate::home::JobSpec) -> Result<u64, InstallError> {
    use std::os::unix::fs::MetadataExt;
    let id = TitleId::parse(&spec.title_id).map_err(|e| InstallError::NotStaging(e.to_string()))?;
    let name = Path::new(&spec.dest_rel)
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| InstallError::NotStaging(spec.dest_rel.clone()))?;
    let staged = Path::new(&spec.library_root).join(pathschema::staging_path(&id, name)?);
    let mut partial = staged.clone();
    partial.as_mut_os_string().push(".partial");
    for path in [staged, partial] {
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() => {
                return Ok(spec
                    .file_len
                    .saturating_sub(meta.blocks().saturating_mul(512)));
            }
            Ok(_) => return Err(InstallError::NotStaging(path.display().to_string())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(InstallError::io(&path, &err)),
        }
    }
    Ok(spec.file_len)
}

/// Check the final filesystem too when a schema directory links another disk.
pub fn install_fits(spec: &crate::home::JobSpec) -> Result<bool, InstallError> {
    use std::os::unix::fs::MetadataExt;
    let root = Path::new(&spec.library_root);
    let dest = root.join(&spec.dest_rel);
    let mut parent = dest
        .parent()
        .ok_or_else(|| InstallError::NotStaging(spec.dest_rel.clone()))?;
    loop {
        match fs::metadata(parent) {
            Ok(meta) => {
                let root_meta = fs::metadata(root).map_err(|e| InstallError::io(root, &e))?;
                if meta.dev() == root_meta.dev() {
                    return Ok(true);
                }
                return Ok(crate::home::pull_fits(
                    free_bytes(parent)?,
                    spec.min_free,
                    0,
                    0,
                    spec.file_len,
                ));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                parent = parent
                    .parent()
                    .ok_or_else(|| InstallError::io(parent, &err))?;
            }
            Err(err) => return Err(InstallError::io(parent, &err)),
        }
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
    install_checked(
        library_root.as_ref(),
        title_id,
        handle,
        None,
        &mut || Ok(()),
    )
}

/// Home worker gate: recheck the persisted whole-file digest before publishing.
pub fn install_verified(
    library_root: impl AsRef<Path>,
    title_id: &TitleId,
    handle: &VerifiedStagingHandle,
    expected: &Blake3Hex,
) -> Result<InstallOutcome, InstallError> {
    install_checked(
        library_root.as_ref(),
        title_id,
        handle,
        Some(expected),
        &mut || Ok(()),
    )
}

/// Fresh installation is bounded by the Job's persisted remaining budget.
/// Each filesystem call is synchronous; no cancelled task can publish later.
/// Checks surround each bounded chunk and precede the atomic publication.
pub fn install_verified_before(
    library_root: impl AsRef<Path>,
    title_id: &TitleId,
    handle: &VerifiedStagingHandle,
    expected: &Blake3Hex,
    deadline: Instant,
) -> Result<InstallOutcome, InstallError> {
    install_checked(
        library_root.as_ref(),
        title_id,
        handle,
        Some(expected),
        &mut || {
            if Instant::now() >= deadline {
                Err(InstallError::DeadlineExceeded)
            } else {
                Ok(())
            }
        },
    )
}

fn install_checked(
    library_root: &Path,
    title_id: &TitleId,
    handle: &VerifiedStagingHandle,
    expected: Option<&Blake3Hex>,
    check: &mut impl FnMut() -> Result<(), InstallError>,
) -> Result<InstallOutcome, InstallError> {
    check()?;
    let dest = library_root.join(&handle.dest_rel);
    check_title(title_id, &handle.dest_rel)?;
    // `exists()` follows symlinks, so a dangling symlink squatting the library
    // path would report `false`. `symlink_metadata` sees the entry itself.
    match fs::symlink_metadata(&dest) {
        Ok(_) => return Err(InstallError::DestinationExists(dest.display().to_string())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(InstallError::io(&dest, &err)),
    }
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&handle.source)
        .map_err(|err| InstallError::io(&handle.source, &err))?;
    if !file
        .metadata()
        .map_err(|e| InstallError::io(&handle.source, &e))?
        .is_file()
    {
        return Err(InstallError::NotStaging(
            handle.source.display().to_string(),
        ));
    }
    let mut hasher = blake3::Hasher::new();
    read_chunks(&mut file, &handle.source, check, |bytes| {
        hasher.update(bytes);
        Ok(())
    })?;
    let whole_file_b3 =
        Blake3Hex::parse(&hasher.finalize().to_hex()).expect("BLAKE3 generates a canonical digest");
    if expected.is_some_and(|digest| *digest != whole_file_b3) {
        return Err(InstallError::DigestMismatch);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| InstallError::io(parent, &err))?;
    }
    move_into_place_checked(&handle.source, &dest, check)?;
    Ok(InstallOutcome {
        path: dest,
        whole_file_b3,
    })
}

/// Atomically link a complete file into a previously absent destination.
/// A music symlink may cross devices: copy to an owned temp on that device,
/// fsync, then link without replacing any existing directory entry.
#[cfg(test)]
fn move_into_place(source: &Path, dest: &Path) -> Result<(), InstallError> {
    move_into_place_checked(source, dest, &mut || Ok(()))
}

fn move_into_place_checked(
    source: &Path,
    dest: &Path,
    check: &mut impl FnMut() -> Result<(), InstallError>,
) -> Result<(), InstallError> {
    copy_into_place_checked(source, dest, check)?;
    fs::remove_file(source).map_err(|err| InstallError::io(source, &err))?;
    sync_parent(source)
}

fn copy_into_place(source: &Path, dest: &Path) -> Result<(), InstallError> {
    copy_into_place_checked(source, dest, &mut || Ok(()))
}

fn copy_into_place_checked(
    source: &Path,
    dest: &Path,
    check: &mut impl FnMut() -> Result<(), InstallError>,
) -> Result<(), InstallError> {
    check()?;
    match fs::hard_link(source, dest) {
        Ok(()) => return sync_parent(dest),
        Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(InstallError::DestinationExists(dest.display().to_string()));
        }
        Err(err) => return Err(InstallError::io(source, &err)),
    }
    copy_across_devices(source, dest, check)
}

fn copy_across_devices(
    source: &Path,
    dest: &Path,
    check: &mut impl FnMut() -> Result<(), InstallError>,
) -> Result<(), InstallError> {
    let slot = InstallTemporary::open(source, dest, true)?.expect("created install slot");
    slot.clear_data()?;
    check()?;
    let mut from = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(source)
        .map_err(|e| InstallError::io(source, &e))?;
    if !from
        .metadata()
        .map_err(|e| InstallError::io(source, &e))?
        .is_file()
    {
        return Err(InstallError::NotStaging(source.display().to_string()));
    }
    let mut to = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&slot.data)
        .map_err(|e| InstallError::io(&slot.data, &e))?;
    // On failure or process death, one explicitly owned data file remains.
    // The next attempt clears it before capacity checks; random or unmarked
    // files are never searched for or removed.
    read_chunks(&mut from, source, check, |bytes| {
        to.write_all(bytes)
            .map_err(|e| InstallError::io(&slot.data, &e))
    })?;
    to.sync_all()
        .map_err(|e| InstallError::io(&slot.data, &e))?;
    check()?;
    fs::hard_link(&slot.data, dest).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            InstallError::DestinationExists(dest.display().to_string())
        } else {
            InstallError::io(dest, &e)
        }
    })?;
    slot.clear_data()?;
    sync_parent(dest)
}

fn read_chunks(
    reader: &mut impl Read,
    path: &Path,
    check: &mut impl FnMut() -> Result<(), InstallError>,
    mut consume: impl FnMut(&[u8]) -> Result<(), InstallError>,
) -> Result<(), InstallError> {
    let mut bytes = [0; 64 * 1024];
    loop {
        check()?;
        let len = reader
            .read(&mut bytes)
            .map_err(|e| InstallError::io(path, &e))?;
        check()?;
        if len == 0 {
            return Ok(());
        }
        consume(&bytes[..len])?;
        check()?;
    }
}

/// Remove only a previous attempt's explicitly owned cross-device data file.
/// Call before disk-capacity checks so an interrupted copy cannot permanently
/// consume the space needed to retry. The small identity/lock slot is retained.
pub fn cleanup_install_temporary(spec: &crate::home::JobSpec) -> Result<(), InstallError> {
    let id = TitleId::parse(&spec.title_id).map_err(|e| InstallError::NotStaging(e.to_string()))?;
    let relative = Path::new(&spec.dest_rel);
    check_title(&id, relative)?;
    let name = relative
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| InstallError::NotStaging(spec.dest_rel.clone()))?;
    let root = Path::new(&spec.library_root);
    let source = root.join(pathschema::staging_path(&id, name)?);
    let dest = root.join(relative);
    if let Some(slot) = InstallTemporary::open(&source, &dest, false)? {
        slot.clear_data()?;
    }
    Ok(())
}

struct InstallTemporary {
    data: PathBuf,
    dest: PathBuf,
    // The marker doubles as a process-lifetime lock, including during cleanup.
    _owner: fs::File,
}

impl InstallTemporary {
    fn identity(source: &Path, dest: &Path) -> Vec<u8> {
        use std::os::unix::ffi::OsStrExt;
        let mut identity = b"mediaops-install-v1\0".to_vec();
        identity.extend_from_slice(source.as_os_str().as_bytes());
        identity.push(0);
        identity.extend_from_slice(dest.as_os_str().as_bytes());
        identity
    }

    fn directory(source: &Path, dest: &Path) -> PathBuf {
        let digest = Blake3Hex::of_bytes(&Self::identity(source, dest));
        dest.with_file_name(format!(".mediaops-install-{digest}"))
    }

    fn open(source: &Path, dest: &Path, create: bool) -> Result<Option<Self>, InstallError> {
        let dir = Self::directory(source, dest);
        let created = if create {
            match fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => true,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
                Err(e) => return Err(InstallError::io(&dir, &e)),
            }
        } else {
            false
        };
        let directory = match fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&dir)
        {
            Ok(file) => file,
            Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(InstallError::io(&dir, &e)),
        };
        let metadata = directory
            .metadata()
            .map_err(|e| InstallError::io(&dir, &e))?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(InstallError::UnsafeTemporary(dir.display().to_string()));
        }
        let marker = dir.join("owner");
        let mut owner = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(created)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&marker)
            .map_err(|e| InstallError::io(&marker, &e))?;
        let metadata = owner
            .metadata()
            .map_err(|e| InstallError::io(&marker, &e))?;
        if !owned_regular(&metadata) || metadata.nlink() != 1 {
            return Err(InstallError::UnsafeTemporary(marker.display().to_string()));
        }
        owner.try_lock().map_err(|e| {
            InstallError::UnsafeTemporary(format!("{} is busy: {e}", dir.display()))
        })?;
        let identity = Self::identity(source, dest);
        if created {
            owner
                .write_all(&identity)
                .map_err(|e| InstallError::io(&marker, &e))?;
            owner
                .sync_all()
                .map_err(|e| InstallError::io(&marker, &e))?;
            directory
                .sync_all()
                .map_err(|e| InstallError::io(&dir, &e))?;
            sync_parent(&dir)?;
        } else {
            let mut saved = Vec::new();
            (&mut owner)
                .take(identity.len() as u64 + 1)
                .read_to_end(&mut saved)
                .map_err(|e| InstallError::io(&marker, &e))?;
            if saved != identity {
                return Err(InstallError::UnsafeTemporary(marker.display().to_string()));
            }
        }
        Ok(Some(Self {
            data: dir.join("data"),
            dest: dest.into(),
            _owner: owner,
        }))
    }

    fn clear_data(&self) -> Result<(), InstallError> {
        let metadata = match fs::symlink_metadata(&self.data) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(InstallError::io(&self.data, &e)),
        };
        // A second link is ours only in the crash window after publication.
        let links_are_owned = metadata.nlink() == 1
            || (metadata.nlink() == 2
                && fs::symlink_metadata(&self.dest).is_ok_and(|dest| {
                    dest.dev() == metadata.dev() && dest.ino() == metadata.ino()
                }));
        if !owned_regular(&metadata) || !links_are_owned {
            return Err(InstallError::UnsafeTemporary(
                self.data.display().to_string(),
            ));
        }
        fs::remove_file(&self.data).map_err(|e| InstallError::io(&self.data, &e))?;
        sync_parent(&self.data)
    }
}

fn owned_regular(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
        && metadata.uid() == unsafe { libc::geteuid() }
        && metadata.mode() & 0o077 == 0
}

fn sync_parent(path: &Path) -> Result<(), InstallError> {
    if let Some(parent) = path.parent() {
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|err| InstallError::io(parent, &err))?;
    }
    Ok(())
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
    check_title(title_id, &handle.dest_rel)?;
    let backup_destination = backup_destination.as_ref();
    // Schema dirs are the only library writers. `_ops/` (encode backups) is
    // under the library root but is not a schema library dir.
    if backup_in_schema_dir(library_root, backup_destination) {
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
    // Preserve the live entry until the encoded file can replace it atomically.
    copy_into_place(&dest, backup_destination)?;
    if let Err(err) = fs::rename(&handle.source, &dest) {
        // The live entry never moved. Discard only the duplicate this call
        // created, allowing a corrected conversion to retry normally.
        fs::remove_file(backup_destination).map_err(|cleanup| InstallError::RollbackFailed {
            cause: err.to_string(),
            restore: cleanup.to_string(),
            live_now_at: dest.display().to_string(),
        })?;
        return Err(InstallError::io(&handle.source, &err));
    }
    sync_parent(&dest)?;
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

    #[test]
    fn concurrent_no_replace_gate_has_one_winner_and_preserves_loser() {
        let tmp = TempTree::new();
        let first = tmp.path.join("first");
        let second = tmp.path.join("second");
        let dest = tmp.path.join("destination");
        write_file(&first, b"first");
        write_file(&second, b"second");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let outcomes = std::thread::scope(|scope| {
            let one = scope.spawn(|| {
                barrier.wait();
                move_into_place(&first, &dest)
            });
            let two = scope.spawn(|| {
                barrier.wait();
                move_into_place(&second, &dest)
            });
            [one.join().expect("first"), two.join().expect("second")]
        });
        assert_eq!(outcomes.iter().filter(|o| o.is_ok()).count(), 1);
        if outcomes[0].is_ok() {
            assert_eq!(fs::read(&dest).expect("dest"), b"first");
            assert_eq!(fs::read(&second).expect("loser preserved"), b"second");
        } else {
            assert_eq!(fs::read(&dest).expect("dest"), b"second");
            assert_eq!(fs::read(&first).expect("loser preserved"), b"first");
        }
    }

    #[test]
    fn deadline_during_hash_and_before_publication_preserves_staging() {
        let tmp = TempTree::new();
        let id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let staged = tmp
            .path
            .join(staging_path(&id, "The.Matrix.(1999).mkv").expect("stage"));
        let bytes = vec![7; 3 * 64 * 1024];
        write_file(&staged, &bytes);
        let handle = VerifiedStagingHandle::verify(&tmp.path, &id, staged.clone(), &placement)
            .expect("handle");
        let proof = Blake3Hex::of_bytes(&bytes);
        assert_eq!(
            install_verified_before(&tmp.path, &id, &handle, &proof, Instant::now()),
            Err(InstallError::DeadlineExceeded)
        );
        // Deterministically expire between chunks, without wall-clock sleeps.
        let mut checks = 0;
        let result = install_checked(&tmp.path, &id, &handle, Some(&proof), &mut || {
            checks += 1;
            if checks == 5 {
                Err(InstallError::DeadlineExceeded)
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err(InstallError::DeadlineExceeded));
        assert_eq!(fs::read(&staged).expect("source"), bytes);
        assert!(!tmp.path.join(handle.dest_rel()).exists());

        // Empty-source hashing ends after these checks; expire at publication.
        write_file(&staged, b"");
        let proof = Blake3Hex::of_bytes(b"");
        checks = 0;
        assert_eq!(
            install_checked(&tmp.path, &id, &handle, Some(&proof), &mut || {
                checks += 1;
                if checks == 4 {
                    Err(InstallError::DeadlineExceeded)
                } else {
                    Ok(())
                }
            }),
            Err(InstallError::DeadlineExceeded)
        );
        assert!(!tmp.path.join(handle.dest_rel()).exists());
        assert!(staged.is_file());
    }

    #[test]
    fn cross_device_copy_expiry_leaves_one_owned_recoverable_partial() {
        let tmp = TempTree::new();
        let source = tmp.path.join("source");
        let dest = tmp.path.join("destination");
        write_file(&source, &vec![9; 3 * 64 * 1024]);
        let mut checks = 0;
        let result = copy_across_devices(&source, &dest, &mut || {
            checks += 1;
            if checks == 5 {
                Err(InstallError::DeadlineExceeded)
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err(InstallError::DeadlineExceeded));
        assert!(!dest.exists());
        let slot = InstallTemporary::open(&source, &dest, false)
            .expect("owned slot")
            .expect("present");
        assert_eq!(fs::metadata(&slot.data).expect("partial").len(), 64 * 1024);
        let data = slot.data.clone();
        slot.clear_data().expect("recover space");
        assert!(!data.exists());
        drop(slot);
        copy_across_devices(&source, &dest, &mut || Ok(())).expect("retry");
        assert_eq!(
            fs::read(&dest).expect("dest"),
            fs::read(&source).expect("source")
        );
        assert!(!data.exists());
    }

    #[test]
    fn job_cleanup_recovers_interrupted_copy_without_removing_unknown_files() {
        let tmp = TempTree::new();
        let id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let relative = render(&id, &placement).expect("schema");
        let source = tmp
            .path
            .join(staging_path(&id, "The.Matrix.(1999).mkv").expect("stage"));
        let dest = tmp.path.join(&relative);
        fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir");
        let spec = crate::home::JobSpec {
            title_id: id.render(),
            dest_rel: relative.display().to_string(),
            library_root: tmp.path.display().to_string(),
            file_len: 128 * 1024,
            ..crate::home::JobSpec::default()
        };
        let slot = InstallTemporary::open(&source, &dest, true)
            .expect("slot")
            .expect("created");
        let mut partial = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&slot.data)
            .expect("data");
        partial
            .write_all(&vec![8; 128 * 1024])
            .expect("allocated partial");
        partial.sync_all().expect("sync");
        let data = slot.data.clone();
        drop(partial);
        drop(slot); // Same on-disk state as process death before publication.
        cleanup_install_temporary(&spec).expect("recover before capacity check");
        assert!(!data.exists());

        // A symlink or a file without the slot's ownership proof is not ours.
        let precious = tmp.path.join("precious");
        write_file(&precious, b"keep");
        std::os::unix::fs::symlink(&precious, &data).expect("symlink collision");
        assert!(cleanup_install_temporary(&spec).is_err());
        assert_eq!(fs::read(&precious).expect("preserved"), b"keep");
        fs::remove_file(&data).expect("remove test symlink");
        let marker = data.parent().expect("slot").join("owner");
        fs::write(&marker, b"unknown user data").expect("foreign marker");
        write_file(&data, b"untouched");
        assert!(cleanup_install_temporary(&spec).is_err());
        assert_eq!(fs::read(&data).expect("unknown preserved"), b"untouched");
    }

    #[test]
    fn published_temp_cleanup_preserves_the_installed_hard_link() {
        let tmp = TempTree::new();
        let source = tmp.path.join("source");
        let dest = tmp.path.join("destination");
        let slot = InstallTemporary::open(&source, &dest, true)
            .expect("slot")
            .expect("created");
        let mut data = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&slot.data)
            .expect("data");
        data.write_all(b"verified").expect("write");
        fs::hard_link(&slot.data, &dest).expect("publication");
        drop(slot);
        let slot = InstallTemporary::open(&source, &dest, false)
            .expect("reopen")
            .expect("slot");
        slot.clear_data().expect("post-publication cleanup");
        assert_eq!(fs::read(dest).expect("installed preserved"), b"verified");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn music_symlink_install_crosses_filesystems_without_overwriting() {
        let library = TempTree::new();
        let music = TempTree::new_at(Path::new("/dev/shm"));
        assert_ne!(
            fs::metadata(&library.path).expect("library disk").dev(),
            fs::metadata(&music.path).expect("music disk").dev(),
            "fixture must cross devices"
        );
        std::os::unix::fs::symlink(&music.path, library.path.join("music")).expect("music link");
        let id = TitleId::album_key("Yes", "Relayer").expect("id");
        let placement = Placement::track(
            "Yes",
            "Relayer",
            1974,
            None,
            Some(1),
            "Sound.Chaser",
            "flac",
        );
        let relative = render(&id, &placement).expect("schema");
        let filename = relative
            .file_name()
            .expect("filename")
            .to_str()
            .expect("utf8");
        let source = library
            .path
            .join(staging_path(&id, filename).expect("stage"));
        write_file(&source, b"verified music");
        let handle = VerifiedStagingHandle::verify(&library.path, &id, source.clone(), &placement)
            .expect("handle");
        let result = install_verified_before(
            &library.path,
            &id,
            &handle,
            &Blake3Hex::of_bytes(b"verified music"),
            Instant::now() + std::time::Duration::from_secs(5),
        )
        .expect("cross-device install");
        assert_eq!(fs::read(&result.path).expect("music"), b"verified music");
        assert_eq!(
            fs::metadata(&result.path).expect("installed disk").dev(),
            fs::metadata(&music.path).expect("music disk").dev()
        );
        assert!(!source.exists());
        let slot = InstallTemporary::open(&source, &result.path, false)
            .expect("slot")
            .expect("owned");
        assert!(!slot.data.exists());
        drop(slot);
        write_file(&source, b"replacement");
        assert!(matches!(
            install(&library.path, &id, &handle),
            Err(InstallError::DestinationExists(_))
        ));
        assert_eq!(fs::read(&result.path).expect("original"), b"verified music");
        assert_eq!(
            fs::read(&source).expect("replacement retained"),
            b"replacement"
        );
    }

    #[test]
    fn persisted_digest_is_checked_before_installing_changed_staging() {
        let tmp = TempTree::new();
        let id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let staged = tmp
            .path
            .join(staging_path(&id, "The.Matrix.(1999).mkv").expect("stage"));
        write_file(&staged, b"before");
        let handle = VerifiedStagingHandle::verify(&tmp.path, &id, staged.clone(), &placement)
            .expect("handle");
        let proof = Blake3Hex::of_bytes(b"before");
        write_file(&staged, b"after!");
        assert!(matches!(
            install_verified(&tmp.path, &id, &handle, &proof),
            Err(InstallError::DigestMismatch)
        ));
        assert!(
            !tmp.path
                .join(render(&id, &placement).expect("dest"))
                .exists()
        );
        assert_eq!(fs::read(staged).expect("staged preserved"), b"after!");
    }

    #[test]
    fn resume_budget_counts_allocated_blocks_not_sparse_length() {
        use std::os::unix::fs::MetadataExt;
        let tmp = TempTree::new();
        let id = TitleId::movie("603").expect("id");
        let rel = render(&id, &Placement::movie("The.Matrix", 1999, "mkv")).expect("render");
        let mut partial = tmp
            .path
            .join(staging_path(&id, "The.Matrix.(1999).mkv").expect("stage"));
        partial.as_mut_os_string().push(".partial");
        fs::create_dir_all(partial.parent().expect("parent")).expect("dir");
        let mut file = fs::File::create(&partial).expect("partial");
        let len = 8 * crate::Bytes::MIB;
        file.set_len(len).expect("sparse length");
        let spec = crate::home::JobSpec {
            title_id: id.render(),
            dest_rel: rel.display().to_string(),
            file_len: len,
            library_root: tmp.path.display().to_string(),
            ..crate::home::JobSpec::default()
        };
        assert_eq!(
            pull_remaining_bytes(&spec).expect("remaining"),
            len.saturating_sub(file.metadata().expect("metadata").blocks() * 512)
        );
        file.write_all(&[7; 4096]).expect("write");
        file.sync_all().expect("sync");
        let remaining = pull_remaining_bytes(&spec).expect("remaining");
        assert!(
            remaining < len && remaining > 0,
            "only allocated blocks count"
        );
    }

    struct TempTree {
        path: PathBuf,
    }

    impl TempTree {
        fn new() -> Self {
            Self::new_at(&std::env::temp_dir())
        }

        fn new_at(base: &Path) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!(
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
        assert_eq!(
            installed.whole_file_b3,
            crate::Blake3Hex::of_bytes(b"matrix-bytes")
        );
        assert!(!staged.exists());
        // A path only carries its key identity; the tmdb id is metadata.
        assert_eq!(
            pathschema::parse(installed.path.strip_prefix(&lib).expect("strip")).expect("parse"),
            TitleId::movie_key("The.Matrix", 1999).expect("key")
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
            TitleId::movie_key("The.Matrix", 1999).expect("key")
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
        // Key ids are what a path can verify; a different key is another title.
        let title_603 = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let title_604 = TitleId::movie_key("The.Matrix.Reloaded", 2003).expect("other");
        // An authority id can only be held to its kind.
        assert!(matches!(
            install(
                &lib,
                &TitleId::series("79126").expect("series"),
                &staged_handle(&lib, &TitleId::movie("603").expect("tmdb"), &placement)
            ),
            Err(InstallError::TitleMismatch)
        ));

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

        let inside = lib
            .join("movies")
            .join("The.Matrix.(1999).{tmdb-603}")
            .join("backup.mkv");
        assert!(matches!(
            replace(&lib, &title_id, &converting_handle, &inside),
            Err(InstallError::BackupInsideLibrary(_))
        ));
        assert_eq!(fs::read(&installed).expect("live untouched"), b"bytes");
    }

    #[test]
    fn backup_under_ops_is_legal_and_movies_is_not() {
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
        let backup = lib
            .join("_ops")
            .join("backup-hevc-originals")
            .join("movie-tmdb-603")
            .join("The.Matrix.(1999).mkv");
        let replaced = replace(&lib, &title_id, &converting_handle, &backup).expect("ops backup");
        assert_eq!(replaced, installed.path);
        assert_eq!(fs::read(&replaced).expect("new"), b"encoded");
        assert_eq!(fs::read(&backup).expect("backup"), b"bytes");
    }
}
