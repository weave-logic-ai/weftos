//! The owner's signed anchor resets (see `anchor_rpc::reset`).
//!
//! A project whose chain was reset restarts its anchors at `seq = 1` with no
//! predecessor, which the user daemon refuses while it holds the old head
//! (review S2). `project.anchor.reset` retires that head: the daemon appends
//! a `project.anchor.reset` event to the user chain and writes a record,
//! sealed with the user key, to `<manifests>/<id>.anchor-reset.json`. The
//! number of the latest verified reset is the project's *epoch*; statements
//! and records of an earlier epoch stay in the chain as history and are never
//! the baseline, so the project anchors again from genesis.
//!
//! The record file survives a crash (the user chain is saved only on clean
//! shutdown); the chain event is read as well, and the higher epoch wins. A
//! record that fails its signature is ignored.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use clawft_kernel::chain_anchor::ANCHOR_SOURCE;
use clawft_kernel::project_identity as ident;
use clawft_types::project::canon::{canonical_json, hex_decode, hex_encode};
use ed25519_dalek::{Signature, Signer, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::warn;

use super::AnchorError;
use crate::project_cert_rpc::CertEnv;

/// Kind of the user-chain event.
pub const KIND_RESET: &str = "project.anchor.reset";
/// Domain tag of the record signature.
const RESET_DOMAIN: &str = "weftos-project-anchor-reset-v1\n";

/// One reset, sealed by the user key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResetRecord {
    /// Project id.
    pub project_id: String,
    /// The epoch this reset opens (1 for the first).
    pub epoch: u64,
    /// Hash of the statement that was the head when the owner reset.
    pub retired_statement_hash: String,
    /// Its `seq`.
    pub retired_seq: u64,
    /// User-chain sequence of the `project.anchor.reset` event.
    pub user_seq: u64,
    /// That event's hash, hex.
    pub user_event_hash: String,
    /// When, `YYYY-MM-DDTHH:MM:SSZ`.
    pub at: String,
    /// The owner's reason (control characters removed, capped).
    #[serde(default)]
    pub reason: String,
    /// User-key signature over the fields above (domain-tagged).
    #[serde(default)]
    pub rec_sig: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ResetFile {
    resets: Vec<ResetRecord>,
}

fn file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.anchor-reset.json"))
}

fn bytes(r: &ResetRecord) -> Vec<u8> {
    let body = json!({
        "project_id": r.project_id,
        "epoch": r.epoch,
        "retired_statement_hash": r.retired_statement_hash,
        "retired_seq": r.retired_seq,
        "user_seq": r.user_seq,
        "user_event_hash": r.user_event_hash,
        "at": r.at,
        "reason": r.reason,
    });
    format!("{RESET_DOMAIN}{}", canonical_json(&body)).into_bytes()
}

pub(super) fn sign(env: &CertEnv, r: &mut ResetRecord) {
    r.rec_sig = hex_encode(&env.user_key.sign(&bytes(r)).to_bytes());
}

/// Sealed by the key in use, or by a retired user key before its rotation point.
fn sig_ok(env: &CertEnv, r: &ResetRecord) -> bool {
    let Some(sig) = hex_decode::<64>(&r.rec_sig) else { return false };
    let Ok(history) = crate::project_cert_rpc::user_history(env) else { return false };
    let Some(at) = DateTime::parse_from_rfc3339(&r.at).ok().map(|t| t.with_timezone(&Utc)) else { return false };
    let sig = Signature::from_bytes(&sig);
    std::iter::once(*history.current())
        .chain(history.retired_keys())
        .filter(|pk| history.accepts(pk, at))
        .filter_map(|pk| VerifyingKey::from_bytes(&pk).ok())
        .any(|vk| vk.verify_strict(&bytes(r), &sig).is_ok())
}

fn read_records(env: &CertEnv, id: &str) -> Vec<ResetRecord> {
    let Ok(raw) = std::fs::read(file(&env.manifests_dir, id)) else { return Vec::new() };
    match serde_json::from_slice::<ResetFile>(&raw) {
        Ok(f) => f
            .resets
            .into_iter()
            .filter(|r| {
                let ok = r.project_id == id && sig_ok(env, r);
                if !ok {
                    warn!(project = id, "anchor reset record fails its signature checks; ignored");
                }
                ok
            })
            .collect(),
        Err(e) => {
            warn!(project = id, error = %e, "anchor reset record is unreadable; ignored");
            Vec::new()
        }
    }
}

/// The project's current epoch: the highest reset in the record file or on
/// the user chain (0 when the owner never reset).
pub(super) fn current_epoch(env: &CertEnv, id: &str) -> u64 {
    let from_chain = env
        .chain
        .tail(0)
        .iter()
        .filter(|e| e.source == ANCHOR_SOURCE && e.kind == KIND_RESET)
        .filter_map(|e| {
            let p = e.payload.as_ref()?;
            (p.get("project_id")?.as_str()? == id).then(|| p.get("epoch")?.as_u64()).flatten()
        })
        .max()
        .unwrap_or(0);
    let from_file = read_records(env, id).iter().map(|r| r.epoch).max().unwrap_or(0);
    from_chain.max(from_file)
}

/// Add `r` to the record file (atomic, 0600).
pub(super) fn record(env: &CertEnv, r: &ResetRecord) -> Result<(), AnchorError> {
    let mut all = read_records(env, &r.project_id);
    all.push(r.clone());
    let body = serde_json::to_vec_pretty(&ResetFile { resets: all }).map_err(|e| AnchorError::Store(e.to_string()))?;
    ident::write_private_atomic(&file(&env.manifests_dir, &r.project_id), &body, false)
        .map_err(|e| AnchorError::Store(format!("record anchor reset: {e}")))
}
