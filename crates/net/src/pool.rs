//! N independent channels; one in-flight stream per slot (AD-12).

use std::sync::Arc;

use tokio::sync::{Mutex, OwnedMutexGuard};
use tonic::transport::Channel;

use crate::NetError;

pub struct ChannelPool {
    slots: Vec<Arc<Mutex<Channel>>>,
}

pub struct SlotGuard {
    guard: OwnedMutexGuard<Channel>,
}

impl ChannelPool {
    pub fn new(channels: Vec<Channel>) -> Result<Self, NetError> {
        if channels.is_empty() {
            return Err(NetError::Pool("channel pool requires N >= 1".into()));
        }
        Ok(Self {
            slots: channels
                .into_iter()
                .map(|ch| Arc::new(Mutex::new(ch)))
                .collect(),
        })
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// One in-flight checkout per slot. N+1 is exhausted, never queued onto a shared channel.
    pub fn try_checkout(&self) -> Result<SlotGuard, NetError> {
        for slot in &self.slots {
            if let Ok(guard) = slot.clone().try_lock_owned() {
                return Ok(SlotGuard { guard });
            }
        }
        Err(NetError::Exhausted)
    }
}

impl SlotGuard {
    pub fn channel(&self) -> &Channel {
        &self.guard
    }
}
