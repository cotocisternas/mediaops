//! Reclaim policy: surplus ranking and qBit path matching.
//!
//! Constraints (private-under-goal, seeding, nlink, install proof) times a
//! ranked dry-run. Goal ratio is a constant, not DesiredState. Size/mtime is
//! never proof.

use std::collections::HashSet;
use std::path::{Component, Path};

use serde::Serialize;

use crate::pathschema::parse_placement;
use crate::plan::Action;
use crate::title_id::TitleId;
use crate::title_index::TitleIndexEntry;
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
        if self.remote.as_ref() == Some(remote) {
            return true;
        }
        torrent_covers_file(&self.content_path, &self.save_path, remote.rel_path())
    }

    pub fn covers_path(&self, absolute: &Path) -> bool {
        torrent_covers_file(&self.content_path, &self.save_path, absolute)
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

/// Proved surplus (install_b3 ∩ on-disk, nlink==1). No qBit filter.
pub fn reclaim_proved(
    listings: &[RemoteEntry],
    title_index: &[TitleIndexEntry],
    on_disk: &[TitleId],
) -> Vec<ReclaimCandidate> {
    let proved: HashSet<&TitleId> = title_index
        .iter()
        .map(TitleIndexEntry::title_id)
        .filter(|id| on_disk.iter().any(|d| d == *id))
        .collect();
    let mut out = Vec::new();
    for entry in listings {
        if entry.nlink() > 1 {
            continue;
        }
        let Ok((title_id, _)) = parse_placement(entry.r#ref().rel_path()) else {
            continue;
        };
        if !proved.contains(&title_id) {
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
    title_index: &[TitleIndexEntry],
    on_disk: &[TitleId],
    torrents: &[GuardPreviewItem],
) -> Vec<ReclaimCandidate> {
    let mut out = Vec::new();
    for mut candidate in reclaim_proved(listings, title_index, on_disk) {
        let covering: Vec<&GuardPreviewItem> = torrents
            .iter()
            .filter(|t| t.matches_remote(&candidate.remote))
            .collect();
        if covering.iter().any(|t| t.blocks_delete()) {
            continue;
        }
        if let Some(best) = covering.iter().min_by(|a, b| a.ratio.total_cmp(&b.ratio)) {
            candidate.ratio = Some(best.ratio);
            candidate.is_private = Some(covering.iter().any(|t| t.is_private));
        }
        out.push(candidate);
    }
    out.sort_by_key(rank_key);
    out
}

/// `DeleteRemote { remote }` for each ranked candidate. Not used by `run`.
pub fn reclaim_actions(
    listings: &[RemoteEntry],
    title_index: &[TitleIndexEntry],
    on_disk: &[TitleId],
    torrents: &[GuardPreviewItem],
) -> Vec<Action> {
    reclaim_preview(listings, title_index, on_disk, torrents)
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

    fn movie(id: &str) -> TitleId {
        TitleId::movie(id).expect("title")
    }

    fn index_row(id: &str, rel: &str) -> TitleIndexEntry {
        TitleIndexEntry::new(movie(id), rel, digest(), digest())
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

    fn schema(id: &str, name: &str) -> String {
        format!("movies/{name}.(1999).{{tmdb-{id}}}/{name}.(1999).mkv")
    }

    #[test]
    fn ranks_public_low_ratio_older_first_and_usenet_by_age() {
        let pub_old = schema("1", "Public.Old");
        let pub_new = schema("2", "Public.New");
        let high = schema("3", "High.Ratio");
        let priv_over = schema("4", "Private.Over");
        let priv_under = schema("5", "Private.Under");
        let usenet_old = schema("6", "Usenet.Old");
        let usenet_new = schema("7", "Usenet.New");
        let hard = schema("8", "Hardlink");
        let no_digest = schema("9", "NoDigest");
        let seeding = schema("10", "Seeding");

        let listings = vec![
            entry(&pub_new, 200, 1),
            entry(&high, 50, 1),
            entry(&priv_over, 10, 1),
            entry(&priv_under, 1, 1),
            entry(&usenet_new, 300, 1),
            entry(&usenet_old, 20, 1),
            entry(&hard, 5, 2),
            entry(&no_digest, 5, 1),
            entry(&seeding, 5, 1),
            entry(&pub_old, 100, 1),
        ];
        let title_index: Vec<_> = ["1", "2", "3", "4", "5", "6", "7", "8", "10"]
            .into_iter()
            .map(|id| {
                let rel = listings
                    .iter()
                    .find_map(|e| {
                        parse_placement(e.r#ref().rel_path())
                            .ok()
                            .filter(|(t, _)| t == &movie(id))
                            .map(|_| e.r#ref().rel_path().display().to_string())
                    })
                    .unwrap_or_else(|| schema(id, id));
                index_row(id, &rel)
            })
            .collect();
        let on_disk: Vec<_> = title_index
            .iter()
            .map(|e| e.title_id().clone())
            .chain(std::iter::once(movie("9")))
            .collect();
        let torrents = vec![
            torrent(&pub_old, "pausedDL", 0.1, false),
            torrent(&pub_new, "pausedDL", 0.1, false),
            torrent(&high, "pausedDL", 2.0, false),
            torrent(&priv_over, "pausedDL", 1.5, true),
            torrent(&priv_under, "pausedDL", 0.2, true),
            torrent(&seeding, "uploading", 2.0, false),
        ];

        let ranked = reclaim_preview(&listings, &title_index, &on_disk, &torrents);
        let ids: Vec<String> = ranked.iter().map(|c| c.title_id.render()).collect();
        // public/low-ratio/older first; usenet is age-only (ratio treated as 0).
        assert_eq!(
            ids,
            vec![
                "movie:tmdb:6",
                "movie:tmdb:7",
                "movie:tmdb:1",
                "movie:tmdb:2",
                "movie:tmdb:3",
                "movie:tmdb:4",
            ],
            "{ids:?}"
        );
        assert!(
            !ids.iter().any(|id| id.ends_with(":5")
                || id.ends_with(":8")
                || id.ends_with(":9")
                || id.ends_with(":10")),
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
        let rel = schema("99", "Only.Mtime");
        let listings = vec![entry(&rel, 1, 1)];
        let on_disk = vec![movie("99")];
        let ranked = reclaim_preview(&listings, &[], &on_disk, &[]);
        assert!(ranked.is_empty(), "no title_index row means no delete");
    }

    #[test]
    fn install_b3_without_on_disk_is_not_proof() {
        let rel = schema("99", "Index.Only");
        let listings = vec![entry(&rel, 1, 1)];
        let title_index = vec![index_row("99", &rel)];
        let ranked = reclaim_preview(&listings, &title_index, &[], &[]);
        assert!(ranked.is_empty());
    }

    #[test]
    fn actions_are_delete_remote_payloads_in_rank_order() {
        let rel = schema("1", "Usenet");
        let listings = vec![entry(&rel, 1, 1)];
        let title_index = vec![index_row("1", &rel)];
        let on_disk = vec![movie("1")];
        let actions = reclaim_actions(&listings, &title_index, &on_disk, &[]);
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
        let file = Path::new("movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv");
        assert!(torrent_covers_file(
            "/data/media/movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv",
            "/data/torrents",
            file
        ));
        assert!(torrent_covers_file(
            "/data/media/movies/The.Matrix.(1999).{tmdb-603}",
            "",
            Path::new("/data/media/movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv")
        ));
        assert!(!torrent_covers_file(
            "/data/torrents/other",
            "/data/torrents",
            Path::new("/data/media/movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv")
        ));
        let library =
            Path::new("/data/media/movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv");
        assert!(
            !torrent_covers_file("", "/", library),
            "/ save_path must not cover library files"
        );
        assert!(!torrent_covers_file("/", "", library));
    }

    #[test]
    fn any_covering_seeding_or_private_under_goal_omits_the_file() {
        let rel = schema("1", "Shared");
        let listings = vec![entry(&rel, 1, 1)];
        let title_index = vec![index_row("1", &rel)];
        let on_disk = vec![movie("1")];
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
        let ranked = reclaim_preview(&listings, &title_index, &on_disk, &[paused, private_under]);
        assert!(ranked.is_empty(), "private-under-goal cover must omit");
    }
}
