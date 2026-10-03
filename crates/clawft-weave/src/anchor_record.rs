//! The user daemon's durable record of accepted anchors (see `anchor_rpc`).
//!
//! `<manifests>/<id>.anchor.json` holds the last accepted statement, where
//! the user chain has it, and `rec_sig`: the user key's signature over
//! `{statement_hash, user_seq, user_event_hash}` under its own domain tag.
//! A record that fails any check (rec_sig, statement signature against the
//! certificate history, project id) is ignored and logged, so a file edited
//! by anyone but the daemon cannot set the baseline.

use std::path::{Path, PathBuf};

use clawft_kernel::chain_anchor::{ANCHOR_SOURCE, KIND_ANCHOR};
use clawft_kernel::project_identity::{self as ident, IdentityError, RevocationView};
use clawft_types::project::canon::{canonical_json, hex_decode, hex_encode};
use clawft_types::project::cert::{ProjectAnchorStmt, key_id};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, VerifyingKey};
use serde_json::json;
use tracing::warn;

use super::{Accepted, AnchorError};
use crate::project_cert_rpc::CertEnv;

/// Domain tag of the record signature.
const RECORD_DOMAIN: &str = "weftos-project-anchor-record-v1\n";

pub(super) fn anchor_file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.anchor.json"))
}

fn record_bytes(statement_hash: &str, user_seq: u64, user_event_hash: &str, epoch: u64) -> Vec<u8> {
    let mut body = json!({
        "statement_hash": statement_hash,
        "user_seq": user_seq,
        "user_event_hash": user_event_hash,
    });
    // Only a non-zero epoch is signed, so records from before epochs keep verifying.
    if epoch > 0 {
        body["epoch"] = json!(epoch);
    }
    format!("{RECORD_DOMAIN}{}", canonical_json(&body)).into_bytes()
}

/// An [`Accepted`] sealed with the user key.
pub(super) fn seal(
    env: &CertEnv,
    statement: ProjectAnchorStmt,
    user_seq: u64,
    user_event_hash: String,
    epoch: u64,
) -> Accepted {
    let bytes = record_bytes(&statement.hash(), user_seq, &user_event_hash, epoch);
    let rec_sig = hex_encode(&env.user_key.sign(&bytes).to_bytes());
    Accepted { statement, user_seq, user_event_hash, rec_sig, epoch }
}

/// Sealed by the key in use, or by a user key rotated out since, provided the
/// statement it seals is dated at or before that key's rotation point
/// (ADR-103 A13) AND the user chain corroborates the record: an anchor event
/// with this `user_seq` and event hash exists below the sequence of that
/// key's `user.key.rotated` event. A record the old key sealed after the
/// rotation point is refused: the daemon seals only with the key in use, so
/// the old key's signature on a later statement is not the daemon's. The
/// date alone is not evidence (whoever holds the old private key can
/// backdate); only the daemon appends to the reserved chain source.
fn seal_ok(env: &CertEnv, a: &Accepted) -> bool {
    let Some(sig) = hex_decode::<64>(&a.rec_sig) else { return false };
    let Ok(history) = crate::project_cert_rpc::user_history(env) else { return false };
    let Some(at) = DateTime::parse_from_rfc3339(&a.statement.at).ok().map(|t| t.with_timezone(&Utc)) else {
        return false;
    };
    let bytes = record_bytes(&a.statement.hash(), a.user_seq, &a.user_event_hash, a.epoch);
    let sig = Signature::from_bytes(&sig);
    let Some(signer) = std::iter::once(*history.current())
        .chain(history.retired_keys())
        .filter(|pk| history.accepts(pk, at))
        .find(|pk| VerifyingKey::from_bytes(pk).is_ok_and(|vk| vk.verify_strict(&bytes, &sig).is_ok()))
    else {
        return false;
    };
    signer == *history.current() || retired_seal_corroborated(env, a, &signer)
}

/// The chain holds the anchor event `a` names, below the rotation of `signer`.
fn retired_seal_corroborated(env: &CertEnv, a: &Accepted, signer: &[u8; 32]) -> bool {
    let events = env.chain.tail(0);
    let signer_id = key_id(signer);
    let rotated_at = events.iter().find_map(|e| {
        let is_rotation = e.source == ident::SOURCE && e.kind == ident::KIND_ROTATED;
        let old = e.payload.as_ref()?.get("record")?.get("old_key_id")?.as_str()?;
        (is_rotation && old == signer_id).then_some(e.sequence)
    });
    let found = rotated_at.is_some_and(|r| {
        events.iter().any(|e| {
            e.source == ANCHOR_SOURCE
                && e.kind == KIND_ANCHOR
                && e.sequence == a.user_seq
                && e.sequence < r
                && ident::hex(&e.hash) == a.user_event_hash
        })
    });
    if !found {
        warn!(
            project = %a.statement.project_id,
            "anchor record sealed by a retired user key is not corroborated by the user chain; ignored"
        );
    }
    found
}

/// The statement still verifies under some certificate ever issued for the
/// project (compromised keys included: this is tamper detection).
fn statement_ok(view: &RevocationView, a: &Accepted) -> bool {
    let s = &a.statement;
    view.all_certs(&s.project_id).iter().any(|c| {
        c.project_key_id == s.project_key_id
            && hex_decode::<32>(&c.project_pubkey).is_some_and(|pk| s.verify(&pk).is_ok())
    })
}

/// The record file, when it exists and passes every check.
pub(super) fn read_file(env: &CertEnv, id: &str, view: &RevocationView) -> Option<Accepted> {
    let bytes = std::fs::read(anchor_file(&env.manifests_dir, id)).ok()?;
    let a: Accepted = match serde_json::from_slice(&bytes) {
        Ok(a) => a,
        Err(e) => {
            warn!(project = id, error = %e, "anchor record is unreadable; ignored");
            return None;
        }
    };
    let ok = a.statement.project_id == id && seal_ok(env, &a) && statement_ok(view, &a);
    if !ok {
        warn!(project = id, "anchor record fails its signature checks; ignored");
    }
    ok.then_some(a)
}

pub(super) fn write_file(env: &CertEnv, a: &Accepted) -> Result<(), AnchorError> {
    let bytes = serde_json::to_vec_pretty(a).map_err(|e| AnchorError::Store(e.to_string()))?;
    ident::write_private_atomic(&anchor_file(&env.manifests_dir, &a.statement.project_id), &bytes, false)
        .map_err(|e: IdentityError| AnchorError::Store(format!("record accepted anchor: {e}")))
}

pub(super) fn append_event(env: &CertEnv, stmt: &ProjectAnchorStmt, recovered: Option<&Accepted>, epoch: u64) -> Accepted {
    let mut payload = json!({
        "project_id": stmt.project_id,
        "statement": stmt,
        "statement_hash": stmt.hash(),
    });
    if epoch > 0 {
        payload["epoch"] = json!(epoch);
    }
    if let Some(old) = recovered {
        payload["recovered"] = json!(true);
        payload["original_user_seq"] = json!(old.user_seq);
        payload["original_user_event_hash"] = json!(old.user_event_hash);
    }
    let ev = env.chain.append(ANCHOR_SOURCE, KIND_ANCHOR, Some(payload));
    seal(env, stmt.clone(), ev.sequence, ident::hex(&ev.hash), epoch)
}
