//! Reclaim policy: surplus ranking and qBit path matching.
//!
//! Constraints (private-under-goal, seeding, nlink, install proof) times a
//! ranked dry-run. Goal ratio is a constant, not DesiredState. Size/mtime is
//! never proof.

use std::collections::HashSet;
use std::path::{Component, Path};

use serde::Serialize;

use crate::pathschema::{parse_placement, render_placement};
use crate::plan::{Action, RootKinds, classify_remote};
use crate::title_id::TitleId;
use crate::title_index::{InstalledFile, TitleIndexEntry};
use crate::walker::{RemoteEntry, RemoteRef};

/// Private-tracker ratio below which a torrent is untouched.
pub struct ReclaimPolicy;

impl ReclaimPolicy {
    pub const GOAL_RATIO: f64 = 1.0;

    pub fn private_under_goal(is_private: bool, ratio: f64) -> bool {
        is_private && ratio < Self::GOAL_RATIO
    }

    /// True when qBit `state` is seeding or not a known non-seed (fail-closed).
    pub fn is_seeding(state: &str) -> bool {
        !matches!(
            state,
            "downloading"
                | "pausedDL"
                | "queuedDL"
                | "stalledDL"
                | "stoppedDL"
                | "metaDL"
                | "forcedDL"
                | "checkingDL"
                | "allocating"
                | "checkingResumeData"
                | "error"
                | "missingFiles"
        )
    }

    pub fn blocks_delete(is_private: bool, ratio: f64, state: &str) -> bool {
        Self::is_seeding(state) || Self::private_under_goal(is_private, ratio)
    }
}

/// One qBit `torrents/info` row, plus an optional allowlisted remote.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GuardPreviewItem {
    pub hash: String,
    pub state: String,
    pub ratio: f64,
    pub is_private: bool,
    pub content_path: String,
    pub save_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteRef>,
}

impl GuardPreviewItem {
    pub fn matches_remote(&self, remote: &RemoteRef) -> bool {
        self.guards(remote, None)
    }

    pub fn covers_path(&self, absolute: &Path) -> bool {
        torrent_covers_file(&self.content_path, &self.save_path, absolute)
    }

    /// Same match preview and DeleteRemote use so the guard cannot miss a cover.
    pub fn guards(&self, remote: &RemoteRef, absolute: Option<&Path>) -> bool {
        if self.remote.as_ref() == Some(remote) {
            return true;
        }
        if let Some(path) = absolute
            && self.covers_path(path)
        {
            return true;
        }
        torrent_covers_file(&self.content_path, &self.save_path, remote.rel_path())
    }

    pub fn blocks_delete(&self) -> bool {
        ReclaimPolicy::blocks_delete(self.is_private, self.ratio, &self.state)
    }
}

/// Ranked surplus candidate. `ratio`/`is_private` are None for usenet (age-only).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReclaimCandidate {
    pub title_id: TitleId,
    pub remote: RemoteRef,
    pub len: u64,
    pub mtime: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_private: Option<bool>,
}

/// True when `file` is `root` or lives under `root` on a component boundary.
///
/// `/` and empty never cover. A directory `content_path` covers nested files.
/// `ends_with` is only a PathSchema-looking relative tail, not a shared dir name.
pub fn torrent_covers_file(content_path: &str, save_path: &str, file: &Path) -> bool {
    covers_one(content_path, file) || covers_one(save_path, file)
}

fn covers_one(root_s: &str, file: &Path) -> bool {
    if root_s.is_empty() {
        return false;
    }
    let root = Path::new(root_s);
    if is_fs_root(root) {
        return false;
    }
    if file == root {
        return true;
    }
    if is_strict_under(file, root) {
        return true;
    }
    parse_placement(file).is_ok() && root.ends_with(file)
}

fn is_fs_root(path: &Path) -> bool {
    let mut saw_root = false;
    for c in path.components() {
        match c {
            Component::RootDir | Component::Prefix(_) => saw_root = true,
            Component::CurDir => {}
            _ => return false,
        }
    }
    saw_root
}

