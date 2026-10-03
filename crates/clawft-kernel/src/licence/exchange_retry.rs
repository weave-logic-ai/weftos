//! Re-asking a peer whose sync answer never came (ADR-106 phase 3).
//!
//! A sync reply can be lost: dropped by the sender's reply cap in service
//! mode, or by the link. Without a retry the requester would wait for the
//! next periodic sync (30 minutes). A request unanswered for
//! [`PENDING_TTL`] is sent again, fresh, at most [`MAX_SYNC_RETRIES`] times in
//! a row (reset by the next answer), each retry paid from the sync bucket.

use std::time::Duration;

use super::LicenceExchange;
use super::exchange_sync::PENDING_TTL;
use super::exchange_types::Budget;

/// How often expired requests are looked for.
pub(super) const RETRY_SCAN: Duration = Duration::from_secs(30);
/// Unanswered requests re-sent to one peer in a row before giving up until
/// the next periodic or on-connect sync.
pub const MAX_SYNC_RETRIES: u8 = 3;

/// The sync-bucket key retries are charged to (never a connection's).
fn retry_key(peer: &str) -> u64 {
    let h = blake3::hash(peer.as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&h.as_bytes()[..8]);
    u64::from_le_bytes(b) & !1
}

impl LicenceExchange {
    /// Re-send each sync request that went unanswered for [`PENDING_TTL`].
    pub(super) async fn retry_expired(&self) {
        let expired: Vec<String> = self
            .pending
            .iter()
            .filter(|p| p.sent.elapsed() >= PENDING_TTL)
            .map(|p| p.key().clone())
            .collect();
        for peer in expired {
            if self.pending.remove_if(&peer, |_, p| p.sent.elapsed() >= PENDING_TTL).is_none() {
                continue;
            }
            let n = self.retries.get(&peer).map_or(0, |r| *r) + 1;
            if n > MAX_SYNC_RETRIES {
                self.retries.remove(&peer);
                tracing::debug!(%peer, "licence sync unanswered; waiting for the next periodic sync");
                continue;
            }
            if !self.sync_buckets.take(Budget::Sync, retry_key(&peer), 1.0) {
                continue;
            }
            self.retries.insert(peer.clone(), n);
            self.sync_peer(&peer).await;
        }
    }
}
