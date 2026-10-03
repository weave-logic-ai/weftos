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
            Ok(preview_of(inner, &b.grant_pubkey, &b.mesh_id, (self.clock)()))
        })
    }

    /// Reset the floor and return the preview it was applied on, in one
    /// critical section. With `expected_floor`, refuses (`floor_changed`)
    /// when the floor is no longer the value the operator was shown.
    pub fn reset_floor_checked(&self, expected_floor: Option<u64>) -> Result<FloorPreview, LicenceError> {
        self.run(|inner, ev| {
            let b = self.binding_in_effect(inner, ev)?;
            let now = (self.clock)();
            let p = preview_of(inner, &b.grant_pubkey, &b.mesh_id, now);
            if expected_floor.is_some_and(|f| f != p.floor) {
                return Err(LicenceError::CheckFailed("floor_changed".into()));
            }
            let mut next = inner.clone();
            next.floors.entry(b.grant_pubkey).or_default().reset(now);
            ev.push(super::LicenceEvent::FloorReset(now));
            self.commit(inner, next)?;
            Ok(p)
        })
    }
}

fn preview_of(inner: &super::store::Inner, key: &str, mesh_id: &str, now: u64) -> FloorPreview {
    let cur = inner.floors.get(key).copied().unwrap_or_default();
    let mut after = cur;
    after.reset(now);
    let (eff_now, eff_after) = (cur.effective_now(now), after.effective_now(now));
    let mut revived: Vec<RevivedGrant> = inner
        .slots
        .values()
        .filter_map(|s| s.current.as_ref())
        .filter(|h| {
            h.body.mesh_id == mesh_id && !grant_valid(&h.body, eff_now) && grant_valid(&h.body, eff_after)
        })
        .map(|h| RevivedGrant {
            cog_id: h.body.cog_id.clone(),
            version: h.body.version.clone(),
            expires_at: h.body.expires_at,
        })
        .collect();
    revived.sort_by(|a, c| (&a.cog_id, &a.version).cmp(&(&c.cog_id, &c.version)));
    FloorPreview { now, floor: cur.floor(), floor_after: after.floor(), revived }
}
