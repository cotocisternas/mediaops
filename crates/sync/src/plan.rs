//! Pure planner. No I/O.
//!
//! Identity is per **file**: a movie is one file, a show is one file per
//! episode, an album one per track (`FileKey`). "Already present" is checked
//! at that grain, so a second episode of a show the disk already has one
//! episode of still copies, and a half-copied album finishes.

use std::collections::HashSet;

use mediaops_core::{
    Action, DesiredState, FileKey, Grabber, HoldLiveItem, InstalledFile, Job, JobState,
    PathSchemaError, Placement, REVIEW_NEEDS_SPLIT, REVIEW_NEEDS_YEAR, REVIEW_UNPARSEABLE,
    RejectBin, RemoteEntry, RemoteRef, RootKinds, SKIP_DUPLICATE_TITLE, SKIP_LOCK, SKIP_MAX_COPY,
    SKIP_UPGRADE_NEVER, SKIP_WATERMARK, TitleId, TitleIndexEntry, TitleKind, WantState,
    classify_remote, normalize_placement, render_placement,
};

pub use mediaops_core::{AUDIO_EXTENSIONS, VIDEO_EXTENSIONS, is_media_file};

pub struct PlanRequest<'a> {
    pub listings: &'a [RemoteEntry],
    /// Kind each allowlisted root holds (`None` = infer from shape).
    pub root_kinds: &'a RootKinds,
    pub title_index: &'a [TitleIndexEntry],
    /// Schema files already on the library disk.
    pub on_disk: &'a [InstalledFile],
    pub open_wants: &'a [Job],
    pub desired: &'a DesiredState,
    pub free_bytes: u64,
    /// When doctor would freeze/drift, emit EdgeApply.
    pub edge_frozen: bool,
    /// Every live import-blocked item; their remotes are inbox, never Review.
    pub holds: &'a [HoldLiveItem],
    /// Approved holds with a live RemoteRef+placement become Copy.
    pub approved: &'a [HoldLiveItem],
    /// *arr wanted/missing TitleIds (key form). Unmonitor is
    /// `title_index` ∩ `on_disk` ∩ this set, movies and albums only.
    pub wanted_missing: &'a [TitleId],
}

pub struct Planned {
    pub actions: Vec<Action>,
    /// True when at least one copy-candidate existed and the first (music-first
    /// order) would by itself exceed `max_copy` or `min_free`.
    pub first_candidate_breaches: bool,
}

struct Candidate {
    title_id: TitleId,
    remote: RemoteRef,
    file_len: u64,
    placement: Placement,
    listing_index: usize,
    kind: TitleKind,
    wanted: bool,
}

fn review_reason(err: &PathSchemaError) -> &'static str {
    match err.reject_bin() {
        Some(RejectBin::NeedsYear) => REVIEW_NEEDS_YEAR,
        Some(RejectBin::NeedsSplit) => REVIEW_NEEDS_SPLIT,
        None => REVIEW_UNPARSEABLE,
    }
}

