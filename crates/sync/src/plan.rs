//! Pure grabber=None planner. No I/O.

use std::collections::HashSet;

use mediaops_core::{
    Action, DesiredState, Grabber, Job, JobState, RemoteEntry, SKIP_DUPLICATE_TITLE, SKIP_LOCK,
    SKIP_MAX_COPY, SKIP_UPGRADE_NEVER, SKIP_WATERMARK, TitleId, TitleIndexEntry, TitleKind,
    WantState, parse_placement,
};

pub struct PlanRequest<'a> {
    pub listings: &'a [RemoteEntry],
    pub title_index: &'a [TitleIndexEntry],
    /// Schema-valid files already on the library disk (TitleId only). Treated
    /// as installed for `upgrade_never` even when sqlite has no row.
    pub on_disk: &'a [TitleId],
    pub open_wants: &'a [Job],
    pub desired: &'a DesiredState,
    pub free_bytes: u64,
    /// When doctor would freeze/drift, emit EdgeApply.
    pub edge_frozen: bool,
}

pub struct Planned {
    pub actions: Vec<Action>,
    /// True when at least one copy-candidate existed and the first (music-first
    /// order) would by itself exceed `max_copy` or `min_free`.
    pub first_candidate_breaches: bool,
}

struct Candidate {
    title_id: TitleId,
    remote: mediaops_core::RemoteRef,
    file_len: u64,
    placement: mediaops_core::Placement,
    listing_index: usize,
    kind: TitleKind,
    wanted: bool,
}

