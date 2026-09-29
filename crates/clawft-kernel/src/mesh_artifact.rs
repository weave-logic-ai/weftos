//! Artifact exchange over the mesh (ADR-099 section 6, mesh-placement-11).
//!
//! [`ArtifactExchange`] is the node-local side of the swarm-ready piece
//! protocol defined in [`crate::mesh_artifact_wire`]. It keeps
//! descriptors, derives each artifact's `have` bitfield from the pieces
//! held in [`ArtifactStore`], and decides what may be served.
//!
//! - **Identity.** An artifact is its piece-list root
//!   ([`ArtifactDescriptor::id`]). Pieces are stored in `ArtifactStore`
//!   under their own BLAKE3, so any node holding verified pieces can serve
//!   them, and a restart resumes from the pieces already on disk.
//! - **Governance.** Only content listed by a signed manifest that
//!   verified on this node is served ([`Self::is_servable`]); see
//!   [`crate::mesh_artifact_pkg`].
//! - **Audit.** Outcomes are chained (`artifact.fetch`,
//!   `artifact.piece_rejected`, `artifact.serve` once per peer);
//!   individual pieces are not.
//!
//! Transfer (serve / fetch) lives in [`crate::mesh_artifact_transfer`].
//! v1 fetches from one peer at a time; card 25 adds multi-source fetch,
//! rarest-first, seeding policy, caching and bandwidth limits on top of
//! the same types.

use std::io::Read;
use std::sync::Arc;

use dashmap::DashMap;

use crate::artifact_store::{ArtifactStore, ArtifactType};
use crate::chain::ChainManager;
pub use crate::mesh_artifact_types::{ExchangeConfig, ExchangeError};
use crate::mesh_artifact_wire::{ArtifactDescriptor, ArtifactId, ArtifactKey, Bitfield};
use crate::workload_pkg::codec::hex_encode;

/// Node-local state of the artifact piece protocol.
pub struct ArtifactExchange {
    node_id: String,
    store: Arc<ArtifactStore>,
    config: ExchangeConfig,
    descriptors: DashMap<ArtifactId, ArtifactDescriptor>,
    by_content: DashMap<[u8; 32], ArtifactId>,
    /// Content hash -> package id of the signed manifest that allows it.
    grants: DashMap<[u8; 32], String>,
    /// `(artifact, peer)` pairs already chained as `artifact.serve`.
    served: DashMap<(ArtifactId, String), ()>,
    /// Last `have` each peer announced (input for card 25's scheduler).
    peer_haves: DashMap<(ArtifactId, String), Bitfield>,
    chain: Option<Arc<ChainManager>>,
}

impl ArtifactExchange {
    /// Exchange for `node_id` over `store`.
    pub fn new(
        node_id: impl Into<String>,
        store: Arc<ArtifactStore>,
        config: ExchangeConfig,
    ) -> Result<Self, ExchangeError> {
        config.validate()?;
        Ok(Self {
            node_id: node_id.into(),
            store,
            config,
            descriptors: DashMap::new(),
            by_content: DashMap::new(),
            grants: DashMap::new(),
            served: DashMap::new(),
            peer_haves: DashMap::new(),
            chain: None,
        })
    }

    /// Chain transfer outcomes to `cm`.
    pub fn set_chain_manager(&mut self, cm: Arc<ChainManager>) {
        self.chain = Some(cm);
    }

    /// This node's id.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Backing store.
    pub fn store(&self) -> &Arc<ArtifactStore> {
        &self.store
    }

    /// Configuration.
    pub fn config(&self) -> &ExchangeConfig {
        &self.config
    }

    /// Split `reader` into pieces, store each, and register the artifact.
    /// Streams: at most one piece is in memory. When `expect` is given
    /// (`content hash, size`), a mismatch is an error and nothing is
    /// registered.
    pub fn seed_reader(
        &self,
        reader: &mut dyn Read,
        expect: Option<([u8; 32], u64)>,
    ) -> Result<ArtifactDescriptor, ExchangeError> {
        let piece_size = self.config.piece_size;
        let mut whole = blake3::Hasher::new();
        let mut pieces = Vec::new();
        let mut total = 0u64;
        let mut buf = vec![0u8; piece_size as usize];
        loop {
            let n = read_full(reader, &mut buf)?;
            if n == 0 {
                break;
            }
            let piece = &buf[..n];
            whole.update(piece);
            let hash = *blake3::hash(piece).as_bytes();
            self.store_piece(piece, &hash)?;
            pieces.push(hash);
            total += n as u64;
            if n < buf.len() {
                break;
            }
        }
        let descriptor = ArtifactDescriptor {
            piece_size,
            total_size: total,
            content_hash: *whole.finalize().as_bytes(),
            pieces,
        };
        if let Some((hash, size)) = expect
            && (hash != descriptor.content_hash || size != total)
        {
            return Err(ExchangeError::Mismatch(format!(
                "expected {} ({size} bytes), read {} ({total} bytes)",
                hex_encode(&hash),
                descriptor.content_hex()
            )));
        }
        self.register_descriptor(descriptor.clone())?;
        Ok(descriptor)
    }

