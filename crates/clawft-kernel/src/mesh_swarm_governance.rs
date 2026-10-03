//! Governance of what a node seeds, holds and accepts from peers (ADR-099
//! section 6, card mesh-placement-25).
//!
//! - **Seeding.** An artifact is seeded once it is verified here and a
//!   verified signed manifest lists it ([`ArtifactExchange::is_servable`]);
//!   the first time that holds, `artifact.seed` is chained.
//! - **Revocation.** A revoked package id, signer key or content hash
//!   stops seeding at once (the live check in `is_servable`);
//!   [`ArtifactExchange::apply_revocations`] also drops the grant, evicts
//!   the bytes and chains `artifact.revoke` and `artifact.evict`.
//! - **Eviction.** [`ArtifactExchange::forget`] removes an artifact's
//!   registration and the blobs no other artifact lists.
//! - **Bans.** A peer that serves a corrupt piece is banned (after
//!   `ban_after_corrupt` of them): never fetched from, never served, and
//!   the ban is chained (`artifact.peer_ban`).

use std::collections::HashSet;
use std::sync::Arc;

use clawft_types::placement::{Capability, CapabilityId, NodeFacts, Provenance};

use crate::chain::{
    EVENT_KIND_ARTIFACT_EVICT, EVENT_KIND_ARTIFACT_PEER_BAN, EVENT_KIND_ARTIFACT_REVOKE,
    EVENT_KIND_ARTIFACT_SEED,
};
use crate::mesh_artifact::ArtifactExchange;
use crate::mesh_artifact_types::{ArtifactDescriptor, ArtifactId};
use crate::mesh_swarm_picker::LinkStats;
use crate::mesh_swarm_state::GrantInfo;
use crate::revocation::{RevocationList, RevokedSubject};
use crate::workload_pkg::codec::hex_encode;

/// Held artifacts advertised at most this many to keep facts small.
pub const MAX_ADVERTISED_ARTIFACTS: usize = 128;

/// What [`ArtifactExchange::forget`] removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forgotten {
    /// The artifact.
    pub id: ArtifactId,
    /// Its whole-content hash.
    pub content_hash: [u8; 32],
    /// Bytes of blobs freed.
    pub bytes_freed: u64,
}

/// One artifact a revocation took away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokedArtifact {
    /// Whole-content hash.
    pub content_hash: [u8; 32],
    /// Package that allowed it.
    pub package_id: String,
    /// The revoked subject that matched.
    pub subject: RevokedSubject,
    /// Bytes freed by evicting it (0 if it was only pending or absent).
    pub bytes_freed: u64,
}

impl ArtifactExchange {
    // ── revocation ──────────────────────────────────────────────

    /// Use `list` as this node's revocation list (first call wins).
    pub fn set_revocations(&self, list: Arc<RevocationList>) -> bool {
        self.swarm.revocations.set(list).is_ok()
    }

    pub(crate) fn is_revoked_subject(
        &self,
        package_id: &str,
        signers: &[String],
        content_hash: &[u8; 32],
    ) -> bool {
        let Some(list) = self.swarm.revocations.get() else {
            return false;
        };
        let hex = hex_encode(content_hash);
        list.first_revoked(Some(package_id), signers.iter(), std::iter::once(&hex))
            .is_some()
    }

    /// True when the content hash is revoked, or every package that allowed
    /// it (or those packages' signers) is. A package revoked while another,
    /// unrevoked one still lists the same content leaves the content allowed.
    pub fn is_revoked(&self, content_hash: &[u8; 32]) -> bool {
        match self.grants.get(content_hash) {
            Some(gs) if !gs.is_empty() => gs
                .iter()
                .all(|g| self.is_revoked_subject(&g.package_id, &g.signers, content_hash)),
            _ => self.is_revoked_subject("", &[], content_hash),
        }
    }

    /// The first grant that currently allows seeding `content_hash`: not
    /// revoked, and its package may be redistributed.
    pub(crate) fn servable_grant(&self, content_hash: &[u8; 32]) -> Option<GrantInfo> {
        self.grants.get(content_hash)?.iter().find(|g| {
            g.redistributable
                && !self.is_revoked_subject(&g.package_id, &g.signers, content_hash)
        }).cloned()
    }

