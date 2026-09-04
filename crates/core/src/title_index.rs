//! Dual-digest title index (AD-8). Pure types and the repository port.
//!
//! `install_b3` is the reclaim/local-proof digest, written once by
//! [`TitleIndexRepo::record_install`] after a successful [`crate::install::install`].
//! `current_b3` is what `verify` checks: that same call sets it, and only
//! [`TitleIndexRepo::record_replace`] (after encode's [`crate::install::replace`])
//! updates it afterwards. Full-row import keeps a distinct `current_b3`;
//! `record_install` cannot.

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use crate::digest::Blake3Hex;
use crate::title_id::TitleId;

/// One `title_index` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TitleIndexEntry {
    title_id: TitleId,
    /// Library-relative schema path. Empty means a pre-v5 row; callers walk
    /// `movies`/`series`/`music` and [`crate::pathschema::parse`] once.
    path: String,
    install_b3: Blake3Hex,
    current_b3: Blake3Hex,
}

impl TitleIndexEntry {
    pub fn new(
        title_id: TitleId,
        path: impl Into<String>,
        install_b3: Blake3Hex,
        current_b3: Blake3Hex,
    ) -> Self {
        Self {
            title_id,
            path: path.into(),
            install_b3,
            current_b3,
        }
    }

    pub fn title_id(&self) -> &TitleId {
        &self.title_id
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn path_missing(&self) -> bool {
        self.path.is_empty()
    }

    pub fn install_b3(&self) -> &Blake3Hex {
        &self.install_b3
    }

    pub fn current_b3(&self) -> &Blake3Hex {
        &self.current_b3
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TitleIndexError {
    #[error("install_b3 is immutable")]
    InstallDigestImmutable,
    #[error("no title_index row to replace")]
    NotInstalled,
    #[error("title_index is not empty")]
    NotEmpty,
}

/// Rewrite `path` when it is absolute and lives under `old_root`.
///
/// Relative (schema) paths are left alone. `/` and empty roots never match.
/// A `..` remainder does not rewrite (would escape `new_root`).
pub fn rewrite_absolute_under(path: &str, old_root: &str, new_root: &str) -> Option<String> {
    let path_p = Path::new(path);
    if !path_p.is_absolute() {
        return None;
    }
    let old = trim_trailing_slashes(old_root);
    let new = trim_trailing_slashes(new_root);
    if old.is_empty() || old == "/" || new.is_empty() || new == "/" {
        return None;
    }
    let old_p = Path::new(old);
    let new_p = Path::new(new);
    if path_p == old_p {
        return Some(new_p.display().to_string());
    }
    let rest = path_p.strip_prefix(old_p).ok()?;
    if rest.components().any(|c| matches!(c, Component::ParentDir)) {
        return None;
    }
    Some(new_p.join(rest).display().to_string())
}

fn trim_trailing_slashes(s: &str) -> &str {
    if s == "/" { s } else { s.trim_end_matches('/') }
}

/// Persistence door for the install gate. Adapter lives in `store`.
///
/// A trait, not I/O: async signatures only. The filesystem gate does not
/// call this; the composition root does, after `install` / `replace`.
#[allow(async_fn_in_trait)]
pub trait TitleIndexRepo: Send + Sync {
    type Error;

    async fn get(&self, title_id: &TitleId) -> Result<Option<TitleIndexEntry>, Self::Error>;
    async fn list(&self) -> Result<Vec<TitleIndexEntry>, Self::Error>;
    /// First placement: writes `install_b3`, `current_b3`, and the schema path.
    /// `install_b3` never changes.
    async fn record_install(
        &self,
        title_id: &TitleId,
        digest: &Blake3Hex,
        path: &str,
    ) -> Result<(), Self::Error>;
    /// Encode replace: updates `current_b3` only.
    async fn record_replace(
        &self,
        title_id: &TitleId,
        current_b3: &Blake3Hex,
    ) -> Result<(), Self::Error>;
    /// New-machine import: insert full rows including distinct digests.
    /// Refuses when `title_index` is already non-empty.
    async fn import_rows(&self, rows: &[TitleIndexEntry]) -> Result<(), Self::Error>;
    /// Relocate: rewrite absolute paths stored under `old_root`. Relative rows
    /// are unchanged. Returns how many rows were rewritten.
    async fn rewrite_absolute_prefix(
        &self,
        old_root: &str,
        new_root: &str,
    ) -> Result<u64, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(fill: char) -> Blake3Hex {
        Blake3Hex::parse(&fill.to_string().repeat(64)).expect("digest")
    }

    #[test]
    fn entry_stores_distinct_install_and_current() {
        let title = TitleId::movie("603").expect("title");
        let install = digest('a');
        let current = digest('b');
        let entry = TitleIndexEntry::new(
            title.clone(),
            "movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv",
            install.clone(),
            current.clone(),
        );
        assert_eq!(entry.title_id(), &title);
        assert!(!entry.path_missing());
        assert_eq!(entry.install_b3(), &install);
        assert_eq!(entry.current_b3(), &current);
    }

    #[test]
    fn rewrite_leaves_relative_and_foreign_absolute_paths() {
        let rel = "movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv";
        assert_eq!(rewrite_absolute_under(rel, "/data/old", "/data/new"), None);
        assert_eq!(
            rewrite_absolute_under("/other/movies/x.mkv", "/data/old", "/data/new"),
            None
        );
        assert_eq!(
            rewrite_absolute_under("/data/old-backup/x.mkv", "/data/old", "/data/new"),
            None
        );
        assert_eq!(
            rewrite_absolute_under("/data/old/x.mkv", "/", "/data/new"),
            None
        );
        assert_eq!(
            rewrite_absolute_under("/data/old/x.mkv", "", "/data/new"),
            None
        );
        assert_eq!(
            rewrite_absolute_under("/data/old/x.mkv", "/data/old", ""),
            None
        );
        assert_eq!(
            rewrite_absolute_under("/data/old/x.mkv", "/data/old", "/"),
            None
        );
        assert_eq!(
            rewrite_absolute_under("/data/old/foo/../x.mkv", "/data/old", "/data/new"),
            None
        );
    }

    #[test]
    fn rewrite_absolute_under_old_root_uses_new_prefix() {
        let abs = "/data/old/movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv";
        assert_eq!(
            rewrite_absolute_under(abs, "/data/old", "/data/new").as_deref(),
            Some("/data/new/movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv")
        );
        assert_eq!(
            rewrite_absolute_under("/data/old", "/data/old/", "/mnt/lib").as_deref(),
            Some("/mnt/lib")
        );
    }

    #[test]
    fn json_round_trip_keeps_distinct_digests() {
        let entry = TitleIndexEntry::new(
            TitleId::movie("603").expect("title"),
            "movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv",
            digest('a'),
            digest('b'),
        );
        let json = serde_json::to_string(&entry).expect("json");
        assert!(json.contains("\"install_b3\""));
        assert!(json.contains("\"current_b3\""));
        let back: TitleIndexEntry = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, entry);
        assert_ne!(back.install_b3(), back.current_b3());
    }
}
