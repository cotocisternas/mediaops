//! Allowlist walker. Sole producer of [`RemoteRef`] and [`RemoteEntry`].
//!
//! Never follows symlinks off the allowlist. Does not list torrent save paths
//! or `torrents/incomplete`. `RemoteRef.rel_path` is relative to its allowlisted
//! root, never an absolute string.

use std::fs::{self, Metadata};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// Reference to a path under one allowlisted root.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemoteRef {
    root_id: String,
    rel_path: PathBuf,
}

impl RemoteRef {
    /// The walker is the only producer from a filesystem.
    pub(crate) fn new(root_id: String, rel_path: PathBuf) -> Self {
        Self { root_id, rel_path }
    }

    /// Rebuild a ref that crossed the wire.
    ///
    /// A remote ref was already produced by a walker on the far side and cannot
    /// be re-checked against a local filesystem, so this enforces the invariants
    /// a receiver *can* check: a non-empty `root_id` and a relative `rel_path`
    /// with no `..`, root, or prefix component. Story 1.4's
    /// `TryFrom<wire::RemoteRef>` lives in `proto` and calls this.
    pub fn from_wire_parts(root_id: String, rel_path: PathBuf) -> Result<Self, WalkerError> {
        if root_id.is_empty() {
            return Err(WalkerError::EmptyRootId);
        }
        if rel_path.as_os_str().is_empty() || !is_contained_relative(&rel_path) {
            return Err(WalkerError::UnknownPath(rel_path.display().to_string()));
        }
        Ok(Self { root_id, rel_path })
    }

    pub fn root_id(&self) -> &str {
        &self.root_id
    }

    pub fn rel_path(&self) -> &Path {
        &self.rel_path
    }
}

/// Listing entry produced only by the walker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    r#ref: RemoteRef,
    len: u64,
    mtime: i64,
    nlink: u64,
}

impl RemoteEntry {
    /// The walker is the only producer from a filesystem.
    pub(crate) fn new(r#ref: RemoteRef, len: u64, mtime: i64, nlink: u64) -> Self {
        Self {
            r#ref,
            len,
            mtime,
            nlink,
        }
    }

    /// Rebuild an entry that crossed the wire.
    ///
    /// The invariant lives on [`RemoteRef::from_wire_parts`]; once the ref is
    /// validated the metadata fields are plain data.
    pub fn from_wire_parts(r#ref: RemoteRef, len: u64, mtime: i64, nlink: u64) -> Self {
        Self {
            r#ref,
            len,
            mtime,
            nlink,
        }
    }

