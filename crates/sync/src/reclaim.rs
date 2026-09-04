//! Reclaim apply: DeleteRemote through Control. Preview ranking lives in core.

use mediaops_core::{ControlError, ControlPort, DeleteRemoteOutcome, RemoteRef};

pub use mediaops_core::{ReclaimCandidate, reclaim_actions as preview_actions, reclaim_preview};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReclaimReport {
    pub deleted: usize,
    pub skipped_seeding: usize,
    pub qbit_unavailable: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

pub async fn apply_reclaim(
    control: &dyn ControlPort,
    remotes: &[RemoteRef],
) -> Result<ReclaimReport, ControlError> {
    let mut report = ReclaimReport::default();
    for remote in remotes {
        match control.delete_remote(remote).await {
            Ok(DeleteRemoteOutcome::Deleted) => report.deleted += 1,
            Ok(DeleteRemoteOutcome::SkippedSeeding) => report.skipped_seeding += 1,
            Ok(DeleteRemoteOutcome::QbitUnavailable) => report.qbit_unavailable += 1,
            Err(err) => {
                report.failed += 1;
                report.errors.push(err.to_string());
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::digest::Blake3Hex;
    use mediaops_core::{
        Action, BoxFuture, ControlPort, DfSnapshot, EdgeApiReport, GrabApplyReport,
        GuardPreviewItem, KeyPresence, RemoteEntry, TitleId, TitleIndexEntry,
    };
    use std::path::PathBuf;
    use std::sync::Mutex;

    fn digest() -> Blake3Hex {
        Blake3Hex::parse(&"a".repeat(64)).expect("digest")
    }

    fn rel() -> &'static str {
        "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv"
    }

    struct FakeControl {
        deletes: Mutex<Vec<RemoteRef>>,
        results: Mutex<Vec<Result<DeleteRemoteOutcome, ControlError>>>,
    }

    impl ControlPort for FakeControl {
        fn df(&self) -> BoxFuture<'_, Result<DfSnapshot, ControlError>> {
            Box::pin(async {
                Ok(DfSnapshot {
                    free: mediaops_core::Bytes::new(0),
                    semver: "0.1.0".into(),
                    proto_package: "mediaops.v1".into(),
                })
            })
        }
        fn unmonitor<'a>(&'a self, _: &'a TitleId) -> BoxFuture<'a, Result<(), ControlError>> {
            Box::pin(async { Ok(()) })
        }
        fn delete_remote<'a>(
            &'a self,
            remote: &'a RemoteRef,
        ) -> BoxFuture<'a, Result<DeleteRemoteOutcome, ControlError>> {
            self.deletes.lock().expect("lock").push(remote.clone());
            let next = self.results.lock().expect("lock").remove(0);
            Box::pin(async move { next })
        }
        fn grab_apply<'a>(
            &'a self,
            _: &'a [u8],
        ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>> {
            Box::pin(async {
                Ok(GrabApplyReport {
                    noop: true,
                    diff: String::new(),
                })
            })
        }
        fn edge_check(&self) -> BoxFuture<'_, Result<EdgeApiReport, ControlError>> {
            Box::pin(async {
                Ok(EdgeApiReport {
                    fingerprint: String::new(),
                    invariant_ok: true,
                    drift: String::new(),
                })
            })
        }
        fn edge_apply<'a>(
            &'a self,
            _: &'a [u8],
        ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>> {
            Box::pin(async {
                Ok(GrabApplyReport {
                    noop: true,
                    diff: String::new(),
                })
            })
        }
        fn key_discovery(&self) -> BoxFuture<'_, Result<KeyPresence, ControlError>> {
            Box::pin(async { Ok(KeyPresence::default()) })
        }
        fn guard_preview(&self) -> BoxFuture<'_, Result<Vec<GuardPreviewItem>, ControlError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn hold_list(
            &self,
        ) -> BoxFuture<'_, Result<Vec<mediaops_core::HoldLiveItem>, ControlError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn hold_reject<'a>(
            &'a self,
            _: &'a mediaops_core::HoldKey,
        ) -> BoxFuture<'a, Result<(), ControlError>> {
            Box::pin(async { Ok(()) })
        }
        fn wanted_missing(&self) -> BoxFuture<'_, Result<Vec<TitleId>, ControlError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    #[tokio::test]
    async fn apply_reclaim_records_deleted_and_skipped_seeding() {
        let remote =
            RemoteRef::from_wire_parts("seedbox".into(), PathBuf::from(rel())).expect("ref");
        let control = FakeControl {
            deletes: Mutex::new(Vec::new()),
            results: Mutex::new(vec![Ok(DeleteRemoteOutcome::Deleted)]),
        };
        let report = apply_reclaim(&control, std::slice::from_ref(&remote))
            .await
            .expect("apply");
        assert_eq!(report.deleted, 1);
        assert_eq!(report.skipped_seeding, 0);
        assert_eq!(control.deletes.lock().expect("lock").len(), 1);

        let skip = FakeControl {
            deletes: Mutex::new(Vec::new()),
            results: Mutex::new(vec![Ok(DeleteRemoteOutcome::SkippedSeeding)]),
        };
        let report = apply_reclaim(&skip, std::slice::from_ref(&remote))
            .await
            .expect("skip");
        assert_eq!(report.skipped_seeding, 1);
        assert_eq!(report.deleted, 0);
    }

    #[tokio::test]
    async fn apply_reclaim_continues_after_a_delete_error() {
        let a = RemoteRef::from_wire_parts("seedbox".into(), PathBuf::from(rel())).expect("a");
        let b = RemoteRef::from_wire_parts(
            "seedbox".into(),
            PathBuf::from("movies/Other.(2000)/Other.(2000).mkv"),
        )
        .expect("b");
        let control = FakeControl {
            deletes: Mutex::new(Vec::new()),
            results: Mutex::new(vec![
                Err(ControlError::runtime("qbit 500")),
                Ok(DeleteRemoteOutcome::Deleted),
            ]),
        };
        let report = apply_reclaim(&control, &[a.clone(), b.clone()])
            .await
            .expect("continue");
        assert_eq!(report.failed, 1);
        assert_eq!(report.deleted, 1);
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].contains("qbit 500"), "{:?}", report.errors);
        assert_eq!(control.deletes.lock().expect("lock").as_slice(), &[a, b]);
    }

    #[test]
    fn preview_actions_are_delete_remote_not_reclaim_unit() {
        // Remote is root-relative under a movies root; local is library-relative.
        let remote = RemoteRef::from_wire_parts(
            "seedbox".into(),
            PathBuf::from("The.Matrix.(1999)/The.Matrix.(1999).mkv"),
        )
        .expect("ref");
        let listings = vec![RemoteEntry::from_wire_parts(remote.clone(), 10, 1, 1)];
        let kinds = mediaops_core::RootKinds::from([(
            "seedbox".to_string(),
            Some(mediaops_core::TitleKind::Movie),
        )]);
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let title_index = vec![TitleIndexEntry::new(
            title.clone(),
            rel(),
            digest(),
            digest(),
        )];
        let on_disk = vec![
            mediaops_core::InstalledFile::from_rel_path(std::path::Path::new(rel())).expect("file"),
        ];
        let actions = preview_actions(&listings, &kinds, &title_index, &on_disk, &[]);
        assert_eq!(actions, vec![Action::DeleteRemote { remote }]);
        assert!(!actions.iter().any(|a| matches!(a, Action::Reclaim)));
    }
}