fn is_strict_under(file: &Path, root: &Path) -> bool {
    let root_comps: Vec<_> = root.components().collect();
    let file_comps: Vec<_> = file.components().collect();
    if root_comps.is_empty() || file_comps.len() <= root_comps.len() {
        return false;
    }
    let normals = root_comps
        .iter()
        .filter(|c| matches!(c, Component::Normal(_)))
        .count();
    if normals < 2 {
        return false;
    }
    file_comps.starts_with(&root_comps)
}

/// Proved surplus: the remote file's schema path has an install digest in the
/// index **and** that exact file is still on disk. nlink==1. No qBit filter.
///
/// The remote is root-relative; its local twin is `render_placement` of what
/// it parses to. A row whose path is missing (pre-v5) counts by TitleId only
/// when the title is a single file (movie).
pub fn reclaim_proved(
    listings: &[RemoteEntry],
    root_kinds: &RootKinds,
    title_index: &[TitleIndexEntry],
    on_disk: &[InstalledFile],
) -> Vec<ReclaimCandidate> {
    let on_disk_paths: HashSet<&str> = on_disk.iter().map(|f| f.path.as_str()).collect();
    let mut out = Vec::new();
    for entry in listings {
        if entry.nlink() > 1 {
            continue;
        }
        let Ok((title_id, placement)) = classify_remote(root_kinds, entry) else {
            continue;
        };
        let Ok(local) = render_placement(&placement) else {
            continue;
        };
        let local = local.to_string_lossy().into_owned();
        if !on_disk_paths.contains(local.as_str()) {
            continue;
        }
        let indexed = title_index.iter().any(|row| {
            row.path() == local
                || (row.path_missing()
                    && row.title_id() == &title_id
                    && placement.file_key() == crate::pathschema::FileKey::Whole)
        });
        if !indexed {
            continue;
        }
        out.push(ReclaimCandidate {
            title_id,
            remote: entry.r#ref().clone(),
            len: entry.len(),
            mtime: entry.mtime(),
            ratio: None,
            is_private: None,
        });
    }
    out
}

/// Surplus remotes with install_b3 ∩ on-disk, nlink==1, not private-under-goal,
/// not seeding. Ranked public / low-ratio / older first; usenet is age-only.
pub fn reclaim_preview(
    listings: &[RemoteEntry],
    root_kinds: &RootKinds,
    title_index: &[TitleIndexEntry],
    on_disk: &[InstalledFile],
    torrents: &[GuardPreviewItem],
) -> Vec<ReclaimCandidate> {
    let mut out = Vec::new();
    for mut candidate in reclaim_proved(listings, root_kinds, title_index, on_disk) {
        let covering: Vec<&GuardPreviewItem> = torrents
            .iter()
            .filter(|t| t.matches_remote(&candidate.remote))
            .collect();
        if covering.iter().any(|t| t.blocks_delete()) {
            continue;
        }
        // One torrent owns both rank fields so the key is not a chimera.
        if let Some(best) = covering.iter().max_by(|a, b| {
            (u8::from(a.is_private), OrderedF64(a.ratio))
                .cmp(&(u8::from(b.is_private), OrderedF64(b.ratio)))
        }) {
            candidate.ratio = Some(best.ratio);
            candidate.is_private = Some(best.is_private);
        }
        out.push(candidate);
    }
    out.sort_by_key(rank_key);
    out
}

/// `DeleteRemote { remote }` for each ranked candidate. Not used by `run`.
pub fn reclaim_actions(
    listings: &[RemoteEntry],
    root_kinds: &RootKinds,
    title_index: &[TitleIndexEntry],
    on_disk: &[InstalledFile],
    torrents: &[GuardPreviewItem],
) -> Vec<Action> {
    reclaim_preview(listings, root_kinds, title_index, on_disk, torrents)
        .into_iter()
        .map(|c| Action::DeleteRemote { remote: c.remote })
        .collect()
}

fn rank_key(c: &ReclaimCandidate) -> (u8, OrderedF64, i64) {
    let private = u8::from(c.is_private.unwrap_or(false));
    let ratio = OrderedF64(c.ratio.unwrap_or(0.0));
    (private, ratio, c.mtime)
}

#[derive(Clone, Copy, PartialEq)]
struct OrderedF64(f64);

