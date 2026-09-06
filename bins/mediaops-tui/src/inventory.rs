//! Hold inbox and RemoteFile listing require a ready inventory generation.

use mediaops_core::{HoldDecisionSpec, HomeObject, Spec, StatusBody, WorkerKind, node_is_ready};

pub fn committed_inventory_generation<'a>(
    objects: impl Iterator<Item = &'a HomeObject>,
    now_unix: i64,
) -> Option<i64> {
    objects
        .filter_map(|obj| match (&obj.spec, &obj.status) {
            (Spec::Node(spec), StatusBody::Node(status))
                if spec.worker_kind == WorkerKind::Inventory
                    && status.list_generation > 0
                    && node_is_ready(status.ready, status.last_heartbeat_unix, now_unix)
                    && node_is_ready(true, status.list_completed_unix, now_unix) =>
            {
                Some(status.list_generation)
            }
            _ => None,
        })
        .next()
}

pub fn open_holds<'a>(
    objects: impl Iterator<Item = &'a HomeObject>,
    generation: i64,
) -> Vec<&'a HomeObject> {
    objects
        .filter(|obj| match (&obj.spec, &obj.status) {
            (Spec::Hold(spec), StatusBody::Hold(status)) => {
                spec.decision == HoldDecisionSpec::Empty && status.list_generation == generation
            }
            _ => false,
        })
        .collect()
}

pub fn current_remote_files<'a>(
    objects: impl Iterator<Item = &'a HomeObject>,
    generation: i64,
) -> Vec<&'a HomeObject> {
    objects
        .filter(|obj| match &obj.status {
            StatusBody::RemoteFile(status) => status.list_generation == generation,
            _ => false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use mediaops_core::{
        HoldSpec, HoldStatus, Kind, NODE_NOTREADY_SECS, NodeSpec, NodeStatus, RemoteFileStatus,
    };

    use super::*;

    fn inventory(ready: bool, generation: i64, beat: i64, listed: i64) -> HomeObject {
        HomeObject::new(
            Kind::Node,
            "inventory",
            Spec::Node(NodeSpec {
                worker_kind: WorkerKind::Inventory,
            }),
            StatusBody::Node(NodeStatus {
                list_generation: generation,
                list_completed_unix: listed,
                ready,
                last_heartbeat_unix: beat,
            }),
        )
    }

    fn hold(name: &str, title: &str, generation: i64, empty: bool) -> HomeObject {
        HomeObject::new(
            Kind::Hold,
            name,
            Spec::Hold(HoldSpec {
                title_id: title.into(),
                release_id: "rel".into(),
                decision: if empty {
                    HoldDecisionSpec::Empty
                } else {
                    HoldDecisionSpec::Approved
                },
            }),
            StatusBody::Hold(HoldStatus {
                list_generation: generation,
                ..HoldStatus::default()
            }),
        )
    }

    #[test]
    fn missing_or_stale_inventory_is_unavailable() {
        let now = 1_000i64;
        let stale = inventory(true, 4, now - NODE_NOTREADY_SECS as i64, now);
        assert!(committed_inventory_generation(std::iter::once(&stale), now).is_none());
        let future_list = inventory(true, 4, now, now + 5);
        assert!(committed_inventory_generation(std::iter::once(&future_list), now).is_none());
        let zero = inventory(true, 0, now, now);
        assert!(committed_inventory_generation(std::iter::once(&zero), now).is_none());
    }

    #[test]
    fn ready_generation_projects_only_matching_empty_holds() {
        let now = 50i64;
        let node = inventory(true, 4, now, now);
        let live = hold("movie:tmdb:1-a", "movie:tmdb:1", 4, true);
        let old = hold("movie:tmdb:1-b", "movie:tmdb:1", 3, true);
        let decided = hold("movie:tmdb:2-a", "movie:tmdb:2", 4, false);
        let objects = [&node, &live, &old, &decided];
        let generation =
            committed_inventory_generation(objects.into_iter(), now).expect("generation");
        let inbox = open_holds(objects.into_iter(), generation);
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].metadata.name, "movie:tmdb:1-a");
    }

    #[test]
    fn remotefiles_use_the_same_generation() {
        let mut file = HomeObject::new(
            Kind::RemoteFile,
            "movies/a.mkv",
            Spec::RemoteFile,
            StatusBody::RemoteFile(RemoteFileStatus {
                list_generation: 4,
                rel_path: "a.mkv".into(),
                root_id: "movies".into(),
                ..RemoteFileStatus::default()
            }),
        );
        let now = 10i64;
        let node = inventory(true, 4, now, now);
        assert_eq!(current_remote_files([&node, &file].into_iter(), 4).len(), 1);
        if let StatusBody::RemoteFile(status) = &mut file.status {
            status.list_generation = 5;
        }
        assert!(current_remote_files([&node, &file].into_iter(), 4).is_empty());
    }
}
