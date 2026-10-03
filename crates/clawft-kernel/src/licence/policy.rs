//! [`MeshCheckoutPolicy`]: the swarm redistribution policy of a Seed-bound
//! mesh (ADR-106 section 5).
//!
//! It wraps [`ManifestPolicy`]: if that allows, so does this. Otherwise it
//! allows a Cognitum-origin hash only for a verified [`Audience::Serve`] peer
//! or [`Audience::Seed`], and only while a valid grant for this mesh covers
//! the hash with the same cog and version. [`Audience::Advertise`] is always
//! denied. It reads the binding and grants live on every call, so a binding
//! that arrives later needs no restart, and with no binding in effect it is
//! exactly [`ManifestPolicy`].

use std::sync::Arc;

use super::{CheckoutGrantStore, VerifiedCheckoutGrant};
use crate::mesh_artifact::ArtifactExchange;
use crate::mesh_swarm_state::{
    Audience, GrantInfo, GrantOrigin, ManifestPolicy, RedistributionPolicy,
};
use crate::workload_pkg::codec::hex_encode;

/// Package id prefix of grants made by [`ArtifactExchange::grant_checkout`].
pub const CHECKOUT_PACKAGE_PREFIX: &str = "checkout:";

/// The checkout-aware redistribution policy.
#[derive(Debug)]
pub struct MeshCheckoutPolicy {
    base: ManifestPolicy,
    store: Arc<CheckoutGrantStore>,
}

impl MeshCheckoutPolicy {
    /// A policy reading `store` (which holds the live local mesh id).
    pub fn new(store: Arc<CheckoutGrantStore>) -> Self {
        Self { base: ManifestPolicy, store }
    }

    /// The policy the daemon installs unconditionally: a store under
    /// `dir/licence` (poisoned, not fatal, if its file is bad), the
    /// revocation list attached, and the local mesh id taken from `local`
    /// (unset until the mesh nonce is configured, which makes the policy
    /// behave exactly like `ManifestPolicy`).
    pub fn open(
        dir: &std::path::Path,
        anchors: crate::workload_pkg::TrustAnchors,
        revocations: Arc<crate::revocation::RevocationList>,
        local: super::LocalMeshId,
    ) -> Arc<Self> {
        let store = CheckoutGrantStore::open_or_poisoned(
            &dir.join("licence"),
            Arc::new(anchors),
            local,
            super::system_clock(),
        );
        if let Some(why) = store.poisoned() {
            tracing::warn!(%why, "licence store unreadable; checkout policy stays off");
        }
        store.attach_revocations(revocations);
        Arc::new(Self::new(Arc::new(store)))
    }

    /// The store the policy reads.
    pub fn store(&self) -> &Arc<CheckoutGrantStore> {
        &self.store
    }
}

impl RedistributionPolicy for MeshCheckoutPolicy {
    fn allows(&self, hash: &[u8; 32], grants: &[GrantInfo], audience: &Audience<'_>) -> bool {
        if self.base.allows(hash, grants, audience) {
            return true;
        }
        match audience {
            Audience::Advertise => return false,
            Audience::Serve(peer) if !peer.verified => return false,
            _ => {}
        }
        if grants.is_empty() {
            return false;
        }
        let hex = hex_encode(hash);
        grants.iter().filter(|g| !g.is_opt_in()).all(|g| match &g.origin {
            GrantOrigin::Cognitum { cog_id, version } => {
                self.store.valid_grant_covering(&hex, cog_id, version).is_some()
            }
            _ => false,
        })
    }
}

impl ArtifactExchange {
    /// Make the bytes of a verified checkout grant *shareable* inside the
    /// mesh: one grant per listed BLAKE3, signed by the bound grant key, with
    /// `GrantOrigin::Cognitum`. A revoked key or hash is skipped by
    /// `grant_with`, and revocation applies through the usual sweep. This
    /// does not make the bytes runnable (see [`super::may_run`]).
    pub fn grant_checkout(&self, v: &VerifiedCheckoutGrant) {
        let g = v.grant();
        let package = format!("{CHECKOUT_PACKAGE_PREFIX}{}@{}", g.cog_id, g.version);
        for a in &g.artifacts {
            let Some(hash) = crate::workload_pkg::codec::hex_decode_exact::<32>(&a.blake3) else {
                continue;
            };
            self.grant_with(
                hash,
                &package,
                vec![v.grant_pubkey_hex().to_owned()],
                GrantOrigin::Cognitum { cog_id: g.cog_id.clone(), version: g.version.clone() },
            );
        }
    }
}
