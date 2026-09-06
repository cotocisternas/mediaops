//! Identity cache: (kind, name) plus UID, revision, tombstones, epoch.

use std::collections::HashMap;

use mediaops_core::{HomeObject, Kind};
use mediaops_home_client::WatchEvent;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectKey {
    pub kind: Kind,
    pub name: String,
}

impl ObjectKey {
    pub fn new(kind: Kind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
        }
    }

    pub fn from_object(obj: &HomeObject) -> Self {
        Self::new(obj.kind, obj.metadata.name.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    pub object: Option<HomeObject>,
    pub uid: String,
    pub resource_version: i64,
    pub tombstone: bool,
    pub epoch: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ObjectCache {
    epoch: u64,
    entries: HashMap<ObjectKey, CacheEntry>,
}

impl ObjectCache {
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn bump_epoch(&mut self) -> u64 {
        self.epoch = self.epoch.saturating_add(1);
        self.epoch
    }

    pub fn get(&self, key: &ObjectKey) -> Option<&CacheEntry> {
        self.entries.get(key)
    }

    pub fn live(&self) -> impl Iterator<Item = &HomeObject> {
        self.entries.values().filter_map(|e| e.object.as_ref())
    }

    pub fn live_kind(&self, kind: Kind) -> impl Iterator<Item = &HomeObject> {
        self.live().filter(move |o| o.kind == kind)
    }

    /// Replace the cache from a successful List baseline for this epoch.
    pub fn install_baseline(&mut self, epoch: u64, objects: Vec<HomeObject>) {
        if epoch != self.epoch {
            return;
        }
        let overlay: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.epoch == epoch)
            .map(|(key, entry)| (key.clone(), entry.clone()))
            .collect();
        self.entries.clear();
        for obj in objects {
            self.upsert_live(epoch, obj);
        }
        for (key, entry) in overlay {
            if self
                .entries
                .get(&key)
                .is_none_or(|baseline| entry.resource_version >= baseline.resource_version)
            {
                self.entries.insert(key, entry);
            }
        }
    }

    pub fn apply_event(&mut self, epoch: u64, event: WatchEvent) -> bool {
        if epoch != self.epoch {
            return false;
        }
        let obj = event.object();
        let key = ObjectKey::from_object(obj);
        let incoming_rv = obj.metadata.resource_version;
        let incoming_uid = obj.metadata.uid.as_str();
        if let Some(existing) = self.entries.get(&key)
            && !accept_revision(existing, incoming_rv, incoming_uid, &event)
        {
            return false;
        }
        match event {
            WatchEvent::Added(obj) | WatchEvent::Modified(obj) => {
                self.upsert_live(epoch, obj);
            }
            WatchEvent::Deleted(obj) => {
                self.entries.insert(
                    key,
                    CacheEntry {
                        uid: obj.metadata.uid,
                        resource_version: obj.metadata.resource_version,
                        tombstone: true,
                        epoch,
                        object: None,
                    },
                );
            }
        }
        true
    }

    fn upsert_live(&mut self, epoch: u64, obj: HomeObject) {
        let key = ObjectKey::from_object(&obj);
        self.entries.insert(
            key,
            CacheEntry {
                uid: obj.metadata.uid.clone(),
                resource_version: obj.metadata.resource_version,
                tombstone: false,
                epoch,
                object: Some(obj),
            },
        );
    }
}

fn accept_revision(
    existing: &CacheEntry,
    incoming_rv: i64,
    incoming_uid: &str,
    event: &WatchEvent,
) -> bool {
    if existing.tombstone && incoming_uid == existing.uid {
        return false;
    }
    if incoming_rv < existing.resource_version {
        return false;
    }
    if incoming_rv == existing.resource_version && incoming_uid == existing.uid {
        return matches!(event, WatchEvent::Deleted(_)) && !existing.tombstone;
    }
    true
}

#[cfg(test)]
mod tests {
    use mediaops_core::{Spec, StatusBody, WantSpec, WantStatus};

    use super::*;

    fn want(name: &str, uid: &str, rv: i64) -> HomeObject {
        let mut obj = HomeObject::new(
            Kind::Want,
            name,
            Spec::Want(WantSpec {
                title_id: name.into(),
            }),
            StatusBody::Want(WantStatus::default()),
        );
        obj.metadata.uid = uid.into();
        obj.metadata.resource_version = rv;
        obj
    }

    #[test]
    fn empty_baseline_is_known_empty() {
        let mut cache = ObjectCache::default();
        let epoch = cache.bump_epoch();
        cache.install_baseline(epoch, Vec::new());
        assert_eq!(cache.live().count(), 0);
    }

    #[test]
    fn old_revision_does_not_regress() {
        let mut cache = ObjectCache::default();
        let epoch = cache.bump_epoch();
        cache.install_baseline(epoch, vec![want("movie:tmdb:1", "a", 10)]);
        assert!(!cache.apply_event(epoch, WatchEvent::Modified(want("movie:tmdb:1", "a", 9))));
        assert_eq!(
            cache
                .get(&ObjectKey::new(Kind::Want, "movie:tmdb:1"))
                .expect("row")
                .resource_version,
            10
        );
    }

    #[test]
    fn delete_then_recreate_replaces_uid() {
        let mut cache = ObjectCache::default();
        let epoch = cache.bump_epoch();
        cache.install_baseline(epoch, vec![want("movie:tmdb:1", "old", 4)]);
        assert!(cache.apply_event(epoch, WatchEvent::Deleted(want("movie:tmdb:1", "old", 5))));
        assert!(cache.apply_event(epoch, WatchEvent::Added(want("movie:tmdb:1", "new", 6))));
        let entry = cache
            .get(&ObjectKey::new(Kind::Want, "movie:tmdb:1"))
            .expect("row");
        assert_eq!(entry.uid, "new");
        assert!(!entry.tombstone);
        assert!(entry.object.is_some());
    }

    #[test]
    fn tombstone_does_not_resurrect_old_uid() {
        let mut cache = ObjectCache::default();
        let epoch = cache.bump_epoch();
        cache.install_baseline(epoch, vec![want("movie:tmdb:1", "old", 4)]);
        assert!(cache.apply_event(epoch, WatchEvent::Deleted(want("movie:tmdb:1", "old", 8))));
        assert!(!cache.apply_event(epoch, WatchEvent::Added(want("movie:tmdb:1", "old", 8))));
        assert!(
            cache
                .get(&ObjectKey::new(Kind::Want, "movie:tmdb:1"))
                .expect("row")
                .tombstone
        );
    }

    #[test]
    fn foreign_epoch_is_rejected() {
        let mut cache = ObjectCache::default();
        let epoch = cache.bump_epoch();
        cache.install_baseline(epoch, Vec::new());
        assert!(!cache.apply_event(epoch + 1, WatchEvent::Added(want("movie:tmdb:1", "a", 1))));
        assert_eq!(cache.live().count(), 0);
    }
}