    /// Apply the revocation list to everything held: drop each revoked
    /// grant; when no grant is left for a content hash, evict its bytes (and
    /// any partial copy), and chain `artifact.revoke` and `artifact.evict`.
    /// Safe to call repeatedly.
    ///
    /// Revocation evicts **even pinned** cache entries: a revoked package must
    /// stop running as well as stop seeding.
    ///
    /// Revocation is monotonic mesh-wide: entries are only ever added, by an
    /// operator command or a signed notice, and a notice is re-flooded to
    /// every peer. Removing one locally (`unrevoke_subject`) is undone by the
    /// next notice a peer re-sends; to lift a revocation, clear the list on
    /// every node or issue the package under a new key.
    pub fn apply_revocations(&self) -> Vec<RevokedArtifact> {
        let Some(list) = self.swarm.revocations.get().cloned() else {
            return Vec::new();
        };
        // (content, package, subject) for every grant a revocation took away.
        let mut hits: Vec<([u8; 32], String, RevokedSubject)> = Vec::new();
        let mut emptied: Vec<[u8; 32]> = Vec::new();
        for entry in self.grants.iter() {
            let hex = hex_encode(entry.key());
            let mut left = 0;
            for g in entry.value() {
                match list.first_revoked(
                    Some(&g.package_id),
                    g.signers.iter(),
                    std::iter::once(&hex),
                ) {
                    Some(subject) => hits.push((*entry.key(), g.package_id.clone(), subject)),
                    None => left += 1,
                }
            }
            if left == 0 {
                emptied.push(*entry.key());
            }
        }
        // Revoked content hashes also cover what was never granted (a partial
        // or unlisted copy).
        for s in list.list_subjects(Some(crate::revocation::RevocationKind::ArtifactHash)) {
            let Some(hash) = crate::workload_pkg::codec::hex_decode_exact::<32>(&s.id) else {
                continue;
            };
            let held = self.by_content.contains_key(&hash)
                || self.pending.iter().any(|p| p.content_hash == hash);
            if held && !hits.iter().any(|h| h.0 == hash) {
                hits.push((hash, String::new(), s));
                emptied.push(hash);
            }
        }
        let mut out = Vec::new();
        for (content_hash, package_id, subject) in hits {
            let evict = emptied.contains(&content_hash);
            if let Some(mut gs) = self.grants.get_mut(&content_hash) {
                gs.retain(|g| g.package_id != package_id);
            }
            self.grants.remove_if(&content_hash, |_, gs| gs.is_empty());
            let mut ids: Vec<ArtifactId> = Vec::new();
            let mut bytes_freed = 0;
            if evict {
                self.swarm.seeded.remove(&content_hash);
                ids = self
                    .by_content
                    .get(&content_hash)
                    .map(|i| *i)
                    .into_iter()
                    .chain(
                        self.pending
                            .iter()
                            .filter(|p| p.content_hash == content_hash)
                            .map(|p| p.id()),
                    )
                    .collect();
                for id in &ids {
                    if let Some(f) = self.forget(id, "revoked") {
                        bytes_freed += f.bytes_freed;
                    }
                }
            }
            self.chain_event(
                EVENT_KIND_ARTIFACT_REVOKE,
                serde_json::json!({
                    "subject_kind": subject.kind.to_string(),
                    "subject_id": subject.id,
                    "reason": subject.reason,
                    "content_hash": hex_encode(&content_hash),
                    "package_id": package_id,
                    "artifact_ids": ids.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
                    "evicted": evict,
                    "bytes_freed": bytes_freed,
                    "node": self.node_id(),
                }),
            );
            out.push(RevokedArtifact {
                content_hash,
                package_id,
                subject,
                bytes_freed,
            });
        }
        out
    }

    // ── bans ────────────────────────────────────────────────────

