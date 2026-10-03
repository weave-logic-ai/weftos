//! Chain subscription: replay-from-sequence plus live events (ADR-103 D).
//!
//! [`ChainManager::subscribe`](crate::chain::ChainManager::subscribe)
//! returns a [`ChainSubscription`]. It yields the retained events from
//! `from_seq`, then live events, with no gap and no duplicate between the
//! two:
//!
//! 1. under the chain lock, `subscribe` creates the live receiver and
//!    records the snapshot end (the chain's next sequence); every append
//!    publishes to the hub under that same lock, so they cannot interleave;
//! 2. the replay is then paged in short lock holds (`REPLAY_PAGE` events,
//!    at most `REPLAY_SCAN` scanned per hold) over `[from_seq, end)`;
//! 3. live events below `end` are dropped (belt and braces).
//!
//! A deep `from_seq` therefore never clones the chain under the lock, and a
//! `from_seq` older than the replay window is refused with a typed error.
//!
//! Live delivery uses a bounded `tokio::sync::broadcast`. `send` never
//! blocks, so a stalled subscriber cannot slow an append; it falls behind,
//! the oldest items are overwritten, and its next read yields
//! [`ChainItem::Lagged`] carrying `resume_from`, the sequence to
//! resubscribe from to recover. The subscription holds only a `Weak`
//! reference to the manager and ends when the manager is dropped.

use crate::chain::ChainEvent;

/// Live buffer per subscriber (events). A subscriber slower than this
/// many events behind sees [`ChainItem::Lagged`].
pub const CHAIN_SUBSCRIBE_CAPACITY: usize = 1024;

/// Default replay window: the furthest back (in events behind the head) a
/// subscriber may start. Adjust with `ChainManager::set_max_replay_window`.
pub const DEFAULT_MAX_REPLAY_WINDOW: u64 = 100_000;

/// Events returned per replay lock hold.
#[cfg(feature = "native")]
const REPLAY_PAGE: usize = 256;
/// Retained events examined per replay lock hold (bounds hold time when
/// `kind_prefix` rejects most events).
#[cfg(feature = "native")]
const REPLAY_SCAN: usize = 4096;

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
    #[cfg(feature = "native")]
    fn matches(&self, e: &ChainEvent) -> bool {
        self.kind_prefix
            .as_deref()
            .is_none_or(|p| e.kind.starts_with(p))
    }
}

/// Why a subscription was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SubscribeError {
    /// `from_seq` is further back than the replay window allows.
    #[error("from_seq {requested} is older than the replay window (oldest allowed {oldest_allowed})")]
    ReplayWindowExceeded { requested: u64, oldest_allowed: u64 },
}

/// One item of a subscription.
#[derive(Debug, Clone)]
// Public fan-out item cloned per subscriber; boxing the event would change the public API and add an allocation per event.
#[allow(clippy::large_enum_variant)]
pub enum ChainItem {
    /// A chain event matching the filter.
    Event(ChainEvent),
    /// The subscriber fell behind and `missed` events were dropped.
    /// Resubscribe with `from_seq = resume_from` to recover them.
    Lagged { missed: u64, resume_from: u64 },
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
    chain: std::sync::Weak<crate::chain::ChainManager>,
    /// Next sequence to page from the replay; replay is done at `end`.
    cursor: u64,
    /// Snapshot end: replay covers `[from_seq, end)`, live starts at `end`.
    end: u64,
    page: std::collections::VecDeque<ChainEvent>,
    rx: tokio::sync::broadcast::Receiver<ChainEvent>,
    /// Sequence after the last event this subscription has passed.
    resume_from: u64,
}

#[cfg(feature = "native")]
impl ChainSubscription {
    pub(crate) fn new(
        filter: ChainFilter,
        chain: std::sync::Weak<crate::chain::ChainManager>,
        rx: tokio::sync::broadcast::Receiver<ChainEvent>,
        end: u64,
    ) -> Self {
        let cursor = filter.from_seq.unwrap_or(end).min(end);
        Self {
            filter,
            chain,
            cursor,
            end,
            page: Default::default(),
            rx,
            resume_from: cursor,
        }
    }

    /// Next item, or `None` when the chain is gone.
    pub async fn next(&mut self) -> Option<ChainItem> {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            if let Some(e) = self.page.pop_front() {
                self.resume_from = e.sequence + 1;
                return Some(ChainItem::Event(e));
            }
            if self.cursor >= self.end {
                break;
            }
            let chain = self.chain.upgrade()?;
            let (page, next) = chain.replay_page(
                self.cursor,
                self.end,
                self.filter.kind_prefix.as_deref(),
                REPLAY_PAGE,
                REPLAY_SCAN,
            );
            // Always make progress; a page that did not advance means
            // nothing retained is left below `end`.
            self.cursor = if next > self.cursor { next } else { self.end };
            self.page.extend(page);
        }
        loop {
            match self.rx.recv().await {
                Ok(e) if e.sequence < self.end => continue,
                Ok(e) => {
                    self.resume_from = e.sequence + 1;
                    if !self.filter.matches(&e) {
                        continue;
                    }
                    return Some(ChainItem::Event(e));
                }
                Err(RecvError::Lagged(n)) => {
                    return Some(ChainItem::Lagged {
                        missed: n,
                        resume_from: self.resume_from,
                    });
                }
                Err(RecvError::Closed) => return None,
            }
        }
    }
}
