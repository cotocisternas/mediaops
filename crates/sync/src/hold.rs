//! Inbox join: live ⊖ decided. The only place the join runs (AD-8).

use std::collections::HashSet;

use mediaops_core::{HoldKey, HoldLiveItem};

/// Undecided live items, preserving live order.
pub fn inbox(live: &[HoldLiveItem], decided: &[HoldKey]) -> Vec<HoldLiveItem> {
    let decided: HashSet<&HoldKey> = decided.iter().collect();
    live.iter()
        .filter(|item| !decided.contains(&item.key))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{
        BoxFuture, ControlError, ControlPort, DeleteRemoteOutcome, DfSnapshot, EdgeApiReport,
        GrabApplyReport, HoldDecision, HoldKey, HoldLiveItem, KeyPresence, ReleaseId, RemoteRef,
        TitleId,
    };
    use std::sync::Mutex;

    fn item(title: &str, release: &str) -> HoldLiveItem {
        HoldLiveItem::new(
            HoldKey::new(
                TitleId::parse(title).expect("title"),
                ReleaseId::parse(release).expect("release"),
            ),
            1_577_836_800,
            100,
            "No files found are eligible for import",
        )
    }

    #[test]
    fn empty_live_is_empty_inbox() {
        assert!(inbox(&[], &[]).is_empty());
        assert!(inbox(&[], &[item("movie:tmdb:603", "abc").key]).is_empty());
    }

    #[test]
    fn two_live_zero_decided_lists_both_with_age_size_reason() {
        let live = [
            item("movie:tmdb:603", "aaa"),
            item("series:tvdb:79126", "bbb"),
        ];
        let listed = inbox(&live, &[]);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].key.release_id.as_str(), "aaa");
        assert_eq!(listed[1].key.release_id.as_str(), "bbb");
        assert_eq!(listed[0].age_secs(1_577_836_850), 50);
        assert_eq!(listed[0].size, 100);
        assert!(!listed[0].reason.is_empty());
    }

    #[test]
    fn two_live_one_decided_is_the_undecided_remainder() {
        let live = [
            item("movie:tmdb:603", "aaa"),
            item("series:tvdb:79126", "bbb"),
        ];
        let decided = [live[0].key.clone()];
        let listed = inbox(&live, &decided);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key, live[1].key);
    }

    struct FakeControl {
        live: Vec<HoldLiveItem>,
        calls: Mutex<usize>,
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
            _: &'a RemoteRef,
        ) -> BoxFuture<'a, Result<DeleteRemoteOutcome, ControlError>> {
            Box::pin(async { Ok(DeleteRemoteOutcome::Deleted) })
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
        fn guard_preview(&self) -> BoxFuture<'_, Result<(), ControlError>> {
            Box::pin(async { Ok(()) })
        }
        fn hold_list(&self) -> BoxFuture<'_, Result<Vec<HoldLiveItem>, ControlError>> {
            Box::pin(async {
                *self.calls.lock().expect("lock") += 1;
                Ok(self.live.clone())
            })
        }
        fn hold_reject<'a>(&'a self, _: &'a HoldKey) -> BoxFuture<'a, Result<(), ControlError>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn fake_control_hold_list_feeds_inbox_join() {
        let live = vec![
            item("movie:tmdb:603", "aaa"),
            item("series:tvdb:79126", "bbb"),
        ];
        let control = FakeControl {
            live: live.clone(),
            calls: Mutex::new(0),
        };
        let from_port = control.hold_list().await.expect("live");
        assert_eq!(*control.calls.lock().expect("lock"), 1);
        let decided = [live[1].key.clone()];
        let listed = inbox(&from_port, &decided);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key.release_id.as_str(), "aaa");
        let _ = HoldDecision::Rejected;
    }
}