    /// Ban `peer`: no more fetching from it or serving it. Returns true if
    /// newly banned (chained as `artifact.peer_ban`).
    pub fn ban_peer(&self, peer: &str, reason: &str) -> bool {
        if self.swarm.bans.insert(peer.to_string(), reason.to_string()).is_some() {
            return false;
        }
        self.chain_event(
            EVENT_KIND_ARTIFACT_PEER_BAN,
            serde_json::json!({ "peer": peer, "reason": reason, "node": self.node_id() }),
        );
        true
    }

    /// Lift a ban. Returns true if the peer was banned.
    pub fn unban_peer(&self, peer: &str) -> bool {
        self.swarm.corrupt.remove(peer);
        self.swarm.bans.remove(peer).is_some()
    }

    /// True if `peer` is banned.
    pub fn is_banned(&self, peer: &str) -> bool {
        self.swarm.bans.contains_key(peer)
    }

    /// Banned peers and why.
    pub fn banned_peers(&self) -> Vec<(String, String)> {
        let mut v: Vec<_> = self
            .swarm
            .bans
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();
        v.sort();
        v
    }

    /// A peer served piece `index` of `id` with a bad hash: chain it, count
    /// it, and ban the peer once it reaches `ban_after_corrupt`. Returns
    /// true if the peer is now banned.
    pub(crate) fn report_corrupt_piece(&self, id: &ArtifactId, index: u32, peer: &str) -> bool {
        self.chain_piece_rejected(id, index, peer, "hash mismatch");
        let n = {
            let mut e = self.swarm.corrupt.entry(peer.to_string()).or_insert(0);
            *e += 1;
            *e
        };
        if n >= self.config().ban_after_corrupt {
            self.ban_peer(peer, &format!("served a corrupt piece of {id}"));
        }
        self.is_banned(peer)
    }

    // ── seeding / eviction ──────────────────────────────────────

    /// Measured link speeds to peers.
    pub fn link_stats(&self) -> &LinkStats {
        &self.swarm.links
    }

    pub(crate) fn on_granted(&self, content_hash: &[u8; 32]) {
        self.maybe_seed(content_hash);
    }

    pub(crate) fn on_verified(&self, id: &ArtifactId, content_hash: &[u8; 32]) {
        self.maybe_seed(content_hash);
        if let Some(cache) = self.swarm.cache() {
            cache.on_verified(id);
        }
    }

    /// Chain `artifact.seed` the first time an artifact is both verified
    /// here and allowed by a verified manifest.
    fn maybe_seed(&self, content_hash: &[u8; 32]) {
        let Some(id) = self.by_content.get(content_hash).map(|i| *i) else {
            return;
        };
        let Some(g) = self.servable_grant(content_hash) else {
            return;
        };
        if self.is_revoked(content_hash) || self.swarm.seeded.insert(*content_hash, ()).is_some() {
            return;
        }
        let size = self.descriptor(&id).map_or(0, |d| d.total_size);
        self.chain_event(
            EVENT_KIND_ARTIFACT_SEED,
            serde_json::json!({
                "artifact_id": id.to_string(),
                "content_hash": hex_encode(content_hash),
                "package_id": g.package_id,
                "size": size,
                "node": self.node_id(),
            }),
        );
    }

    /// Bytes of downloads in progress (what their descriptors claim).
    pub fn pending_bytes(&self) -> u64 {
        self.pending.iter().map(|d| d.total_size).sum()
    }

    /// Verified artifacts this node may serve (verified, allowed, not revoked).
    pub fn servable_artifacts(&self) -> Vec<ArtifactDescriptor> {
        self.descriptors
            .iter()
            .map(|d| d.clone())
            .filter(|d| self.is_servable(d))
            .collect()
    }