    /// [`Self::seed_reader`] over an in-memory buffer.
    pub fn seed_bytes(&self, content: &[u8]) -> Result<ArtifactDescriptor, ExchangeError> {
        self.seed_reader(&mut &content[..], None)
    }

    /// Remember a validated descriptor (pieces may still be missing).
    pub fn register_descriptor(&self, d: ArtifactDescriptor) -> Result<ArtifactId, ExchangeError> {
        d.validate()?;
        let id = d.id();
        self.by_content.insert(d.content_hash, id);
        self.descriptors.insert(id, d);
        Ok(id)
    }

    /// Descriptor by id.
    pub fn descriptor(&self, id: &ArtifactId) -> Option<ArtifactDescriptor> {
        self.descriptors.get(id).map(|d| d.clone())
    }

    /// Descriptor by key.
    pub fn resolve(&self, key: &ArtifactKey) -> Option<ArtifactDescriptor> {
        let id = match key {
            ArtifactKey::Root(id) => *id,
            ArtifactKey::Content(h) => *self.by_content.get(h)?,
        };
        self.descriptor(&id)
    }

    /// Verified pieces held locally, derived from the store.
    pub fn have(&self, id: &ArtifactId) -> Option<Bitfield> {
        let d = self.descriptors.get(id)?;
        let mut b = Bitfield::new(d.piece_count());
        for (i, p) in d.pieces.iter().enumerate() {
            b.set(i as u32, self.store.contains(&hex_encode(p)));
        }
        Some(b)
    }

    /// Stream the whole artifact through `sink` in piece order, checking
    /// every piece and the whole-content hash. Returns the byte count.
    pub fn read_to(
        &self,
        id: &ArtifactId,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), ExchangeError>,
    ) -> Result<u64, ExchangeError> {
        let d = self
            .descriptor(id)
            .ok_or_else(|| ExchangeError::Unknown(id.to_string()))?;
        let mut whole = blake3::Hasher::new();
        let mut total = 0u64;
        for i in 0..d.piece_count() {
            let data = self.load_piece(&d, i)?;
            whole.update(&data);
            total += data.len() as u64;
            sink(&data)?;
        }
        if *whole.finalize().as_bytes() != d.content_hash || total != d.total_size {
            return Err(ExchangeError::Mismatch(format!(
                "pieces of {id} do not assemble to content {}",
                d.content_hex()
            )));
        }
        Ok(total)
    }

    /// Whole artifact in memory (bounded by `materialize_limit`).
    pub fn read_all(&self, id: &ArtifactId) -> Result<Vec<u8>, ExchangeError> {
        let d = self
            .descriptor(id)
            .ok_or_else(|| ExchangeError::Unknown(id.to_string()))?;
        if d.total_size > self.config.materialize_limit {
            return Err(ExchangeError::Config(format!(
                "artifact {id} is larger than materialize_limit"
            )));
        }
        let mut out = Vec::with_capacity(d.total_size as usize);
        self.read_to(id, &mut |b| {
            out.extend_from_slice(b);
            Ok(())
        })?;
        Ok(out)
    }

    /// Allow serving `content_hash` under a verified package.
    pub(crate) fn grant(&self, content_hash: [u8; 32], package_id: &str) {
        self.grants.insert(content_hash, package_id.to_string());
    }

    /// Governance: served only when a verified signed manifest lists it.
    pub fn is_servable(&self, d: &ArtifactDescriptor) -> bool {
        self.grants.contains_key(&d.content_hash)
    }

    /// Record the `have` a peer announced.
    pub(crate) fn note_peer_have(&self, id: ArtifactId, peer: &str, have: Bitfield) {
        self.peer_haves.insert((id, peer.to_string()), have);
    }

    /// Last `have` announced by `peer` for `id`.
    pub fn peer_have(&self, id: &ArtifactId, peer: &str) -> Option<Bitfield> {
        self.peer_haves
            .get(&(*id, peer.to_string()))
            .map(|b| b.clone())
    }

    /// Load piece `i` of `d` (the store re-checks its hash).
    pub(crate) fn load_piece(
        &self,
        d: &ArtifactDescriptor,
        i: u32,
    ) -> Result<Vec<u8>, ExchangeError> {
        let hash = d
            .pieces
            .get(i as usize)
            .ok_or_else(|| ExchangeError::Unknown(format!("piece {i}")))?;
        self.store
            .load(&hex_encode(hash))
            .map_err(|e| ExchangeError::Store(e.to_string()))
    }

    /// Store a piece whose hash the caller has already checked.
    pub(crate) fn store_piece(&self, data: &[u8], hash: &[u8; 32]) -> Result<(), ExchangeError> {
        let key = hex_encode(hash);
        if self.store.contains(&key) {
            return Ok(());
        }
        let stored = self
            .store
            .store(data, ArtifactType::Generic)
            .map_err(|e| ExchangeError::Store(e.to_string()))?;
        if stored != key {
            return Err(ExchangeError::Mismatch(format!(
                "piece stored as {stored}, expected {key}"
            )));
        }
        Ok(())
    }

    /// After a verified fetch, keep small artifacts whole as well.
    pub(crate) fn materialize(&self, id: &ArtifactId) -> Result<(), ExchangeError> {
        let Some(d) = self.descriptor(id) else {
            return Ok(());
        };
        if d.total_size > self.config.materialize_limit || self.store.contains(&d.content_hex()) {
            return Ok(());
        }
        let bytes = self.read_all(id)?;
        self.store
            .store(&bytes, ArtifactType::Generic)
            .map_err(|e| ExchangeError::Store(e.to_string()))?;
        Ok(())
    }

    // ── chain ────────────────────────────────────────────────────

    pub(crate) fn chain_fetch(&self, payload: serde_json::Value) {
        if let Some(cm) = &self.chain {
            cm.append(
                "mesh_artifact",
                crate::chain::EVENT_KIND_ARTIFACT_FETCH,
                Some(payload),
            );
        }
    }

    pub(crate) fn chain_piece_rejected(&self, id: &ArtifactId, index: u32, peer: &str, why: &str) {
        if let Some(cm) = &self.chain {
            cm.append(
                "mesh_artifact",
                crate::chain::EVENT_KIND_ARTIFACT_PIECE_REJECTED,
                Some(serde_json::json!({
                    "artifact_id": id.to_string(),
                    "piece_index": index,
                    "peer": peer,
                    "reason": why,
                    "node": self.node_id,
                })),
            );
        }
    }

    /// Chain `artifact.serve` the first time `peer` is served `d`.
    pub(crate) fn chain_serve_once(&self, d: &ArtifactDescriptor, peer: &str) {
        let key = (d.id(), peer.to_string());
        if self.served.insert(key, ()).is_some() {
            return;
        }
        if let Some(cm) = &self.chain {
            cm.append(
                "mesh_artifact",
                crate::chain::EVENT_KIND_ARTIFACT_SERVE,
                Some(serde_json::json!({
                    "artifact_id": d.id().to_string(),
                    "content_hash": d.content_hex(),
                    "peer": peer,
                    "node": self.node_id,
                })),
            );
        }
    }
}

