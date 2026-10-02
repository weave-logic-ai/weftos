//! Signed machine facts (`facts.json`, plan 1.1, 1.4).
//!
//! The document holds the node facts (probe results signed by the box key
//! with the kernel's `sign_node_facts`) and a signed revocation statement: the
//! revoked certificate serials, and the `(user_id, through)` ranges that also
//! cover users a journal quarantine proved revoked whose serials were lost
//! with the tail. A `mesh.revocations` note in the signed facts carries the
//! statement's hash, tying the two together.
//!
//! Facts are never published from degraded bindings (a partial revocation
//! list must not look complete); the previous document stays until the
//! journal is repaired.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clawft_kernel::node_facts::{build_facts, probe_capabilities, Collected, ProbeConfig, SystemHost};
use clawft_kernel::node_facts_advert::{sign_node_facts, NodeFactsAdvertError};
use clawft_mesh_local::hexser;
use clawft_types::placement::node_facts::ProbeNote;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::MeshServiceConfig;
use crate::force_revoked::ForceRevoked;
use crate::state::Core;

/// Domain separator of the revocation statement signature.
pub const REVOCATION_DOMAIN: &[u8] = b"weftos/mesh-revocations/v1\0";
/// File name inside the state directory.
pub const FACTS_FILE: &str = "facts.json";

#[derive(Debug, thiserror::Error)]
pub enum FactsError {
    #[error("bindings are degraded; not publishing partial revocations: {0}")]
    Degraded(String),
    #[error("facts: {0}")]
    Sign(#[from] NodeFactsAdvertError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

struct Inner {
    seq: u64,
    probed: Collected,
    last_revocations_hash: Option<String>,
    current: Option<Value>,
}

pub struct Facts {
    path: PathBuf,
    key: SigningKey,
    node_id: String,
    ttl_s: u64,
    probe: bool,
    inner: Mutex<Inner>,
}

impl Facts {
    pub fn new(cfg: &MeshServiceConfig, key: SigningKey, node_id: String) -> Arc<Self> {
        Arc::new(Self {
            path: cfg.state_dir.join(FACTS_FILE),
            key,
            node_id,
            ttl_s: cfg.facts_ttl_s,
            probe: cfg.probe_facts,
            inner: Mutex::new(Inner {
                seq: 0,
                probed: Collected::default(),
                last_revocations_hash: None,
                current: None,
            }),
        })
    }

    /// Probe the host (blocking: runs external tools with timeouts). Call from
    /// `spawn_blocking`. A no-op when probing is disabled.
    pub fn reprobe(&self) {
        if !self.probe {
            return;
        }
        let cfg = ProbeConfig { docker_probe_image: None, ..ProbeConfig::default() };
        let collected = probe_capabilities(&SystemHost::default(), &cfg);
        self.inner.lock().expect("facts lock").probed = collected;
    }

    /// The current facts document, if one was published.
    pub fn current(&self) -> Option<Value> {
        self.inner.lock().expect("facts lock").current.clone()
    }

    /// Sign and write fresh facts from the cached probe and the current
    /// revocations; journal `facts.sign` when the revocations changed.
    pub fn refresh(&self, core: &Mutex<Core>, force: &ForceRevoked, now: u64) -> Result<(), FactsError> {
        let (serials, ranges) = {
            let c = core.lock().expect("core lock");
            let mut serials = c.bindings.revoked_serials().map_err(|e| FactsError::Degraded(e.to_string()))?;
            let mut ranges = c.bindings.revoked_ranges().map_err(|e| FactsError::Degraded(e.to_string()))?;
            // Principals revoked while the journal could not record it: their
            // certificate serials are revoked for verifiers all the same.
            for p in force.list() {
                let Some(key) = c.bindings.key_of(&p) else { continue };
                let user_id = clawft_mesh_local::node_id_from_pubkey(&key);
                let issued = c.bindings.serials(&p);
                if let Some(through) = issued.iter().max().copied() {
                    serials.extend(issued);
                    ranges.retain(|(u, _)| *u != user_id);
                    ranges.push((user_id, through));
                }
            }
            serials.sort_unstable();
            serials.dedup();
            ranges.sort();
            (serials, ranges)
        };
        let payload = json!({
            "v": 1, "node_id": self.node_id, "issued_at": now, "serials": serials,
            "ranges": ranges.iter().map(|(u, t)| json!({"user_id": u, "through": t})).collect::<Vec<_>>(),
        })
        .to_string();
        let mut signed_msg = REVOCATION_DOMAIN.to_vec();
        signed_msg.extend_from_slice(payload.as_bytes());
        let rev_sig = self.key.sign(&signed_msg);
        // The hash excludes `issued_at` so an unchanged revocation set does
        // not look like a change on every refresh.
        let stable = json!({"serials": serials, "ranges": ranges}).to_string();
        let rev_hash = hexser::encode(&Sha256::digest(stable.as_bytes()));

        let mut g = self.inner.lock().expect("facts lock");
        let seq = g.seq.saturating_add(1).max(now.saturating_mul(1000));
        let mut collected = g.probed.clone();
        // First, so the bound never truncates it away.
        collected.notes.insert(0, ProbeNote::new("mesh.revocations", format!("sha256:{rev_hash}")));
        let facts = build_facts(&self.node_id, now, self.ttl_s, seq, collected);
        let signed = sign_node_facts(&facts, &self.key)?;
        let doc = json!({
            "facts": signed,
            "revocations": {
                "payload": payload,
                "signature": hexser::encode(&rev_sig.to_bytes()),
                "public_key": hexser::encode(&self.key.verifying_key().to_bytes()),
            },
        });
        let bytes = serde_json::to_vec_pretty(&doc).map_err(std::io::Error::other)?;
        write_private(&self.path, &bytes)?;
        let facts_hash = hexser::encode(&Sha256::digest(&bytes));
        let changed = g.last_revocations_hash.as_deref() != Some(rev_hash.as_str());
        g.seq = seq;
        g.current = Some(doc);
        g.last_revocations_hash = Some(rev_hash.clone());
        drop(g);
        if changed {
            let mut c = core.lock().expect("core lock");
            let body = json!({
                "facts_hash": facts_hash, "valid_until": now.saturating_add(self.ttl_s),
                "revocations_hash": rev_hash,
            });
            if let Err(e) = c.journal.append("facts.sign", body) {
                tracing::error!(error = %e, "journal append failed for facts.sign");
            }
        }
        Ok(())
    }
}

/// Write `bytes` to `path` (mode 0600) via a temp file in the same directory,
/// fsync, then rename, so a reader never sees a partial file.
pub(crate) fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    write_with_mode(path, bytes, 0o600)
}

pub(crate) fn write_with_mode(path: &std::path::Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}