    /// Remove `id` (verified or pending) and the blobs no other artifact
    /// lists. The grant is kept, so a later fetch can seed it again;
    /// revocation drops the grant itself. Chains `artifact.evict`.
    ///
    /// Only blobs this exchange created and nobody else has stored since
    /// (store reference count 1) are removed: the store is shared with
    /// package verification and installed workloads, which may hold the same
    /// bytes. A pinned cache entry is not forgotten, except for `revoked`.
    pub fn forget(&self, id: &ArtifactId, reason: &str) -> Option<Forgotten> {
        if reason != "revoked" && self.swarm.cache().is_some_and(|c| c.is_pinned(id)) {
            return None;
        }
        let verified = self.descriptors.remove(id).map(|(_, d)| d);
        let pending = self.pending.remove(id).map(|(_, d)| d);
        let d = verified.or(pending)?;
        self.by_content.remove_if(&d.content_hash, |_, v| v == id);
        self.swarm.seeded.remove(&d.content_hash);
        self.peer_haves.retain(|k, _| k.0 != *id);
        let mut keep: HashSet<[u8; 32]> = HashSet::new();
        for v in self.descriptors.iter() {
            keep.insert(v.content_hash);
            keep.extend(v.pieces.iter().copied());
        }
        for p in self.pending.iter() {
            keep.insert(p.content_hash);
            keep.extend(p.pieces.iter().copied());
        }
        let mut freed = 0u64;
        let candidates = d.pieces.iter().copied().chain(std::iter::once(d.content_hash));
        let mut seen = HashSet::new();
        for hash in candidates {
            if keep.contains(&hash) || !seen.insert(hash) {
                continue;
            }
            if !self.swarm.owned.contains_key(&hash) {
                continue; // not ours: another subsystem put it there
            }
            let key = hex_encode(&hash);
            if let Some((_, size, _, _, refs)) = self.store.metadata(&key) {
                if refs > 1 {
                    // Someone else stored the same bytes since: theirs now.
                    self.swarm.owned.remove(&hash);
                } else if self.store.remove(&key).is_ok() {
                    self.swarm.owned.remove(&hash);
                    freed += size;
                }
            }
        }
        if let Some(cache) = self.swarm.cache() {
            cache.on_forgotten(id);
        }
        self.chain_event(
            EVENT_KIND_ARTIFACT_EVICT,
            serde_json::json!({
                "artifact_id": id.to_string(),
                "content_hash": d.content_hex(),
                "size": d.total_size,
                "bytes_freed": freed,
                "reason": reason,
                "node": self.node_id(),
            }),
        );
        Some(Forgotten {
            id: *id,
            content_hash: d.content_hash,
            bytes_freed: freed,
        })
    }

    pub(crate) fn chain_event(&self, kind: &str, payload: serde_json::Value) {
        if let Some(cm) = &self.chain {
            cm.append("mesh_artifact", kind, Some(payload));
        }
    }

    // ── advertisement ───────────────────────────────────────────

    /// Capabilities for what this node can serve, to merge into its facts:
    /// `store.artifact.<16 hex of content hash>` per artifact (attrs
    /// `content`, `size`) and, for content hashes in `model_shards`,
    /// `model.present` with the shard list. Held artifacts are `probed`: this
    /// node verified them itself.
    pub fn held_capabilities(&self, model_shards: &HashSet<[u8; 32]>) -> Vec<Capability> {
        let mut held = self.servable_artifacts();
        held.sort_by_key(|d| d.content_hash);
        held.truncate(MAX_ADVERTISED_ARTIFACTS);
        let mut caps = Vec::new();
        let mut shards = Vec::new();
        for d in &held {
            let hex = d.content_hex();
            if let Ok(id) = CapabilityId::new(format!("store.artifact.{}", &hex[..16])) {
                let size = i64::try_from(d.total_size).unwrap_or(i64::MAX);
                caps.push(
                    Capability::new(id, Provenance::Probed)
                        .with_attr("content", hex.as_str())
                        .with_attr("size", size),
                );
            }
            if model_shards.contains(&d.content_hash) {
                shards.push(hex);
            }
        }
        if !shards.is_empty()
            && let Ok(id) = CapabilityId::new("model.present")
        {
            let list = crate::node_facts::probe::str_list(&shards);
            caps.push(Capability::new(id, Provenance::Probed).with_attr("shards", list));
        }
        caps
    }
}

/// Replace the swarm capabilities in `facts` (`store.artifact.*`,
/// `model.present`) with `held`.
pub fn set_held_capabilities(facts: &mut NodeFacts, held: Vec<Capability>) {
    facts
        .capabilities
        .retain(|c| !c.id.is_under("store.artifact") && c.id.as_str() != "model.present");
    facts.capabilities.extend(held);
}