fn read_full(r: &mut dyn Read, buf: &mut [u8]) -> Result<usize, ExchangeError> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(ExchangeError::Io(e.to_string())),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_artifact_wire::MAX_ARTIFACT_FRAME;

    fn exchange(piece: u64) -> ArtifactExchange {
        let cfg = ExchangeConfig {
            piece_size: piece,
            ..Default::default()
        };
        ArtifactExchange::new("n1", Arc::new(ArtifactStore::new_memory()), cfg).unwrap()
    }

    #[test]
    fn seeding_splits_into_pieces_and_roundtrips() {
        let x = exchange(1024);
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let d = x.seed_bytes(&data).unwrap();
        assert_eq!(d.piece_count(), 5);
        assert_eq!(d.piece_len(4), 5000 - 4096);
        assert_eq!(d.content_hash, *blake3::hash(&data).as_bytes());
        assert!(x.have(&d.id()).unwrap().is_complete());
        assert_eq!(x.read_all(&d.id()).unwrap(), data);
        assert_eq!(
            x.resolve(&ArtifactKey::Content(d.content_hash))
                .unwrap()
                .id(),
            d.id()
        );
    }

    #[test]
    fn have_follows_the_store() {
        let x = exchange(1024);
        let d = x.seed_bytes(&[3u8; 3000]).unwrap();
        // Pieces 0 and 1 are identical content (one store entry); drop the
        // distinct last piece.
        x.store().remove(&hex_encode(&d.pieces[2])).unwrap();
        let have = x.have(&d.id()).unwrap();
        assert_eq!(have.missing().collect::<Vec<_>>(), vec![2]);
        assert!(x.read_all(&d.id()).is_err());
    }

    #[test]
    fn seeding_with_wrong_expectation_registers_nothing() {
        let x = exchange(1024);
        let err = x.seed_reader(&mut &b"abc"[..], Some(([0; 32], 3)));
        assert!(matches!(err, Err(ExchangeError::Mismatch(_))));
        assert!(
            x.resolve(&ArtifactKey::Content(*blake3::hash(b"abc").as_bytes()))
                .is_none()
        );
    }

    #[test]
    fn nothing_is_servable_without_a_grant() {
        let x = exchange(1024);
        let d = x.seed_bytes(b"payload").unwrap();
        assert!(!x.is_servable(&d));
        x.grant(d.content_hash, "pkg");
        assert!(x.is_servable(&d));
    }

    #[test]
    fn config_limits_are_enforced() {
        let store = Arc::new(ArtifactStore::new_memory());
        let bad_block = ExchangeConfig {
            block_size: MAX_ARTIFACT_FRAME,
            ..Default::default()
        };
        assert!(ArtifactExchange::new("n", store.clone(), bad_block).is_err());
        let bad_piece = ExchangeConfig {
            piece_size: 10,
            ..Default::default()
        };
        assert!(ArtifactExchange::new("n", store, bad_piece).is_err());
    }
}
