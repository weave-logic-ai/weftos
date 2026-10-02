//! Chain subscription: replay-from-sequence plus live events (ADR-103 D).
//!
//! [`ChainManager::subscribe`](crate::chain::ChainManager::subscribe)
//! returns a [`ChainSubscription`]. It yields the retained events from
//! `from_seq`, then live events, with no gap and no duplicate between the
//! two:
//!
//! 1. the live receiver is created and the replay snapshot is taken in one
//!    critical section under the chain lock, and every append publishes to
//!    the hub under that same lock, so the two cannot interleave;
//! 2. the subscription additionally drops any live event whose sequence is
//!    below the snapshot's next sequence (belt and braces).
//!
//! Live delivery uses a bounded `tokio::sync::broadcast`. `send` never
//! blocks, so a stalled subscriber cannot slow an append; it falls behind,
//! the oldest items are overwritten, and its next read yields
//! [`ChainItem::Lagged`] with the number of events it missed. A consumer
//! that needs them can resubscribe with `from_seq`.

use crate::chain::ChainEvent;

/// Live buffer per subscriber (events). A subscriber slower than this
/// many events behind sees [`ChainItem::Lagged`].
pub const CHAIN_SUBSCRIBE_CAPACITY: usize = 1024;

/// Which events a subscription yields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChainFilter {
    /// Only events whose `kind` starts with this prefix (`None` = all).
    pub kind_prefix: Option<String>,
    /// Replay retained events with `sequence >= from_seq` before going
    /// live. `None` = live only.
    pub from_seq: Option<u64>,
}

impl ChainFilter {
    fn matches(&self, e: &ChainEvent) -> bool {
        self.kind_prefix
            .as_deref()
            .is_none_or(|p| e.kind.starts_with(p))
    }
}

/// One item of a subscription.
#[derive(Debug, Clone)]
pub enum ChainItem {
    /// A chain event matching the filter.
    Event(ChainEvent),
    /// The subscriber fell behind; this many events were dropped.
    Lagged(u64),
}

/// Live fan-out owned by the chain. No-op without the `native` feature.
#[derive(Debug)]
pub(crate) struct EventHub {
    #[cfg(feature = "native")]
    tx: tokio::sync::broadcast::Sender<ChainEvent>,
}

impl Default for EventHub {
    fn default() -> Self {
        Self {
            #[cfg(feature = "native")]
            tx: tokio::sync::broadcast::channel(CHAIN_SUBSCRIBE_CAPACITY).0,
        }
    }
}

impl EventHub {
    /// Publish an appended event. Call under the chain lock, before the
    /// next append. Never blocks; clones only when someone is listening.
    #[cfg_attr(not(feature = "native"), allow(unused_variables))]
    pub(crate) fn publish(&self, event: &ChainEvent) {
        #[cfg(feature = "native")]
        if self.tx.receiver_count() > 0 {
            let _ = self.tx.send(event.clone());
        }
    }

    #[cfg(feature = "native")]
    pub(crate) fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ChainEvent> {
        self.tx.subscribe()
    }
}

/// A replay-then-live stream of chain events.
#[cfg(feature = "native")]
#[derive(Debug)]
pub struct ChainSubscription {
    filter: ChainFilter,
    replay: std::collections::VecDeque<ChainEvent>,
    rx: tokio::sync::broadcast::Receiver<ChainEvent>,
    /// Live events below this sequence were already covered by the replay.
    next_seq: u64,
}

#[cfg(feature = "native")]
impl ChainSubscription {
    pub(crate) fn new(
        filter: ChainFilter,
        replay: Vec<ChainEvent>,
        rx: tokio::sync::broadcast::Receiver<ChainEvent>,
        next_seq: u64,
    ) -> Self {
        Self {
            filter,
            replay: replay.into(),
            rx,
            next_seq,
        }
    }

    /// Next item, or `None` when the chain is gone.
    pub async fn next(&mut self) -> Option<ChainItem> {
        use tokio::sync::broadcast::error::RecvError;
        while let Some(e) = self.replay.pop_front() {
            if self.filter.matches(&e) {
                return Some(ChainItem::Event(e));
            }
        }
        loop {
            match self.rx.recv().await {
                Ok(e) if e.sequence < self.next_seq || !self.filter.matches(&e) => continue,
                Ok(e) => {
                    self.next_seq = e.sequence.saturating_add(1);
                    return Some(ChainItem::Event(e));
                }
                Err(RecvError::Lagged(n)) => return Some(ChainItem::Lagged(n)),
                Err(RecvError::Closed) => return None,
            }
        }
    }
}