/// Build Copy/Skip actions. Upgrade class is the constant **never**.
pub fn plan_actions(req: PlanRequest<'_>) -> Planned {
    let mut installed: HashSet<TitleId> = req
        .title_index
        .iter()
        .map(|e| e.title_id().clone())
        .collect();
    installed.extend(req.on_disk.iter().cloned());
    let wants: HashSet<TitleId> = req
        .open_wants
        .iter()
        .filter(|j| matches!(j.state(), JobState::Want(WantState::Open)))
        .map(|j| j.title_id().clone())
        .collect();

    let mut upgrade_never = Vec::new();
    let mut duplicates = Vec::new();
    let mut candidates = Vec::new();
    let mut planned_titles = HashSet::new();

    for (listing_index, entry) in req.listings.iter().enumerate() {
        if entry.len() == 0 {
            continue;
        }
        let Ok((title_id, placement)) = parse_placement(entry.r#ref().rel_path()) else {
            continue;
        };
        if installed.contains(&title_id) {
            if planned_titles.insert(title_id.clone()) {
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
        if !planned_titles.insert(title_id.clone()) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{
        Bytes, JobId, JobState, RemoteEntry, RemoteRef, TitleIndexEntry, WantState,
    };
    use std::path::PathBuf;

    const DS: &str = r#"
schema_version = 1
max_copy_gib = 1
min_free_gib = 1
range_len_mib = 8
max_nvenc = 1
lock = false
"#;

    fn ds() -> DesiredState {
        DesiredState::from_toml(DS).expect("ds")
    }

    fn digest(fill: char) -> mediaops_core::Blake3Hex {
        mediaops_core::Blake3Hex::parse(&fill.to_string().repeat(64)).expect("d")
    }

    fn entry(rel: &str, len: u64) -> RemoteEntry {
        RemoteEntry::from_wire_parts(
            RemoteRef::from_wire_parts("seed".into(), PathBuf::from(rel)).expect("ref"),
            len,
            0,
            1,
        )
    }

    fn movie_rel(id: &str, year: u16) -> String {
        format!("movies/Title.({year}).{{tmdb-{id}}}/Title.({year}).mkv")
    }

    fn album_rel() -> &'static str {
        "music/Relayer.(2013).{mbid-0f82b02e-c6cd-4242-b195-93d4bf3e0d63}/01.The.Gates.Of.Delirium.(2013).flac"
    }

    fn series_rel() -> &'static str {
        "series/The.Wire.(2002).{tvdb-79126}/The.Wire.(2002).S01E01.mkv"
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

    fn copies(actions: &[Action]) -> Vec<&TitleId> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Copy { title_id, .. } => Some(title_id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn planner_music_first_then_movie_then_series() {
        let listings = [
            entry(&movie_rel("603", 1999), 10),
            entry(series_rel(), 10),
            entry(album_rel(), 10),
        ];
        let planned = plan_actions(PlanRequest {
            listings: &listings,
            title_index: &[],
            on_disk: &[],
            open_wants: &[],
            desired: &ds(),
            free_bytes: 2 * Bytes::GIB,
            edge_frozen: false,
        });
        let ids: Vec<String> = copies(&planned.actions)
            .into_iter()
            .map(|t| t.render())
            .collect();
        assert_eq!(
            ids,
            vec![
                "album:mbid:0f82b02e-c6cd-4242-b195-93d4bf3e0d63".to_string(),
                "movie:tmdb:603".to_string(),
                "series:tvdb:79126".to_string(),
            ]
        );
        assert!(!planned.first_candidate_breaches);
    }

    #[test]
    fn installed_is_skip_upgrade_never_not_auto_upgrade_1080p_to_4k_remux_because_disk_is_bored() {
        let title = TitleId::movie("603").expect("id");
        let listings = [
            entry(&movie_rel("603", 1999), 10),
            entry(&movie_rel("604", 2000), 10),
        ];
        let index = [TitleIndexEntry::new(
            title.clone(),
            movie_rel("603", 1999),
            digest('a'),
            digest('a'),
        )];
        let planned = plan_actions(PlanRequest {
            listings: &listings,
            title_index: &index,
            on_disk: &[],
            open_wants: &[],
            desired: &ds(),
            free_bytes: 2 * Bytes::GIB,
            edge_frozen: false,
        });
        assert!(
            planned.actions.iter().any(|a| matches!(
                a,
                Action::Skip {
                    title_id: Some(id),
                    reason
                } if *id == title && reason == SKIP_UPGRADE_NEVER
            )),
            "auto-upgrade 1080p → 4k remux because disk is bored must never happen: {:?}",
            planned.actions
        );
        let copied = copies(&planned.actions);
        assert_eq!(copied, vec![&TitleId::movie("604").expect("604")]);
    }

    #[test]
    fn watermark_skip_rest_of_class() {
        let listings = [
            entry(&movie_rel("603", 1999), 200),
            entry(&movie_rel("604", 2000), 10),
        ];
        let planned = plan_actions(PlanRequest {
            listings: &listings,
            title_index: &[],
            on_disk: &[],
            open_wants: &[],
            desired: &ds(),
            free_bytes: Bytes::GIB + 100,
            edge_frozen: false,
        });
        assert!(
            planned.actions.iter().all(|a| matches!(
                a,
                Action::Skip {
                    reason,
                    ..
                } if reason == SKIP_WATERMARK
            )),
            "{:?}",
            planned.actions
        );
        assert!(planned.first_candidate_breaches);
        assert!(copies(&planned.actions).is_empty());
    }

    #[test]
    fn max_copy_skip() {
        let listings = [entry(&movie_rel("603", 1999), Bytes::GIB + 1)];
        let planned = plan_actions(PlanRequest {
            listings: &listings,
            title_index: &[],
            on_disk: &[],
            open_wants: &[],
            desired: &ds(),
            free_bytes: 4 * Bytes::GIB,
            edge_frozen: false,
        });
        assert!(
            planned.actions.iter().any(|a| matches!(
                a,
                Action::Skip {
                    reason,
                    ..
                } if reason == SKIP_MAX_COPY
            )),
            "{:?}",
            planned.actions
        );
        assert!(planned.first_candidate_breaches);
    }

    #[test]
    fn unparseable_remotes_are_omitted() {
        let listings = [
            entry("movies/Not.A.Schema.mkv", 10),
            entry("needs-year/x.mkv", 10),
            entry(&movie_rel("603", 1999), 10),
        ];
        let planned = plan_actions(PlanRequest {
            listings: &listings,
            title_index: &[],
            on_disk: &[],
            open_wants: &[],
            desired: &ds(),
            free_bytes: 2 * Bytes::GIB,
            edge_frozen: false,
        });
        assert_eq!(planned.actions.len(), 1);
        assert!(matches!(planned.actions[0], Action::Copy { .. }));
    }

    #[test]
    fn want_prioritizes_matching_title_within_kind() {
        let listings = [
            entry(&movie_rel("603", 1999), 10),
            entry(&movie_rel("604", 2000), 10),
        ];
        let wanted = TitleId::movie("604").expect("604");
        let wants = [want(wanted.clone())];
        let planned = plan_actions(PlanRequest {
            listings: &listings,
            title_index: &[],
            on_disk: &[],
            open_wants: &wants,
            desired: &ds(),
            free_bytes: 2 * Bytes::GIB,
            edge_frozen: false,
        });
        let ids: Vec<String> = copies(&planned.actions)
            .into_iter()
            .map(|t| t.render())
            .collect();
        assert_eq!(
            ids,
            vec!["movie:tmdb:604".to_string(), "movie:tmdb:603".to_string()]
        );
    }

    fn extra_album_rel() -> &'static str {
        "music/Relayer.(2013).{mbid-0f82b02e-c6cd-4242-b195-93d4bf3e0d63}/02.Sound.Chaser.(2013).flac"
    }

    #[test]
    fn extra_files_sharing_title_id_are_skip_duplicate_title() {
        let listings = [entry(album_rel(), 10), entry(extra_album_rel(), 10)];
        let planned = plan_actions(PlanRequest {
            listings: &listings,
            title_index: &[],
            on_disk: &[],
            open_wants: &[],
            desired: &ds(),
            free_bytes: 2 * Bytes::GIB,
            edge_frozen: false,
        });
        assert_eq!(copies(&planned.actions).len(), 1);
        assert!(
            planned.actions.iter().any(|a| matches!(
                a,
                Action::Skip {
                    reason,
                    ..
                } if reason == SKIP_DUPLICATE_TITLE
            )),
            "{:?}",
            planned.actions
        );
    }

    #[test]
    fn on_disk_schema_files_are_upgrade_never() {
        let title = TitleId::movie("603").expect("id");
        let listings = [entry(&movie_rel("603", 1999), 10)];
        let planned = plan_actions(PlanRequest {
            listings: &listings,
            title_index: &[],
            on_disk: std::slice::from_ref(&title),
            open_wants: &[],
            desired: &ds(),
            free_bytes: 2 * Bytes::GIB,
            edge_frozen: false,
        });
        assert!(copies(&planned.actions).is_empty());
        assert!(
            planned.actions.iter().any(|a| matches!(
                a,
                Action::Skip {
                    title_id: Some(id),
                    reason
                } if *id == title && reason == SKIP_UPGRADE_NEVER
            )),
            "{:?}",
            planned.actions
        );
    }

    #[test]
    fn lock_true_skips_every_candidate() {
        let toml = r#"
schema_version = 1
max_copy_gib = 1
min_free_gib = 1
range_len_mib = 8
max_nvenc = 1
lock = true
"#;
        let desired = DesiredState::from_toml(toml).expect("ds");
        let listings = [entry(&movie_rel("603", 1999), 10)];
        let planned = plan_actions(PlanRequest {
            listings: &listings,
            title_index: &[],
            on_disk: &[],
            open_wants: &[],
            desired: &desired,
            free_bytes: 2 * Bytes::GIB,
            edge_frozen: false,
        });
        assert!(copies(&planned.actions).is_empty());
        assert!(
            planned.actions.iter().all(|a| matches!(
                a,
                Action::Skip {
                    reason,
                    ..
                } if reason == SKIP_LOCK
            )),
            "{:?}",
            planned.actions
        );
        assert!(!planned.first_candidate_breaches);
    }

    #[test]
    fn min_free_zero_does_not_copy_a_file_larger_than_free_disk() {
        let toml = r#"
schema_version = 1
max_copy_gib = 1
min_free_gib = 0
range_len_mib = 8
max_nvenc = 1
lock = false
"#;
        let desired = DesiredState::from_toml(toml).expect("ds");
        let listings = [entry(&movie_rel("603", 1999), 50)];
        let planned = plan_actions(PlanRequest {
            listings: &listings,
            title_index: &[],
            on_disk: &[],
            open_wants: &[],
            desired: &desired,
            free_bytes: 10,
            edge_frozen: false,
        });
        assert!(copies(&planned.actions).is_empty());
        assert!(
            planned.actions.iter().any(|a| matches!(
                a,
                Action::Skip {
                    reason,
                    ..
                } if reason == SKIP_WATERMARK
            )),
            "{:?}",
            planned.actions
        );
        assert!(planned.first_candidate_breaches);
    }

    #[test]
    fn servarr_grabber_emits_grab_apply() {
        let toml = r#"
schema_version = 1
max_copy_gib = 1
min_free_gib = 1
range_len_mib = 8
max_nvenc = 1
lock = false
grabber = "servarr"
"#;
        let desired = DesiredState::from_toml(toml).expect("ds");
        let listings = [entry(&movie_rel("603", 1999), 10)];
        let planned = plan_actions(PlanRequest {
            listings: &listings,
            title_index: &[],
            on_disk: &[],
            open_wants: &[],
            desired: &desired,
            free_bytes: 2 * Bytes::GIB,
            edge_frozen: false,
        });
        assert!(
            matches!(planned.actions.first(), Some(Action::GrabApply)),
            "{:?}",
            planned.actions
        );
    }
}
