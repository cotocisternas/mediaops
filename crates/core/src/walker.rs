//! Allowlist walker. Sole producer of [`RemoteRef`] and [`RemoteEntry`].
//!
//! Never follows symlinks off the allowlist. Does not list torrent save paths
//! or `torrents/incomplete`. `RemoteRef.rel_path` is relative to its allowlisted
//! root, never an absolute string.

use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
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
    pub(crate) fn new(root_id: String, rel_path: PathBuf) -> Self {
        Self { root_id, rel_path }
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
    pub(crate) fn new(r#ref: RemoteRef, len: u64, mtime: i64, nlink: u64) -> Self {
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

    pub fn ref_(&self) -> &RemoteRef {
        self.r#ref()
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
    #[error("empty allowlist root id")]
    EmptyRootId,
    #[error("io error: {0}")]
    Io(String),
}

impl From<std::io::Error> for WalkerError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

#[derive(Debug, Clone)]
struct AllowlistedRoot {
    id: String,
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
        let canonical = fs::canonicalize(&path)?;
        if !canonical.is_dir() {
            return Err(WalkerError::NotDirectory(path.display().to_string()));
        }
        if self.roots.iter().any(|r| r.canonical == canonical) {
            return Err(WalkerError::DuplicateCanonical(
                canonical.display().to_string(),
            ));
        }
        self.roots.push(AllowlistedRoot {
            id: root_id,
            canonical,
        });
        Ok(())
    }

    /// List files under every allowlisted root as [`RemoteEntry`] values.
    pub fn list(&self) -> Result<Vec<RemoteEntry>, WalkerError> {
        let mut out = Vec::new();
        for root in &self.roots {
            walk_dir(root, &root.canonical, PathBuf::new(), &mut out)?;
        }
        out.sort_by(|a, b| {
            (a.r#ref().root_id(), a.r#ref().rel_path())
                .cmp(&(b.r#ref().root_id(), b.r#ref().rel_path()))
        });
        Ok(out)
    }

    /// Resolve an existing path to a [`RemoteRef`]. Paths outside the allowlist error.
    pub fn resolve(&self, path: &Path) -> Result<RemoteRef, WalkerError> {
        let canonical = match fs::canonicalize(path) {
            Ok(p) => p,
            Err(_) => {
                return Err(WalkerError::UnknownPath(path.display().to_string()));
            }
        };
        self.ref_for_canonical(path, &canonical)
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

fn walk_dir(
    root: &AllowlistedRoot,
    dir: &Path,
    rel: PathBuf,
    out: &mut Vec<RemoteEntry>,
) -> Result<(), WalkerError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let child_rel = rel.join(&name);
        if is_torrent_skip(&child_rel) {
            continue;
        }
        let child_path = entry.path();
        let meta = fs::symlink_metadata(&child_path)?;
        if meta.file_type().is_symlink() {
            // Never follow. Escaped target contents are not listed.
            continue;
        }
        if meta.is_dir() {
            walk_dir(root, &child_path, child_rel, out)?;
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

fn is_torrent_skip(rel: &Path) -> bool {
    let names: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();
    names
        .windows(2)
        .any(|w| w[0] == "torrents" && w[1] == "incomplete")
}

fn mtime_secs(meta: &Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
        assert_eq!(a.ref_().root_id(), "seedbox");
        assert!(!a.r#ref().rel_path().is_absolute());
        assert_eq!(a.len(), 5);
        let meta = fs::metadata(root.join("a.bin")).expect("meta");
        assert_eq!(a.nlink(), nlink(&meta));
        assert_eq!(a.mtime(), mtime_secs(&meta));
        assert_eq!(a.len(), meta.len());

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
}
