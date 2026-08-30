//! N independent channels; one in-flight stream per slot (AD-12).

use tokio::sync::{Mutex, MutexGuard};
use tonic::transport::Channel;

use crate::NetError;

pub struct ChannelPool {
    slots: Vec<Mutex<Channel>>,
}

pub struct SlotGuard<'a> {
    guard: MutexGuard<'a, Channel>,
}

impl ChannelPool {
    pub fn new(channels: Vec<Channel>) -> Result<Self, NetError> {
        if channels.is_empty() {
            return Err(NetError::Pool("channel pool requires N >= 1".into()));
        }
        Ok(Self {
            slots: channels.into_iter().map(Mutex::new).collect(),
        })
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// One in-flight checkout per slot. N+1 is exhausted, never queued onto a shared channel.
    pub fn try_checkout(&self) -> Result<SlotGuard<'_>, NetError> {
        for slot in &self.slots {
            if let Ok(guard) = slot.try_lock() {
                return Ok(SlotGuard { guard });
            }
        }
        Err(NetError::Exhausted)
    }
}

impl SlotGuard<'_> {
    pub fn channel(&self) -> &Channel {
        &self.guard
    }
}
