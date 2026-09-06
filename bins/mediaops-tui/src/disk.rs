use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;

pub const DISK_REFRESH: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskObservation {
    Unavailable { age: Duration },
    Ready { bytes: u64, age: Duration },
}

impl DiskObservation {
    pub const fn unavailable() -> Self {
        Self::Unavailable {
            age: Duration::ZERO,
        }
    }
}

#[derive(Default)]
pub struct DiskWatch {
    root: Option<PathBuf>,
    last: Option<Instant>,
    value: Option<u64>,
    tasks: JoinSet<(PathBuf, Option<u64>)>,
}

impl DiskWatch {
    pub fn update(&mut self, root: Option<&str>) -> bool {
        let root = root.map(PathBuf::from);
        let mut changed = false;
        if root != self.root {
            self.root = root;
            self.value = None;
            self.last = None;
            changed = true;
        }
        while let Some(result) = self.tasks.try_join_next() {
            if let Ok((root, value)) = result
                && self.root.as_ref() == Some(&root)
            {
                self.last = Some(Instant::now());
                self.value = value;
                changed = true;
            }
        }
        if self.tasks.is_empty()
            && self.last.is_none_or(|last| last.elapsed() >= DISK_REFRESH)
            && let Some(root) = self.root.clone()
        {
            self.last = Some(Instant::now());
            self.tasks.spawn_blocking(move || {
                let value = mediaops_core::free_bytes(&root).ok();
                (root, value)
            });
        }
        changed
    }

    pub fn observation(&self) -> DiskObservation {
        let age = self.last.map(|last| last.elapsed()).unwrap_or_default();
        match self.value {
            Some(bytes) => DiskObservation::Ready { bytes, age },
            None => DiskObservation::Unavailable { age },
        }
    }
}
