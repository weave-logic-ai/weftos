//! What an Admin `reset_floor` would change, shown before it is confirmed.
//!
//! A reset forgets the clock high-water mark of the bound grant key and
//! restarts it from now. A grant that the floor currently holds expired can
//! then be valid again; the operator should see which before agreeing.

use serde::Serialize;

use super::store::{CheckoutGrantStore, grant_valid};
use super::LicenceError;

/// A grant a reset would make valid again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RevivedGrant {
    /// Cog id.
    pub cog_id: String,
    /// Version.
    pub version: String,
    /// When the grant expires, unix seconds.
    pub expires_at: u64,
}

/// The floor now, after a reset, and the grants that would come back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FloorPreview {
    /// The local clock, unix seconds.
    pub now: u64,
    /// The floor now (`max(highest issued_at, high-water mark)`).
    pub floor: u64,
    /// The floor right after a reset.
    pub floor_after: u64,
    /// Grants expired by the floor now that a reset would make valid.
    pub revived: Vec<RevivedGrant>,
}

impl CheckoutGrantStore {
    /// Preview [`Self::reset_floor`] without changing anything. Needs a
    /// binding in effect, like the reset.
    pub fn floor_preview(&self) -> Result<FloorPreview, LicenceError> {
        self.run(|inner, ev| {
            let b = self.binding_in_effect(inner, ev)?;
            let now = (self.clock)();
            let cur = inner.floors.get(&b.grant_pubkey).copied().unwrap_or_default();
            let mut after = cur;
            after.reset(now);
            let (eff_now, eff_after) = (cur.effective_now(now), after.effective_now(now));
            let mut revived: Vec<RevivedGrant> = inner
                .slots
                .values()
                .filter_map(|s| s.current.as_ref())
                .filter(|h| {
                    h.body.mesh_id == b.mesh_id
                        && !grant_valid(&h.body, eff_now)
                        && grant_valid(&h.body, eff_after)
                })
                .map(|h| RevivedGrant {
                    cog_id: h.body.cog_id.clone(),
                    version: h.body.version.clone(),
                    expires_at: h.body.expires_at,
                })
                .collect();
            revived.sort_by(|a, c| (&a.cog_id, &a.version).cmp(&(&c.cog_id, &c.version)));
            Ok(FloorPreview { now, floor: cur.floor(), floor_after: after.floor(), revived })
        })
    }
}