/// Build Copy/Skip/Review actions. Upgrade class is the constant **never**.
pub fn plan_actions(req: PlanRequest<'_>) -> Planned {
    let mut installed: HashSet<(TitleId, FileKey)> =
        req.on_disk.iter().map(InstalledFile::identity).collect();
    installed.extend(
        req.title_index
            .iter()
            .filter_map(TitleIndexEntry::installed_file)
            .map(|f| f.identity()),
    );
    let wants: HashSet<TitleId> = req
        .open_wants
        .iter()
        .filter(|j| matches!(j.state(), JobState::Want(WantState::Open)))
        .map(|j| j.title_id().clone())
        .collect();
    let held: HashSet<&RemoteRef> = req.holds.iter().filter_map(|h| h.remote.as_ref()).collect();

    let mut upgrade_never = Vec::new();
    let mut duplicates = Vec::new();
    let mut reviews = Vec::new();
    let mut candidates = Vec::new();
    let mut planned_files: HashSet<(TitleId, FileKey)> = HashSet::new();
    let mut skipped_files: HashSet<(TitleId, FileKey)> = HashSet::new();

    for (listing_index, entry) in req.listings.iter().enumerate() {
        if entry.len() == 0 || !is_media_file(entry.r#ref()) {
            continue;
        }
        if held.contains(entry.r#ref()) {
            continue;
        }
        let (title_id, placement) = match classify_remote(req.root_kinds, entry) {
            Ok(parsed) => parsed,
            Err(err) => {
                reviews.push(Action::Review {
                    remote: Some(entry.r#ref().clone()),
                    reason: review_reason(&err).to_string(),
                });
                continue;
            }
        };
        let identity = (title_id.clone(), placement.file_key());
        if installed.contains(&identity) {
            if skipped_files.insert(identity) {
                upgrade_never.push(Action::Skip {
                    title_id: Some(title_id),
                    reason: SKIP_UPGRADE_NEVER.to_string(),
                });
            } else {
                duplicates.push(Action::Skip {
                    title_id: Some(title_id),
                    reason: SKIP_DUPLICATE_TITLE.to_string(),
                });
            }
            continue;
        }
        if !planned_files.insert(identity) {
            duplicates.push(Action::Skip {
                title_id: Some(title_id),
                reason: SKIP_DUPLICATE_TITLE.to_string(),
            });
            continue;
        }
        let kind = title_id.kind();
        candidates.push(Candidate {
            wanted: wants.contains(&title_id),
            title_id,
            remote: entry.r#ref().clone(),
            file_len: entry.len(),
            placement,
            listing_index,
            kind,
        });
    }

    for (offset, item) in req.approved.iter().enumerate() {
        let (Some(remote), Some(placement)) = (&item.remote, &item.placement) else {
            continue;
        };
        let placement = normalize_placement(placement);
        if render_placement(&placement).is_err() {
            continue;
        }
        // The library identity is the key the placement names; the *arr id on
        // the hold stays the inbox key for reject/approve.
        let Ok(title_id) = placement.key_title_id() else {
            continue;
        };
        let listing_index = req.listings.len() + offset;
        let identity = (title_id.clone(), placement.file_key());
        if installed.contains(&identity) {
            if skipped_files.insert(identity) {
                upgrade_never.push(Action::Skip {
                    title_id: Some(title_id),
                    reason: SKIP_UPGRADE_NEVER.to_string(),
                });
            } else {
                duplicates.push(Action::Skip {
                    title_id: Some(title_id),
                    reason: SKIP_DUPLICATE_TITLE.to_string(),
                });
            }
            continue;
        }
        if !planned_files.insert(identity) {
            duplicates.push(Action::Skip {
                title_id: Some(title_id),
                reason: SKIP_DUPLICATE_TITLE.to_string(),
            });
            continue;
        }
        let file_len = req
            .listings
            .iter()
            .find(|entry| entry.r#ref() == remote)
            .map(RemoteEntry::len)
            .unwrap_or(item.size);
        if file_len == 0 {
            continue;
        }
        let kind = title_id.kind();
        candidates.push(Candidate {
            wanted: wants.contains(&title_id) || wants.contains(&item.key.title_id),
            title_id,
            remote: remote.clone(),
            file_len,
            placement,
            listing_index,
            kind,
        });
    }

    candidates.sort_by_key(|c| {
        let kind_rank = match c.kind {
            TitleKind::Album => 0u8,
            TitleKind::Movie => 1,
            TitleKind::Series => 2,
        };
        let want_rank = u8::from(!c.wanted);
        (kind_rank, want_rank, c.listing_index)
    });

    let first_candidate_breaches = match candidates.first() {
        Some(first) if !req.desired.lock() => {
            first.file_len > req.desired.max_copy().get()
                || first.file_len > req.free_bytes
                || req.free_bytes.saturating_sub(first.file_len) < req.desired.min_free().get()
        }
        _ => false,
    };

    let mut actions = Vec::new();
    if req.desired.lock() {
        for c in &candidates {
            actions.push(Action::Skip {
                title_id: Some(c.title_id.clone()),
                reason: SKIP_LOCK.to_string(),
            });
        }
        actions.extend(upgrade_never);
        actions.extend(duplicates);
        actions.extend(reviews);
        actions.extend(unmonitor_actions(
            req.desired.grabber(),
            req.title_index,
            req.on_disk,
            req.wanted_missing,
        ));
        if req.desired.grabber() == Grabber::Servarr {
            actions.insert(0, Action::GrabApply);
        }
        if req.edge_frozen {
            actions.insert(0, Action::EdgeApply);
        }
        return Planned {
            actions,
            first_candidate_breaches: false,
        };
    }

    let mut remaining_copy = req.desired.max_copy().get();
    let mut remaining_free = req.free_bytes;
    let min_free = req.desired.min_free().get();

    for kind in [TitleKind::Album, TitleKind::Movie, TitleKind::Series] {
        let mut blocked: Option<&'static str> = None;
        for c in candidates.iter().filter(|c| c.kind == kind) {
            if let Some(reason) = blocked {
                actions.push(Action::Skip {
                    title_id: Some(c.title_id.clone()),
                    reason: reason.to_string(),
                });
                continue;
            }
            let reason = if c.file_len > remaining_copy {
                Some(SKIP_MAX_COPY)
            } else if c.file_len > remaining_free
                || remaining_free.saturating_sub(c.file_len) < min_free
            {
                Some(SKIP_WATERMARK)
            } else {
                None
            };
            if let Some(reason) = reason {
                blocked = Some(reason);
                actions.push(Action::Skip {
                    title_id: Some(c.title_id.clone()),
                    reason: reason.to_string(),
                });
                continue;
            }
            remaining_copy = remaining_copy.saturating_sub(c.file_len);
            remaining_free = remaining_free.saturating_sub(c.file_len);
            actions.push(Action::Copy {
                title_id: c.title_id.clone(),
                remote: c.remote.clone(),
                file_len: c.file_len,
                placement: c.placement.clone(),
            });
        }
    }

    actions.extend(upgrade_never);
    actions.extend(duplicates);
    actions.extend(reviews);
    actions.extend(unmonitor_actions(
        req.desired.grabber(),
        req.title_index,
        req.on_disk,
        req.wanted_missing,
    ));
    if req.desired.grabber() == Grabber::Servarr {
        actions.insert(0, Action::GrabApply);
    }
    if req.edge_frozen {
        actions.insert(0, Action::EdgeApply);
    }
    Planned {
        actions,
        first_candidate_breaches,
    }
}

/// Unmonitor is `title_index` ∩ `on_disk` ∩ wanted/missing, movies and albums only.
///
/// The `title_index` row is the install proof: an on-disk file with no row never
/// unmonitors. The `on_disk` check is the *still there* proof -- nothing ever
/// deletes a `title_index` row, so an operator who reclaims space by hand would
/// otherwise have *arr told to stop re-acquiring what they just deleted.
///
/// Series are excluded. A series TitleId is the whole show, and Sonarr's
/// wanted/missing is one record per missing *episode* collapsed onto that parent
/// id, so a single installed episode would read as "complete" and unmonitor every
/// episode still outstanding. Per-episode completeness is not knowable from this
/// snapshot; until it is, series stay monitored.
///
/// This survives `lock = true` deliberately: the lock freezes copies -- disk and
/// bandwidth -- and unmonitoring only stops *arr chasing titles the library
/// already holds, which is the state a freeze should settle into.
fn unmonitor_actions(
    grabber: Grabber,
    title_index: &[TitleIndexEntry],
    on_disk: &[InstalledFile],
    wanted_missing: &[TitleId],
) -> Vec<Action> {
    if grabber != Grabber::Servarr {
        return Vec::new();
    }
    let missing: HashSet<&TitleId> = wanted_missing.iter().collect();
    let present: HashSet<&TitleId> = on_disk.iter().map(|f| &f.title_id).collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for entry in title_index {
        let title_id = entry.title_id();
        if title_id.kind() == TitleKind::Series {
            continue;
        }
        if missing.contains(title_id) && present.contains(title_id) && seen.insert(title_id) {
            out.push(Action::Unmonitor {
                title_id: title_id.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{
        Bytes, HoldKey, JobId, JobState, ReleaseId, RemoteEntry, RemoteRef, TitleIndexEntry,
        WantState,
    };
    use std::path::{Path, PathBuf};

    const DS: &str = r#"
schema_version = 1
max_copy_gib = 1
min_free_gib = 1
range_len_mib = 8
max_nvenc = 1
lock = false
"#;

    fn ds() -> &'static DesiredState {
        static PARSED: std::sync::LazyLock<DesiredState> =
            std::sync::LazyLock::new(|| DesiredState::from_toml(DS).expect("ds"));
        &PARSED
    }

    fn ds_servarr() -> &'static DesiredState {
        static PARSED: std::sync::LazyLock<DesiredState> = std::sync::LazyLock::new(|| {
            DesiredState::from_toml(&format!("{DS}\ngrabber = \"servarr\"\n")).expect("ds")
        });
        &PARSED
    }

    fn ds_locked() -> &'static DesiredState {
        static PARSED: std::sync::LazyLock<DesiredState> = std::sync::LazyLock::new(|| {
            DesiredState::from_toml(&DS.replace("lock = false", "lock = true")).expect("ds")
        });
        &PARSED
    }

    fn digest(fill: char) -> mediaops_core::Blake3Hex {
        mediaops_core::Blake3Hex::parse(&fill.to_string().repeat(64)).expect("d")
    }

    /// Roots as this operator's box has them: `movies`, `tv`, `music`.
    fn kinds() -> RootKinds {
        RootKinds::from([
            ("movies".to_string(), Some(TitleKind::Movie)),
            ("tv".to_string(), Some(TitleKind::Series)),
            ("music".to_string(), Some(TitleKind::Album)),
            ("usenet_movies".to_string(), Some(TitleKind::Movie)),
        ])
    }

    fn entry_in(root: &str, rel: &str, len: u64) -> RemoteEntry {
        RemoteEntry::from_wire_parts(
            RemoteRef::from_wire_parts(root.into(), PathBuf::from(rel)).expect("ref"),
            len,
            0,
            1,
        )
    }

    fn movie(name: &str, year: u16, len: u64) -> RemoteEntry {
        entry_in(
            "movies",
            &format!("{name}.({year})/{name}.({year}).mkv"),
            len,
        )
    }

    fn episode(show: &str, year: u16, s: u8, e: u16, len: u64) -> RemoteEntry {
        entry_in(
            "tv",
            &format!("{show}.({year})/Season.{s:02}/{show}.({year}).S{s:02}E{e:02}.mkv"),
            len,
        )
    }

    fn track(artist: &str, album: &str, year: u16, n: u8, title: &str, len: u64) -> RemoteEntry {
        entry_in(
            "music",
            &format!("{artist}/{album}.({year})/{album}.({year}).{n:02}.{title}.flac"),
            len,
        )
    }

    fn on_disk(rel: &str) -> InstalledFile {
        InstalledFile::from_rel_path(Path::new(rel)).expect("on disk")
    }

    fn want(title: TitleId) -> Job {
        Job::new(
            JobId::new(1).expect("id"),
            title,
            JobState::Want(WantState::Open),
            None,
        )
        .expect("want")
    }

    fn copies(actions: &[Action]) -> Vec<String> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Copy {
                    title_id,
                    placement,
                    ..
                } => Some(format!("{} {}", title_id.render(), placement.label())),
                _ => None,
            })
            .collect()
    }

    fn skips(actions: &[Action], reason: &str) -> usize {
        actions
            .iter()
            .filter(|a| matches!(a, Action::Skip { reason: r, .. } if r == reason))
            .count()
    }

    fn request<'a>(
        listings: &'a [RemoteEntry],
        kinds: &'a RootKinds,
        title_index: &'a [TitleIndexEntry],
        on_disk: &'a [InstalledFile],
        desired: &'a DesiredState,
    ) -> PlanRequest<'a> {
        PlanRequest {
            listings,
            root_kinds: kinds,
            title_index,
            on_disk,
            open_wants: &[],
            desired,
            free_bytes: 2 * Bytes::GIB,
            edge_frozen: false,
            holds: &[],
            approved: &[],
            wanted_missing: &[],
        }
    }

    #[test]
    fn planner_music_first_then_movie_then_series() {
        let listings = [
            movie("Coco", 2017, 10),
            episode("Silo", 2023, 1, 1, 10),
            track("Tool", "Lateralus", 2001, 1, "The.Grudge", 10),
        ];
        let kinds = kinds();
        let planned = plan_actions(request(&listings, &kinds, &[], &[], ds()));
        assert_eq!(
            copies(&planned.actions),
            vec![
                "album:key:tool.lateralus Tool/Lateralus.(2001) 01",
                "movie:key:coco.2017 Coco.(2017)",
                "series:key:silo.2023 Silo.(2023) S01E01",
            ]
        );
        assert!(!planned.first_candidate_breaches);
    }

    #[test]
    fn every_episode_and_track_is_its_own_copy() {
        let listings = [
            episode("Silo", 2023, 1, 1, 10),
            episode("Silo", 2023, 1, 2, 10),
            episode("Silo", 2023, 2, 1, 10),
            track("Tool", "Lateralus", 2001, 1, "The.Grudge", 10),
            track("Tool", "Lateralus", 2001, 2, "Eon.Blue.Apocalypse", 10),
        ];
        let kinds = kinds();
        let planned = plan_actions(request(&listings, &kinds, &[], &[], ds()));
        assert_eq!(copies(&planned.actions).len(), 5, "{:?}", planned.actions);
        assert_eq!(skips(&planned.actions, SKIP_DUPLICATE_TITLE), 0);
    }

    #[test]
    fn present_episode_is_skipped_but_missing_sibling_still_copies() {
        let listings = [
            episode("Silo", 2023, 1, 1, 10),
            episode("Silo", 2023, 1, 2, 10),
        ];
        let disk = [on_disk(
            "series/Silo.(2023)/Season.01/Silo.(2023).S01E01.mkv",
        )];
        let kinds = kinds();
        let planned = plan_actions(request(&listings, &kinds, &[], &disk, ds()));
        assert_eq!(
            copies(&planned.actions),
            vec!["series:key:silo.2023 Silo.(2023) S01E02"]
        );
        assert_eq!(skips(&planned.actions, SKIP_UPGRADE_NEVER), 1);
    }

    #[test]
    fn half_copied_album_finishes_and_remaster_counts_as_present() {
        let listings = [
            track("Yes", "Relayer", 1974, 1, "The.Gates.Of.Delirium", 10),
            track("Yes", "Relayer", 1974, 2, "Sound.Chaser", 10),
        ];
        // Local has the 2013 remaster's track 1 only.
        let disk = [on_disk(
            "music/Yes/Relayer.(2013)/Relayer.(2013).01.The.Gates.Of.Delirium.flac",
        )];
        let kinds = kinds();
        let planned = plan_actions(request(&listings, &kinds, &[], &disk, ds()));
        assert_eq!(
            copies(&planned.actions),
            vec!["album:key:yes.relayer Yes/Relayer.(1974) 02"]
        );
    }

    #[test]
    fn movie_present_on_disk_or_in_index_is_upgrade_never() {
        let listings = [movie("Coco", 2017, 10), movie("Up", 2009, 10)];
        let index = [TitleIndexEntry::new(
            TitleId::movie_key("Coco", 2017).expect("id"),
            "movies/Coco.(2017)/Coco.(2017).mkv",
            digest('a'),
            digest('a'),
        )];
        let disk = [on_disk("movies/Up.(2009)/Up.(2009).mkv")];
        let kinds = kinds();
        let planned = plan_actions(request(&listings, &kinds, &index, &disk, ds()));
        assert!(copies(&planned.actions).is_empty());
        assert_eq!(skips(&planned.actions, SKIP_UPGRADE_NEVER), 2);
    }

    #[test]
    fn spaced_arr_folders_still_match_the_dotted_local_library() {
        // Radarr made the folder before dotted naming; the file is dotted.
        let listings = [entry_in(
            "movies",
            "Spider-Man - Brand New Day (2026)/Spider-Man.Brand.New.Day.(2026).mkv",
            10,
        )];
        let disk = [on_disk(
            "movies/Spider-Man.Brand.New.Day.(2026)/Spider-Man.Brand.New.Day.(2026).mkv",
        )];
        let kinds = kinds();
        let planned = plan_actions(request(&listings, &kinds, &[], &disk, ds()));
        assert!(copies(&planned.actions).is_empty(), "{:?}", planned.actions);
        assert_eq!(skips(&planned.actions, SKIP_UPGRADE_NEVER), 1);
    }

    #[test]
    fn second_remote_for_the_same_file_is_duplicate_title() {
        let listings = [
            movie("Coco", 2017, 10),
            entry_in("usenet_movies", "Coco.(2017)/Coco.(2017).mkv", 12),
        ];
        let kinds = kinds();
        let planned = plan_actions(request(&listings, &kinds, &[], &[], ds()));
        assert_eq!(copies(&planned.actions).len(), 1);
        assert_eq!(skips(&planned.actions, SKIP_DUPLICATE_TITLE), 1);
        // The media root wins because listings are sorted by root id.
        assert!(matches!(
            &planned.actions[0],
            Action::Copy { remote, .. } if remote.root_id() == "movies"
        ));
    }

    #[test]
    fn unplaceable_media_is_review_and_furniture_is_silent() {
        let listings = [
            entry_in(
                "usenet_movies",
                "Some.Movie.2024.1080p.WEB-DL-GROUP/Some.Movie.2024.1080p.WEB-DL-GROUP.mkv",
                10,
            ),
            entry_in("movies", "No.Year/No.Year.mkv", 10),
            entry_in("movies", "Coco.(2017)/Coco.(2017).nfo", 10),
            entry_in("movies", "Coco.(2017)/Sample/Coco.(2017).sample.mkv", 10),
            entry_in("usenet_movies", "Job/file.par2", 10),
        ];
        let kinds = kinds();
        let planned = plan_actions(request(&listings, &kinds, &[], &[], ds()));
        let reviews: Vec<(String, String)> = planned
            .actions
            .iter()
            .filter_map(|a| match a {
                Action::Review {
                    remote: Some(r),
                    reason,
                } => Some((r.rel_path().display().to_string(), reason.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            reviews,
            vec![
                (
                    "Some.Movie.2024.1080p.WEB-DL-GROUP/Some.Movie.2024.1080p.WEB-DL-GROUP.mkv"
                        .to_string(),
                    REVIEW_NEEDS_YEAR.to_string()
                ),
                (
                    "No.Year/No.Year.mkv".to_string(),
                    REVIEW_NEEDS_YEAR.to_string()
                ),
            ],
            "{:?}",
            planned.actions
        );
        assert!(copies(&planned.actions).is_empty());
        assert_eq!(planned.actions.len(), 2, "nfo/sample/par2 are silent");
    }

    #[test]
    fn a_live_hold_remote_is_inbox_not_review() {
        let rel = "Some.Movie.2024.1080p-GROUP/Some.Movie.2024.1080p-GROUP.mkv";
        let listings = [entry_in("usenet_movies", rel, 10)];
        let mut hold = HoldLiveItem::new(
            HoldKey::new(
                TitleId::movie("603").expect("tmdb"),
                ReleaseId::parse("deadbeef").expect("release"),
            ),
            0,
            10,
            "importBlocked",
        );
        hold.remote = Some(listings[0].r#ref().clone());
        let kinds = kinds();
        let mut req = request(&listings, &kinds, &[], &[], ds());
        let holds = [hold];
        req.holds = &holds;
        let planned = plan_actions(req);
        assert!(planned.actions.is_empty(), "{:?}", planned.actions);
    }

    #[test]
    fn approved_hold_copies_under_its_arr_title_as_a_key_identity() {
        let rel = "Some.Movie.2024.1080p-GROUP/Some.Movie.2024.1080p-GROUP.mkv";
        let listings = [entry_in("usenet_movies", rel, 10)];
        let mut hold = HoldLiveItem::new(
            HoldKey::new(
                TitleId::movie("603").expect("tmdb"),
                ReleaseId::parse("deadbeef").expect("release"),
            ),
            0,
            10,
            "importBlocked",
        );
        hold.remote = Some(listings[0].r#ref().clone());
        hold.placement = Some(Placement::movie("Some Movie: Part II", 2024, "mkv"));
        let kinds = kinds();
        let holds = [hold.clone()];
        let approved = [hold];
        let mut req = request(&listings, &kinds, &[], &[], ds());
        req.holds = &holds;
        req.approved = &approved;
        let planned = plan_actions(req);
        assert_eq!(
            copies(&planned.actions),
            vec!["movie:key:somemoviepartii.2024 Some.Movie.Part.II.(2024)"]
        );
        match &planned.actions[0] {
            Action::Copy { placement, .. } => {
                assert_eq!(
                    render_placement(placement)
                        .expect("render")
                        .to_str()
                        .expect("utf8"),
                    "movies/Some.Movie.Part.II.(2024)/Some.Movie.Part.II.(2024).mkv"
                );
            }
            other => panic!("expected Copy, got {other:?}"),
        }
    }

    #[test]
    fn approved_hold_already_on_disk_is_upgrade_never() {
        let rel = "Coco.2017.1080p-GROUP/Coco.2017.1080p-GROUP.mkv";
        let listings = [entry_in("usenet_movies", rel, 10)];
        let mut hold = HoldLiveItem::new(
            HoldKey::new(
                TitleId::movie("354912").expect("tmdb"),
                ReleaseId::parse("cafe").expect("release"),
            ),
            0,
            10,
            "importBlocked",
        );
        hold.remote = Some(listings[0].r#ref().clone());
        hold.placement = Some(Placement::movie("Coco", 2017, "mkv"));
        let disk = [on_disk("movies/Coco.(2017)/Coco.(2017).mkv")];
        let kinds = kinds();
        let holds = [hold.clone()];
        let approved = [hold];
        let mut req = request(&listings, &kinds, &[], &disk, ds());
        req.holds = &holds;
        req.approved = &approved;
        let planned = plan_actions(req);
        assert!(copies(&planned.actions).is_empty());
        assert_eq!(skips(&planned.actions, SKIP_UPGRADE_NEVER), 1);
    }

    #[test]
    fn want_prioritizes_matching_title_within_kind() {
        let listings = [movie("Alpha", 2001, 10), movie("Beta", 2002, 10)];
        let wants = [want(TitleId::movie_key("Beta", 2002).expect("id"))];
        let kinds = kinds();
        let mut req = request(&listings, &kinds, &[], &[], ds());
        req.open_wants = &wants;
        let planned = plan_actions(req);
        assert_eq!(
            copies(&planned.actions),
            vec![
                "movie:key:beta.2002 Beta.(2002)",
                "movie:key:alpha.2001 Alpha.(2001)"
            ]
        );
    }

    #[test]
    fn max_copy_blocks_the_rest_of_the_class_and_watermark_the_rest() {
        // max_copy 1 GiB: first movie fits, second does not, third is skipped too.
        let listings = [
            movie("A", 2001, 600 * Bytes::MIB),
            movie("B", 2002, 600 * Bytes::MIB),
            movie("C", 2003, 10),
            episode("S", 2020, 1, 1, 10),
        ];
        let kinds = kinds();
        let planned = plan_actions(request(&listings, &kinds, &[], &[], ds()));
        assert_eq!(
            copies(&planned.actions),
            vec![
                "movie:key:a.2001 A.(2001)",
                "series:key:s.2020 S.(2020) S01E01"
            ]
        );
        assert_eq!(skips(&planned.actions, SKIP_MAX_COPY), 2);

        // Watermark: free 1.5 GiB, min_free 1 GiB → only 0.5 GiB of copies fit.
        let listings = [
            movie("A", 2001, 400 * Bytes::MIB),
            movie("B", 2002, 400 * Bytes::MIB),
        ];
        let mut req = request(&listings, &kinds, &[], &[], ds());
        req.free_bytes = Bytes::GIB + 512 * Bytes::MIB;
        let planned = plan_actions(req);
        assert_eq!(copies(&planned.actions).len(), 1);
        assert_eq!(skips(&planned.actions, SKIP_WATERMARK), 1);
        assert!(!planned.first_candidate_breaches);

        let listings = [movie("Huge", 2001, 3 * Bytes::GIB)];
        let planned = plan_actions(request(&listings, &kinds, &[], &[], ds()));
        assert!(planned.first_candidate_breaches);
        assert!(copies(&planned.actions).is_empty());
    }

    #[test]
    fn lock_skips_every_candidate_with_lock_reason() {
        let listings = [movie("A", 2001, 10), episode("S", 2020, 1, 1, 10)];
        let kinds = kinds();
        let planned = plan_actions(request(&listings, &kinds, &[], &[], ds_locked()));
        assert!(copies(&planned.actions).is_empty());
        assert_eq!(skips(&planned.actions, SKIP_LOCK), 2);
        assert!(!planned.first_candidate_breaches);
    }

    #[test]
    fn servarr_inserts_grab_apply_first_and_frozen_edge_before_it() {
        let listings = [movie("A", 2001, 10)];
        let kinds = kinds();
        let mut req = request(&listings, &kinds, &[], &[], ds_servarr());
        req.edge_frozen = true;
        let planned = plan_actions(req);
        assert!(matches!(planned.actions[0], Action::EdgeApply));
        assert!(matches!(planned.actions[1], Action::GrabApply));
        let planned = plan_actions(request(&listings, &kinds, &[], &[], ds()));
        assert!(
            !planned
                .actions
                .iter()
                .any(|a| matches!(a, Action::GrabApply | Action::EdgeApply))
        );
    }

    #[test]
    fn unmonitor_needs_index_and_disk_and_wanted_missing_and_never_series() {
        let coco = TitleId::movie_key("Coco", 2017).expect("id");
        let silo = TitleId::series_key("Silo", 2023).expect("id");
        let index = [
            TitleIndexEntry::new(
                coco.clone(),
                "movies/Coco.(2017)/Coco.(2017).mkv",
                digest('a'),
                digest('a'),
            ),
            TitleIndexEntry::new(
                silo.clone(),
                "series/Silo.(2023)/Season.01/Silo.(2023).S01E01.mkv",
                digest('b'),
                digest('b'),
            ),
        ];
        let disk = [
            on_disk("movies/Coco.(2017)/Coco.(2017).mkv"),
            on_disk("series/Silo.(2023)/Season.01/Silo.(2023).S01E01.mkv"),
        ];
        let missing = [coco.clone(), silo.clone()];
        let kinds = kinds();
        let mut req = request(&[], &kinds, &index, &disk, ds_servarr());
        req.wanted_missing = &missing;
        let planned = plan_actions(req);
        let unmonitors: Vec<&TitleId> = planned
            .actions
            .iter()
            .filter_map(|a| match a {
                Action::Unmonitor { title_id } => Some(title_id),
                _ => None,
            })
            .collect();
        assert_eq!(unmonitors, vec![&coco], "{:?}", planned.actions);

        // Index without disk: nothing; disk without index: nothing.
        let mut req = request(&[], &kinds, &index, &[], ds_servarr());
        req.wanted_missing = &missing;
        assert!(
            !plan_actions(req)
                .actions
                .iter()
                .any(|a| matches!(a, Action::Unmonitor { .. }))
        );
        let mut req = request(&[], &kinds, &[], &disk, ds_servarr());
        req.wanted_missing = &missing;
        assert!(
            !plan_actions(req)
                .actions
                .iter()
                .any(|a| matches!(a, Action::Unmonitor { .. }))
        );
        // grabber=None never unmonitors.
        let mut req = request(&[], &kinds, &index, &disk, ds());
        req.wanted_missing = &missing;
        assert!(plan_actions(req).actions.is_empty());
        // Lock still unmonitors.
        let locked = DesiredState::from_toml(&format!(
            "{}\ngrabber = \"servarr\"\n",
            DS.replace("lock = false", "lock = true")
        ))
        .expect("ds");
        let mut req = request(&[], &kinds, &index, &disk, &locked);
        req.wanted_missing = &missing;
        assert!(
            plan_actions(req)
                .actions
                .iter()
                .any(|a| matches!(a, Action::Unmonitor { .. }))
        );
    }

    #[test]
    fn kind_is_inferred_when_a_root_declares_none() {
        let kinds = RootKinds::from([("box".to_string(), None)]);
        let listings = [
            entry_in("box", "Coco.(2017)/Coco.(2017).mkv", 10),
            entry_in("box", "Silo.(2023)/Season.01/Silo.(2023).S01E01.mkv", 10),
            entry_in(
                "box",
                "Tool/Lateralus.(2001)/Lateralus.(2001).01.The.Grudge.flac",
                10,
            ),
        ];
        let planned = plan_actions(request(&listings, &kinds, &[], &[], ds()));
        assert_eq!(copies(&planned.actions).len(), 3, "{:?}", planned.actions);
    }

    #[test]
    fn media_file_filter() {
        let r = |p: &str| RemoteRef::from_wire_parts("x".into(), PathBuf::from(p)).expect("ref");
        assert!(is_media_file(&r("a/b.mkv")));
        assert!(is_media_file(&r("a/b.FLAC")));
        assert!(!is_media_file(&r("a/b.nfo")));
        assert!(!is_media_file(&r("a/b.sample.mkv")));
        assert!(!is_media_file(&r("a/Sample.mkv")));
        assert!(!is_media_file(&r("a/noext")));
    }
}
