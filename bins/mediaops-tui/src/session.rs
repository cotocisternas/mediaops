//! One owned Watch(0) subscription per connection epoch.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mediaops_core::HomeObject;
use mediaops_home_client::{HomeApi, WatchEvent, default_api_socket};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::cache::ObjectCache;
use crate::model::SyncState;

#[derive(Debug)]
pub enum SessionEvent {
    ConnectFailed {
        epoch: u64,
        message: String,
    },
    Connected {
        epoch: u64,
        api: HomeApi,
    },
    Watch {
        epoch: u64,
        event: Box<WatchEvent>,
    },
    WatchEnded {
        epoch: u64,
    },
    WatchFailed {
        epoch: u64,
        message: String,
    },
    Baseline {
        epoch: u64,
        objects: Vec<HomeObject>,
    },
    BaselineFailed {
        epoch: u64,
        message: String,
    },
}

impl SessionEvent {
    pub const fn epoch(&self) -> u64 {
        match self {
            Self::ConnectFailed { epoch, .. }
            | Self::Connected { epoch, .. }
            | Self::Watch { epoch, .. }
            | Self::WatchEnded { epoch }
            | Self::WatchFailed { epoch, .. }
            | Self::Baseline { epoch, .. }
            | Self::BaselineFailed { epoch, .. } => *epoch,
        }
    }
}

pub struct Session {
    socket: PathBuf,
    pub api: Option<HomeApi>,
    pub cache: ObjectCache,
    pub sync: SyncState,
    pub list_failed: bool,
    pub message: Option<String>,
    pub needs_reconnect: bool,
    failed_epoch: Option<u64>,
    failures: u32,
    tx: mpsc::Sender<SessionEvent>,
    rx: mpsc::Receiver<SessionEvent>,
    task: Option<JoinHandle<()>>,
}

impl Session {
    pub fn new(socket: Option<&Path>) -> Self {
        let (tx, rx) = mpsc::channel(64);
        Self {
            socket: socket
                .map(Path::to_path_buf)
                .unwrap_or_else(default_api_socket),
            api: None,
            cache: ObjectCache::default(),
            sync: SyncState::Connecting,
            list_failed: false,
            message: None,
            needs_reconnect: false,
            failed_epoch: None,
            failures: 0,
            tx,
            rx,
            task: None,
        }
    }

    pub fn backoff(&self) -> Duration {
        Duration::from_secs(1_u64 << self.failures.saturating_sub(1).min(3))
    }

    pub async fn recv(&mut self) -> Option<SessionEvent> {
        self.rx.recv().await
    }

    pub fn try_recv(&mut self) -> Option<SessionEvent> {
        self.rx.try_recv().ok()
    }

    pub fn apply_event(&mut self, event: SessionEvent) {
        let epoch = event.epoch();
        if epoch != self.cache.epoch() || self.failed_epoch == Some(epoch) {
            return;
        }
        match event {
            SessionEvent::ConnectFailed { message, .. } => {
                self.fail(epoch, message);
                if self.cache.live().next().is_none() {
                    self.sync = SyncState::Connecting;
                }
            }
            SessionEvent::Connected { api, .. } => self.api = Some(api),
            SessionEvent::Baseline { objects, .. } => {
                self.cache.install_baseline(epoch, objects);
                self.sync = SyncState::Current;
                self.list_failed = false;
                self.message = None;
                self.needs_reconnect = false;
                self.failures = 0;
            }
            SessionEvent::Watch { event, .. } => {
                self.cache.apply_event(epoch, *event);
            }
            SessionEvent::BaselineFailed { message, .. } => {
                self.list_failed = true;
                self.fail(epoch, message);
            }
            SessionEvent::WatchEnded { .. } => self.fail(epoch, "subscription ended".into()),
            SessionEvent::WatchFailed { message, .. } => self.fail(epoch, message),
        }
    }

    fn fail(&mut self, epoch: u64, message: String) {
        self.failed_epoch = Some(epoch);
        self.sync = SyncState::Stale;
        self.api = None;
        self.message = Some(message);
        self.needs_reconnect = true;
        self.failures = self.failures.saturating_add(1);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    /// Starts owned asynchronous work; never waits for its own event queue.
    pub async fn bootstrap(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let epoch = self.cache.bump_epoch();
        self.failed_epoch = None;
        self.api = None;
        self.sync = if self.cache.live().next().is_some() {
            SyncState::Stale
        } else {
            SyncState::Synchronizing
        };
        self.needs_reconnect = false;
        let socket = self.socket.clone();
        let tx = self.tx.clone();
        self.task = Some(tokio::spawn(async move {
            crate::subscription::run(socket, epoch, tx).await;
        }));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