    pub fn r#ref(&self) -> &RemoteRef {
        &self.r#ref
    }

    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn mtime(&self) -> i64 {
        self.mtime
    }

    pub fn nlink(&self) -> u64 {
        self.nlink
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WalkerError {
    #[error("unknown path `{0}` is outside the allowlist")]
    UnknownPath(String),
    #[error("allowlist root `{0}` is not a directory")]
    NotDirectory(String),
    #[error("duplicate allowlist root id `{0}`")]
    DuplicateRoot(String),
    #[error("duplicate allowlist canonical path `{0}`")]
    DuplicateCanonical(String),
    #[error("allowlist root `{root}` is nested inside `{existing}`")]
    NestedRoot { root: String, existing: String },
    #[error("empty allowlist root id")]
    EmptyRootId,
    /// Carries the failing path and the `ErrorKind`, so callers can tell
    /// `EXDEV` from `ENOSPC` from `EACCES` instead of string-matching.
    #[error("io error at `{path}`: {message}")]
    Io {
        path: String,
        kind: std::io::ErrorKind,
        message: String,
    },
}

impl WalkerError {
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

/// A subtree or entry `list_partial` could not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedPath {
    pub path: PathBuf,
    pub error: WalkerError,
}

/// True when `rel` is relative and stays inside its root.
fn is_contained_relative(rel: &Path) -> bool {
    !rel.is_absolute()
        && rel.components().all(|c| {
            !matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

#[derive(Debug, Clone)]
struct AllowlistedRoot {
    id: String,
    /// The path as the caller supplied it, used to judge whether a caller's own
    /// path lies inside the allowlist before any symlink resolution.
    original: PathBuf,
    canonical: PathBuf,
}

/// Caller-supplied filesystem roots the walker may list.
#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    roots: Vec<AllowlistedRoot>,
}

impl Allowlist {
    pub fn new() -> Self {
        Self { roots: Vec::new() }
    }

    pub fn add_root(
        &mut self,
        root_id: impl Into<String>,
        path: PathBuf,
    ) -> Result<(), WalkerError> {
        let root_id = root_id.into();
        if root_id.is_empty() {
            return Err(WalkerError::EmptyRootId);
        }
        if self.roots.iter().any(|r| r.id == root_id) {
            return Err(WalkerError::DuplicateRoot(root_id));
        }
        let canonical =
            fs::canonicalize(&path).map_err(|err| WalkerError::io(path.as_path(), &err))?;
        if !canonical.is_dir() {
            return Err(WalkerError::NotDirectory(path.display().to_string()));
        }
        if self.roots.iter().any(|r| r.canonical == canonical) {
            return Err(WalkerError::DuplicateCanonical(
                canonical.display().to_string(),
            ));
        }
        // A nested root would list the same physical file twice under two
        // root_ids and make `resolve` insertion-order dependent.
        if let Some(existing) = self
            .roots
            .iter()
            .find(|r| canonical.starts_with(&r.canonical) || r.canonical.starts_with(&canonical))
        {
            return Err(WalkerError::NestedRoot {
                root: canonical.display().to_string(),
                existing: existing.canonical.display().to_string(),
            });
        }
        self.roots.push(AllowlistedRoot {
            id: root_id,
            original: path,
            canonical,
        });
        Ok(())
    }

    /// List files under every allowlisted root as [`RemoteEntry`] values.
    ///
    /// Strict: an unreadable directory or entry is an error naming the path, so
    /// a caller never mistakes a partial listing for a complete one. Use
    /// [`Self::list_partial`] to survive unreadable subtrees deliberately.
    ///
    /// Entries are sorted by `(root_id, rel_path)`; the order is part of the
    /// contract, since later stories diff listings against each other.
    pub fn list(&self) -> Result<Vec<RemoteEntry>, WalkerError> {
        let mut out = Vec::new();
        for root in &self.roots {
            walk_dir(root, &root.canonical, PathBuf::new(), &mut out, None)?;
        }
        sort_entries(&mut out);
        Ok(out)
    }

    /// List what is readable, reporting every subtree that was skipped.
    ///
    /// One `EACCES` directory on a remote box should not cost the whole listing,
    /// but a silently short listing would drive deletions in later set-diff
    /// stories -- so what was skipped is returned, never swallowed.
    pub fn list_partial(&self) -> (Vec<RemoteEntry>, Vec<SkippedPath>) {
        let mut out = Vec::new();
        let mut skipped = Vec::new();
        for root in &self.roots {
            let _ = walk_dir(
                root,
                &root.canonical,
                PathBuf::new(),
                &mut out,
                Some(&mut skipped),
            );
        }
        sort_entries(&mut out);
        (out, skipped)
    }

    /// Resolve an existing path to a [`RemoteRef`]. Paths outside the allowlist error.
    ///
    /// The caller's own path must lie inside an allowlisted root *before* any
    /// symlink resolution: a symlink sitting outside the allowlist that points
    /// in is an unknown path, not a shortcut into the typed world. Listing
    /// exclusions apply here too, so `resolve` cannot mint a ref for something
    /// [`Self::list`] refuses to emit.
    pub fn resolve(&self, path: &Path) -> Result<RemoteRef, WalkerError> {
        let unknown = || WalkerError::UnknownPath(path.display().to_string());
        if path.components().any(|c| c == Component::ParentDir) {
            return Err(unknown());
        }
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            let cwd = std::env::current_dir().map_err(|err| WalkerError::io(path, &err))?;
            cwd.join(path)
        };
        if !self
            .roots
            .iter()
            .any(|r| absolute.starts_with(&r.canonical) || absolute.starts_with(&r.original))
        {
            return Err(unknown());
        }
        let canonical = fs::canonicalize(path).map_err(|_| unknown())?;
        let remote_ref = self.ref_for_canonical(path, &canonical)?;
        if is_torrent_skip(&canonical) {
            return Err(unknown());
        }
        Ok(remote_ref)
    }

    /// Resolve a [`RemoteRef`] to a path under an allowlisted root.
    pub fn absolute(&self, remote: &RemoteRef) -> Result<PathBuf, WalkerError> {
        let root = self
            .roots
            .iter()
            .find(|r| r.id == remote.root_id())
            .ok_or_else(|| WalkerError::UnknownPath(remote.root_id().to_string()))?;
        if !is_contained_relative(remote.rel_path()) {
            return Err(WalkerError::UnknownPath(
                remote.rel_path().display().to_string(),
            ));
        }
        let path = root.canonical.join(remote.rel_path());
        if !path.starts_with(&root.canonical) {
            return Err(WalkerError::UnknownPath(path.display().to_string()));
        }
        Ok(path)
    }

    /// Stat a file that a prior walker listing produced.
    pub fn entry(&self, remote: &RemoteRef) -> Result<RemoteEntry, WalkerError> {
        let path = self.absolute(remote)?;
        if is_torrent_skip(&path) {
            return Err(WalkerError::UnknownPath(path.display().to_string()));
        }
        let meta = fs::symlink_metadata(&path)
            .map_err(|_| WalkerError::UnknownPath(path.display().to_string()))?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(WalkerError::UnknownPath(path.display().to_string()));
        }
        Ok(RemoteEntry::from_wire_parts(
            remote.clone(),
            meta.len(),
            mtime_secs(&meta),
            nlink(&meta),
        ))
    }

    /// Open a file for a range read. Never follows a symlink.
    pub fn open(&self, remote: &RemoteRef) -> Result<std::fs::File, WalkerError> {
        let path = self.absolute(remote)?;
        let _ = self.entry(remote)?;
        std::fs::File::open(&path).map_err(|err| WalkerError::io(&path, &err))
    }

    /// Least available bytes among allowlisted roots (`df`).
    pub fn free_bytes(&self) -> Result<u64, WalkerError> {
        if self.roots.is_empty() {
            return Ok(0);
        }
        let mut min = u64::MAX;
        for root in &self.roots {
            min = min.min(statvfs_available(&root.canonical)?);
        }
        Ok(min)
    }

    fn ref_for_canonical(
        &self,
        original: &Path,
        canonical: &Path,
    ) -> Result<RemoteRef, WalkerError> {
        for root in &self.roots {
            if let Ok(rel) = canonical.strip_prefix(&root.canonical) {
                return Ok(RemoteRef::new(root.id.clone(), rel.to_path_buf()));
            }
        }
        Err(WalkerError::UnknownPath(original.display().to_string()))
    }
}

fn sort_entries(entries: &mut [RemoteEntry]) {
    entries.sort_by(|a, b| {
        (a.r#ref().root_id(), a.r#ref().rel_path())
            .cmp(&(b.r#ref().root_id(), b.r#ref().rel_path()))
    });
}

/// Record a failure, or propagate it when the caller asked for a strict listing.
fn note(
    skipped: &mut Option<&mut Vec<SkippedPath>>,
    path: &Path,
    err: WalkerError,
) -> Result<(), WalkerError> {
    match skipped {
        Some(list) => {
            list.push(SkippedPath {
                path: path.to_path_buf(),
                error: err,
            });
            Ok(())
        }
        None => Err(err),
    }
}

fn walk_dir(
    root: &AllowlistedRoot,
    dir: &Path,
    rel: PathBuf,
    out: &mut Vec<RemoteEntry>,
    mut skipped: Option<&mut Vec<SkippedPath>>,
) -> Result<(), WalkerError> {
    let reader = match fs::read_dir(dir) {
        Ok(reader) => reader,
        Err(err) => return note(&mut skipped, dir, WalkerError::io(dir, &err)),
    };
    for entry in reader {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                note(&mut skipped, dir, WalkerError::io(dir, &err))?;
                continue;
            }
        };
        let child_rel = rel.join(entry.file_name());
        let child_path = entry.path();
        if is_torrent_skip(&child_path) {
            continue;
        }
        let meta = match fs::symlink_metadata(&child_path) {
            Ok(meta) => meta,
            Err(err) => {
                note(
                    &mut skipped,
                    &child_path,
                    WalkerError::io(&child_path, &err),
                )?;
                continue;
            }
        };
        if meta.file_type().is_symlink() {
            // Never follow. Escaped target contents are not listed.
            continue;
        }
        if meta.is_dir() {
            walk_dir(root, &child_path, child_rel, out, skipped.as_deref_mut())?;
        } else if meta.is_file() {
            out.push(RemoteEntry::new(
                RemoteRef::new(root.id.clone(), child_rel),
                meta.len(),
                mtime_secs(&meta),
                nlink(&meta),
            ));
        }
    }
    Ok(())
}

/// True for anything under a `torrents/incomplete` directory.
///
/// This takes the *absolute* path, not the root-relative one: allowlisting the
/// torrents directory itself is an ordinary seedbox layout, and a relative path
/// of `incomplete/…` has no `torrents` component left to pair with.
fn is_torrent_skip(path: &Path) -> bool {
    let names: Vec<&str> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    names
        .windows(2)
        .any(|w| w[0] == "torrents" && w[1] == "incomplete")
}

/// Seconds since the Unix epoch, negative for pre-1970 timestamps.
///
/// `duration_since(UNIX_EPOCH)` fails for earlier times, so the sign has to be
/// recovered from the error rather than collapsed into `0` -- otherwise a real
/// 1969 mtime, an epoch mtime, and an unreadable one are indistinguishable.
fn mtime_secs(meta: &Metadata) -> i64 {
    let Ok(modified) = meta.modified() else {
        return 0;
    };
    match modified.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(err) => -(err.duration().as_secs() as i64),
    }
}

fn nlink(meta: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        meta.nlink()
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        1
    }
}