impl Eq for OrderedF64 {}

impl PartialOrd for OrderedF64 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedF64 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::Blake3Hex;
    use crate::title_index::TitleIndexEntry;
    use std::path::PathBuf;

    fn digest() -> Blake3Hex {
        Blake3Hex::parse(&"a".repeat(64)).expect("digest")
    }

    fn entry(rel: &str, mtime: i64, nlink: u64) -> RemoteEntry {
        let remote = RemoteRef::from_wire_parts("seedbox".into(), PathBuf::from(rel)).expect("ref");
        RemoteEntry::from_wire_parts(remote, 10, mtime, nlink)
    }

    fn kinds() -> RootKinds {
        RootKinds::from([(
            "seedbox".to_string(),
            Some(crate::title_id::TitleKind::Movie),
        )])
    }

    fn movie(name: &str) -> TitleId {
        TitleId::movie_key(name, 1999).expect("title")
    }

    /// Root-relative remote path for a movie named `name`.
    fn remote(name: &str) -> String {
        format!("{name}.(1999)/{name}.(1999).mkv")
    }

    /// Library-relative local path for the same movie.
    fn local(name: &str) -> String {
        format!("movies/{}", remote(name))
    }

    fn index_row(name: &str) -> TitleIndexEntry {
        TitleIndexEntry::new(movie(name), local(name), digest(), digest())
    }

    fn on_disk_file(name: &str) -> InstalledFile {
        InstalledFile::from_rel_path(Path::new(&local(name))).expect("installed")
    }

    fn torrent(rel: &str, state: &str, ratio: f64, is_private: bool) -> GuardPreviewItem {
        GuardPreviewItem {
            hash: format!("h-{rel}"),
            state: state.into(),
            ratio,
            is_private,
            content_path: rel.into(),
            save_path: String::new(),
            remote: None,
        }
    }

    #[test]
    fn ranks_public_low_ratio_older_first_and_usenet_by_age() {
        let names = [
            "Public.Old",
            "Public.New",
            "High.Ratio",
            "Private.Over",
            "Private.Under",
            "Usenet.Old",
            "Usenet.New",
            "Hardlink",
            "NoDigest",
            "Seeding",
        ];
        let listings = vec![
            entry(&remote("Public.New"), 200, 1),
            entry(&remote("High.Ratio"), 50, 1),
            entry(&remote("Private.Over"), 10, 1),
            entry(&remote("Private.Under"), 1, 1),
            entry(&remote("Usenet.New"), 300, 1),
            entry(&remote("Usenet.Old"), 20, 1),
            entry(&remote("Hardlink"), 5, 2),
            entry(&remote("NoDigest"), 5, 1),
            entry(&remote("Seeding"), 5, 1),
            entry(&remote("Public.Old"), 100, 1),
        ];
        let title_index: Vec<_> = names
            .iter()
            .filter(|n| **n != "NoDigest")
            .map(|n| index_row(n))
            .collect();
        let on_disk: Vec<_> = names.iter().map(|n| on_disk_file(n)).collect();
        let torrents = vec![
            torrent(&remote("Public.Old"), "pausedDL", 0.1, false),
            torrent(&remote("Public.New"), "pausedDL", 0.1, false),
            torrent(&remote("High.Ratio"), "pausedDL", 2.0, false),
            torrent(&remote("Private.Over"), "pausedDL", 1.5, true),
            torrent(&remote("Private.Under"), "pausedDL", 0.2, true),
            torrent(&remote("Seeding"), "uploading", 2.0, false),
        ];

        let ranked = reclaim_preview(&listings, &kinds(), &title_index, &on_disk, &torrents);
        let ids: Vec<String> = ranked.iter().map(|c| c.title_id.render()).collect();
        // public/low-ratio/older first; usenet is age-only (ratio treated as 0).
        assert_eq!(
            ids,
            vec![
                "movie:key:usenetold.1999",
                "movie:key:usenetnew.1999",
                "movie:key:publicold.1999",
                "movie:key:publicnew.1999",
                "movie:key:highratio.1999",
                "movie:key:privateover.1999",
            ],
            "{ids:?}"
        );
        assert!(
            !ids.iter().any(|id| id.contains("privateunder")
                || id.contains("hardlink")
                || id.contains("nodigest")
                || id.contains("seeding")),
            "private-under-goal, hardlink, no-digest, seeding must be omitted: {ids:?}"
        );
        assert_eq!(ranked[0].ratio, None, "usenet is age-only");
        assert_eq!(ranked[1].ratio, None);
        assert_eq!(ranked[0].mtime, 20);
        assert_eq!(ranked[1].mtime, 300);
        assert_eq!(ranked[2].ratio, Some(0.1));
        assert_eq!(ranked[2].mtime, 100);
        assert_eq!(ranked[3].mtime, 200);
        assert!(ranked.iter().any(|c| c.is_private == Some(true)));
    }

    #[test]
    fn size_mtime_without_install_b3_is_not_proof() {
        let listings = vec![entry(&remote("Only.Mtime"), 1, 1)];
        let on_disk = vec![on_disk_file("Only.Mtime")];
        let ranked = reclaim_preview(&listings, &kinds(), &[], &on_disk, &[]);
        assert!(ranked.is_empty(), "no title_index row means no delete");
    }

    #[test]
    fn install_b3_without_on_disk_is_not_proof() {
        let listings = vec![entry(&remote("Index.Only"), 1, 1)];
        let title_index = vec![index_row("Index.Only")];
        let ranked = reclaim_preview(&listings, &kinds(), &title_index, &[], &[]);
        assert!(ranked.is_empty());
    }

    #[test]
    fn episodes_prove_per_file_not_per_show() {
        let kinds = RootKinds::from([(
            "seedbox".to_string(),
            Some(crate::title_id::TitleKind::Series),
        )]);
        let e1 = "Silo.(2023)/Season.01/Silo.(2023).S01E01.mkv";
        let e2 = "Silo.(2023)/Season.01/Silo.(2023).S01E02.mkv";
        let listings = vec![entry(e1, 1, 1), entry(e2, 1, 1)];
        let local_e1 = format!("series/{e1}");
        let title_index = vec![TitleIndexEntry::new(
            TitleId::series_key("Silo", 2023).expect("id"),
            &local_e1,
            digest(),
            digest(),
        )];
        let on_disk = vec![InstalledFile::from_rel_path(Path::new(&local_e1)).expect("file")];
        let ranked = reclaim_preview(&listings, &kinds, &title_index, &on_disk, &[]);
        assert_eq!(ranked.len(), 1, "only the installed episode is surplus");
        assert_eq!(ranked[0].remote.rel_path(), Path::new(e1));
    }

    #[test]
    fn actions_are_delete_remote_payloads_in_rank_order() {
        let listings = vec![entry(&remote("Usenet"), 1, 1)];
        let title_index = vec![index_row("Usenet")];
        let on_disk = vec![on_disk_file("Usenet")];
        let actions = reclaim_actions(&listings, &kinds(), &title_index, &on_disk, &[]);
        assert_eq!(
            actions,
            vec![Action::DeleteRemote {
                remote: listings[0].r#ref().clone()
            }]
        );
    }

    #[test]
    fn seeding_and_private_under_goal_helpers() {
        assert!(ReclaimPolicy::is_seeding("uploading"));
        assert!(ReclaimPolicy::is_seeding("pausedUP"));
        assert!(ReclaimPolicy::is_seeding("stoppedUP"));
        assert!(ReclaimPolicy::is_seeding("unknown-state"));
        assert!(!ReclaimPolicy::is_seeding("downloading"));
        assert!(!ReclaimPolicy::is_seeding("pausedDL"));
        assert!(!ReclaimPolicy::is_seeding("stoppedDL"));
        assert!(ReclaimPolicy::private_under_goal(true, 0.99));
        assert!(!ReclaimPolicy::private_under_goal(true, 1.0));
        assert!(!ReclaimPolicy::private_under_goal(false, 0.0));
        assert_eq!(ReclaimPolicy::GOAL_RATIO, 1.0);
    }

    #[test]
    fn torrent_path_match_is_component_suffix_or_prefix() {
        let file = Path::new("movies/The.Matrix.(1999)/The.Matrix.(1999).mkv");
        assert!(torrent_covers_file(
            "/data/media/movies/The.Matrix.(1999)/The.Matrix.(1999).mkv",
            "/data/torrents",
            file
        ));
        assert!(torrent_covers_file(
            "/data/media/movies/The.Matrix.(1999)",
            "",
            Path::new("/data/media/movies/The.Matrix.(1999)/The.Matrix.(1999).mkv")
        ));
        assert!(!torrent_covers_file(
            "/data/torrents/other",
            "/data/torrents",
            Path::new("/data/media/movies/The.Matrix.(1999)/The.Matrix.(1999).mkv")
        ));
        let library = Path::new("/data/media/movies/The.Matrix.(1999)/The.Matrix.(1999).mkv");
        assert!(
            !torrent_covers_file("", "/", library),
            "/ save_path must not cover library files"
        );
        assert!(!torrent_covers_file("/", "", library));
    }

    #[test]
    fn any_covering_seeding_or_private_under_goal_omits_the_file() {
        let rel = remote("Shared");
        let listings = vec![entry(&rel, 1, 1)];
        let title_index = vec![index_row("Shared")];
        let on_disk = vec![on_disk_file("Shared")];
        let paused = torrent(&rel, "pausedDL", 0.1, false);
        let seeding = GuardPreviewItem {
            hash: "seed".into(),
            state: "uploading".into(),
            ratio: 2.0,
            is_private: false,
            content_path: rel.clone(),
            save_path: String::new(),
            remote: None,
        };
        let ranked = reclaim_preview(
            &listings,
            &kinds(),
            &title_index,
            &on_disk,
            &[paused.clone(), seeding],
        );
        assert!(
            ranked.is_empty(),
            "a seeding cover must omit even if another torrent is paused"
        );
        let private_under = GuardPreviewItem {
            hash: "priv".into(),
            state: "pausedDL".into(),
            ratio: 0.2,
            is_private: true,
            content_path: rel,
            save_path: String::new(),
            remote: None,
        };
        let ranked = reclaim_preview(
            &listings,
            &kinds(),
            &title_index,
            &on_disk,
            &[paused, private_under],
        );
        assert!(ranked.is_empty(), "private-under-goal cover must omit");
    }

    #[test]
    fn extra_remote_of_a_proved_title_is_not_a_candidate() {
        let installed = remote("The.Matrix");
        // Same title folder, a second file that renders to a different local path.
        let extra = "The.Matrix.(1999)/The.Matrix.(1999).Sample.mkv".to_string();
        let listings = vec![entry(&installed, 1, 1), entry(&extra, 1, 1)];
        let title_index = vec![index_row("The.Matrix")];
        let on_disk = vec![on_disk_file("The.Matrix")];
        let ranked = reclaim_preview(&listings, &kinds(), &title_index, &on_disk, &[]);
        // Both remotes render to the one movie path, which is installed and indexed.
        assert_eq!(ranked.len(), 2);
        assert!(
            ranked
                .iter()
                .any(|c| c.remote.rel_path().display().to_string() == installed)
        );
        // A remote for a title that is *not* on disk is never a candidate.
        let listings = vec![entry(&remote("Not.Here"), 1, 1)];
        let ranked = reclaim_preview(&listings, &kinds(), &title_index, &on_disk, &[]);
        assert!(ranked.is_empty());
    }

    #[test]
    fn rank_fields_come_from_one_covering_torrent() {
        let rel = remote("Shared");
        let listings = vec![entry(&rel, 1, 1)];
        let title_index = vec![index_row("Shared")];
        let on_disk = vec![on_disk_file("Shared")];
        let low_public = torrent(&rel, "pausedDL", 0.1, false);
        let high_private = GuardPreviewItem {
            hash: "priv".into(),
            state: "pausedDL".into(),
            ratio: 1.5,
            is_private: true,
            content_path: rel,
            save_path: String::new(),
            remote: None,
        };
        let ranked = reclaim_preview(
            &listings,
            &kinds(),
            &title_index,
            &on_disk,
            &[low_public, high_private],
        );
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].is_private, Some(true));
        assert_eq!(ranked[0].ratio, Some(1.5));
    }
}