fn statvfs_available(path: &Path) -> Result<u64, WalkerError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let cstr = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            WalkerError::UnknownPath(path.display().to_string())
        })?;
        let mut vfs = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        let rc = unsafe { libc::statvfs(cstr.as_ptr(), vfs.as_mut_ptr()) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            return Err(WalkerError::io(path, &err));
        }
        let vfs = unsafe { vfs.assume_init() };
        Ok(vfs.f_bavail as u64 * vfs.f_frsize as u64)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(WalkerError::UnknownPath(
            "df is unix-only in v1".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempTree {
        path: PathBuf,
    }

    impl TempTree {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mediaops-walker-{}-{}-{}",
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
            fs::create_dir_all(parent).expect("mkdir parent");
        }
        let mut f = File::create(path).expect("create");
        f.write_all(bytes).expect("write");
    }

    #[test]
    fn walker_happy_lists_only_typed_entries_under_allowlist() {
        let tmp = TempTree::new();
        let root = tmp.path.join("media");
        write_file(&root.join("a.bin"), b"hello");
        write_file(&root.join("nested").join("b.bin"), b"world!!");

        let mut allowlist = Allowlist::new();
        allowlist.add_root("seedbox", root.clone()).expect("root");
        let entries = allowlist.list().expect("list");
        assert_eq!(entries.len(), 2);

        let a = entries
            .iter()
            .find(|e| e.r#ref().rel_path() == Path::new("a.bin"))
            .expect("a.bin");
        assert_eq!(a.r#ref().root_id(), "seedbox");
        assert!(!a.r#ref().rel_path().is_absolute());
        assert_eq!(a.len(), 5);
        let meta = fs::metadata(root.join("a.bin")).expect("meta");
        assert_eq!(a.len(), meta.len());
        // Derived without `mtime_secs`/`nlink`, so replacing either helper with a
        // constant fails here instead of passing on both sides.
        let expected_mtime = meta
            .modified()
            .expect("modified")
            .duration_since(UNIX_EPOCH)
            .expect("post-epoch")
            .as_secs() as i64;
        assert_eq!(a.mtime(), expected_mtime);
        assert!(
            a.mtime() > 1_600_000_000,
            "a file written during this test cannot have a 1970 mtime, got {}",
            a.mtime()
        );
        assert_eq!(a.nlink(), 1, "a freshly written file has exactly one link");

        let resolved = allowlist.resolve(&root.join("a.bin")).expect("resolve");
        assert_eq!(resolved.root_id(), a.r#ref().root_id());
        assert_eq!(resolved.rel_path(), a.r#ref().rel_path());

        let b = entries
            .iter()
            .find(|e| e.r#ref().rel_path() == Path::new("nested/b.bin"))
            .expect("nested");
        assert_eq!(b.len(), 7);
        assert_eq!(b.r#ref().root_id(), "seedbox");
        let resolved_b = allowlist
            .resolve(&root.join("nested").join("b.bin"))
            .expect("resolve nested");
        assert_eq!(resolved_b.root_id(), b.r#ref().root_id());
        assert_eq!(resolved_b.rel_path(), b.r#ref().rel_path());
    }

    #[test]
    fn unknown_path_errors_and_does_not_return_an_entry() {
        let tmp = TempTree::new();
        let allowed = tmp.path.join("allowed");
        let outside = tmp.path.join("outside");
        fs::create_dir_all(&allowed).expect("allowed");
        write_file(&outside.join("secret.bin"), b"nope");

        let mut allowlist = Allowlist::new();
        allowlist.add_root("in", allowed).expect("root");
        let err = allowlist
            .resolve(&outside.join("secret.bin"))
            .expect_err("unknown");
        assert!(matches!(err, WalkerError::UnknownPath(_)));

        let listed = allowlist.list().expect("list");
        assert!(listed.is_empty());
        assert!(
            !listed
                .iter()
                .any(|e| e.r#ref().rel_path().ends_with("secret.bin"))
        );
    }

    #[test]
    fn symlink_escape_is_not_followed() {
        let tmp = TempTree::new();
        let allowed = tmp.path.join("allowed");
        let outside = tmp.path.join("outside");
        fs::create_dir_all(&allowed).expect("allowed");
        write_file(&outside.join("secret.bin"), b"escaped");
        std::os::unix::fs::symlink(&outside, allowed.join("escape")).expect("symlink");
        write_file(&allowed.join("ok.bin"), b"ok");

        let mut allowlist = Allowlist::new();
        allowlist.add_root("in", allowed.clone()).expect("root");
        let entries = allowlist.list().expect("list");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].r#ref().rel_path(), PathBuf::from("ok.bin"));
        assert!(
            !entries
                .iter()
                .any(|e| e.r#ref().rel_path().ends_with("secret.bin"))
        );
        assert!(allowlist.resolve(&outside.join("secret.bin")).is_err());
        assert!(allowlist.resolve(&allowed.join("escape")).is_err());
    }

    #[test]
    fn torrent_incomplete_and_non_allowlisted_save_dir_are_not_listed() {
        let tmp = TempTree::new();
        let media = tmp.path.join("media");
        let torrent_save = tmp.path.join("torrent-save");
        write_file(&media.join("keep.bin"), b"keep");
        write_file(
            &media.join("torrents").join("incomplete").join("dl.bin"),
            b"dl",
        );
        write_file(&torrent_save.join("save.bin"), b"save");

        let mut allowlist = Allowlist::new();
        allowlist.add_root("media", media).expect("media root");

        let entries = allowlist.list().expect("list");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].r#ref().root_id(), "media");
        assert_eq!(entries[0].r#ref().rel_path(), PathBuf::from("keep.bin"));
        assert!(
            !entries
                .iter()
                .any(|e| e.r#ref().rel_path().ends_with("dl.bin"))
        );
        assert!(
            !entries
                .iter()
                .any(|e| e.r#ref().rel_path().ends_with("save.bin"))
        );
        assert!(allowlist.resolve(&torrent_save.join("save.bin")).is_err());
    }

    #[test]
    fn add_root_rejects_duplicate_canonical_paths() {
        let tmp = TempTree::new();
        let real = tmp.path.join("real");
        fs::create_dir_all(&real).expect("mkdir");
        let link = tmp.path.join("alias");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let mut allowlist = Allowlist::new();
        allowlist.add_root("a", real).expect("first");
        let err = allowlist.add_root("b", link).expect_err("dup canonical");
        assert!(matches!(err, WalkerError::DuplicateCanonical(_)));
    }

    #[test]
    fn torrents_incomplete_is_skipped_even_when_the_root_is_the_torrents_dir() {
        let tmp = TempTree::new();
        let torrents = tmp.path.join("torrents");
        write_file(&torrents.join("incomplete").join("part.bin"), b"x");
        write_file(&torrents.join("done.bin"), b"y");

        let mut allowlist = Allowlist::new();
        allowlist.add_root("t", torrents.clone()).expect("root");
        let rels: Vec<String> = allowlist
            .list()
            .expect("list")
            .iter()
            .map(|e| e.r#ref().rel_path().display().to_string())
            .collect();
        assert_eq!(rels, vec!["done.bin".to_string()]);
    }

    #[test]
    fn resolve_refuses_to_mint_a_ref_the_listing_refuses_to_emit() {
        let tmp = TempTree::new();
        let media = tmp.path.join("media");
        let partial = media.join("torrents").join("incomplete").join("dl.bin");
        write_file(&partial, b"partial");
        let mut allowlist = Allowlist::new();
        allowlist.add_root("media", media.clone()).expect("root");

        assert!(allowlist.list().expect("list").is_empty());
        assert!(
            matches!(
                allowlist.resolve(&partial),
                Err(WalkerError::UnknownPath(_))
            ),
            "resolve must share the listing exclusions"
        );
    }

    #[test]
    fn resolve_refuses_an_outside_symlink_that_points_into_the_allowlist() {
        let tmp = TempTree::new();
        let root = tmp.path.join("media");
        write_file(&root.join("a.bin"), b"hello");
        let outside = tmp.path.join("outside");
        fs::create_dir_all(&outside).expect("mkdir");
        let link = outside.join("link.bin");
        std::os::unix::fs::symlink(root.join("a.bin"), &link).expect("symlink");

        let mut allowlist = Allowlist::new();
        allowlist.add_root("media", root.clone()).expect("root");
        assert!(matches!(
            allowlist.resolve(&link),
            Err(WalkerError::UnknownPath(_))
        ));
        // A `..` escape hatch is refused before the filesystem is consulted.
        assert!(matches!(
            allowlist.resolve(&root.join("..").join("outside").join("link.bin")),
            Err(WalkerError::UnknownPath(_))
        ));
        // The ordinary path still resolves.
        assert!(allowlist.resolve(&root.join("a.bin")).is_ok());
    }

    #[test]
    fn nested_allowlist_roots_are_refused() {
        let tmp = TempTree::new();
        let outer = tmp.path.join("outer");
        let inner = outer.join("inner");
        write_file(&inner.join("f.bin"), b"x");

        let mut allowlist = Allowlist::new();
        allowlist.add_root("outer", outer.clone()).expect("outer");
        assert!(matches!(
            allowlist.add_root("inner", inner.clone()),
            Err(WalkerError::NestedRoot { .. })
        ));
        // ... and in the other order.
        let mut reversed = Allowlist::new();
        reversed.add_root("inner", inner).expect("inner");
        assert!(matches!(
            reversed.add_root("outer", outer),
            Err(WalkerError::NestedRoot { .. })
        ));
        // One physical file, one entry.
        assert_eq!(allowlist.list().expect("list").len(), 1);
    }

    #[test]
    fn unreadable_subdir_fails_strict_list_and_is_reported_by_list_partial() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempTree::new();
        let root = tmp.path.join("media");
        write_file(&root.join("good.bin"), b"x");
        let locked = root.join("locked");
        fs::create_dir_all(&locked).expect("mkdir");
        write_file(&locked.join("hidden.bin"), b"y");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod");

        let mut allowlist = Allowlist::new();
        allowlist.add_root("seedbox", root.clone()).expect("root");

        // Strict listing refuses to look complete, and names the path.
        let err = allowlist.list().expect_err("strict");
        assert_eq!(err.io_kind(), Some(std::io::ErrorKind::PermissionDenied));
        assert!(
            matches!(&err, WalkerError::Io { path, .. } if path.contains("locked")),
            "the error must name the failing path, got {err}"
        );

        // Partial listing survives, and says exactly what it could not read.
        let (entries, skipped) = allowlist.list_partial();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].r#ref().rel_path(), Path::new("good.bin"));
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].path.ends_with("locked"));

        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
    }

    #[test]
    fn list_order_is_stable_and_sorted() {
        let tmp = TempTree::new();
        let root = tmp.path.join("media");
        for name in ["z.bin", "a.bin", "m.bin"] {
            write_file(&root.join(name), b"x");
        }
        write_file(&root.join("nested").join("b.bin"), b"x");
        let mut allowlist = Allowlist::new();
        allowlist.add_root("seedbox", root).expect("root");

        let rels: Vec<String> = allowlist
            .list()
            .expect("list")
            .iter()
            .map(|e| e.r#ref().rel_path().display().to_string())
            .collect();
        let mut expected = rels.clone();
        expected.sort();
        assert_eq!(rels, expected, "listing order is part of the contract");
        assert_eq!(
            rels,
            vec![
                "a.bin".to_string(),
                "m.bin".to_string(),
                "nested/b.bin".to_string(),
                "z.bin".to_string()
            ]
        );
    }

    #[test]
    fn pre_epoch_mtime_keeps_its_sign() {
        use std::time::Duration;
        let tmp = TempTree::new();
        let root = tmp.path.join("media");
        let old = root.join("old.bin");
        write_file(&old, b"x");
        let stamp = UNIX_EPOCH - Duration::from_secs(86_400);
        let times = fs::FileTimes::new().set_modified(stamp).set_accessed(stamp);
        File::options()
            .write(true)
            .open(&old)
            .expect("open")
            .set_times(times)
            .expect("set_times");

        let mut allowlist = Allowlist::new();
        allowlist.add_root("seedbox", root).expect("root");
        let entries = allowlist.list().expect("list");
        assert_eq!(
            entries[0].mtime(),
            -86_400,
            "a 1969 mtime must not be reported as the epoch"
        );
    }

    #[test]
    fn from_wire_parts_is_the_only_other_door_and_it_validates() {
        // Story 1.4 rebuilds refs that crossed the wire; the walker cannot
        // re-check them against a local filesystem, so the shape is checked.
        let ok = RemoteRef::from_wire_parts("seedbox".into(), PathBuf::from("a/b.bin"))
            .expect("valid wire ref");
        assert_eq!(ok.root_id(), "seedbox");
        assert_eq!(ok.rel_path(), Path::new("a/b.bin"));

        let entry = RemoteEntry::from_wire_parts(ok, 5, -1, 2);
        assert_eq!(entry.len(), 5);
        assert_eq!(entry.mtime(), -1);
        assert_eq!(entry.nlink(), 2);

        assert!(matches!(
            RemoteRef::from_wire_parts(String::new(), PathBuf::from("a.bin")),
            Err(WalkerError::EmptyRootId)
        ));
        for bad in ["/etc/passwd", "../../etc/passwd", "a/../../b", ""] {
            assert!(
                matches!(
                    RemoteRef::from_wire_parts("seedbox".into(), PathBuf::from(bad)),
                    Err(WalkerError::UnknownPath(_))
                ),
                "{bad} must not survive the wire boundary"
            );
        }
    }

    #[test]
    fn entry_and_open_round_trip_a_listed_file() {
        let tmp = TempTree::new();
        let root = tmp.path.join("media");
        write_file(&root.join("a.bin"), b"hello");
        let mut allowlist = Allowlist::new();
        allowlist.add_root("seedbox", root).expect("root");
        let listed = allowlist.list().expect("list");
        assert_eq!(listed.len(), 1);
        let entry = allowlist.entry(listed[0].r#ref()).expect("entry");
        assert_eq!(entry.len(), 5);
        let mut file = allowlist.open(listed[0].r#ref()).expect("open");
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut buf).expect("read");
        assert_eq!(buf, b"hello");
        assert!(allowlist.free_bytes().expect("df") > 0);
    }
}
