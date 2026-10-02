//! Answers the service's `verdict.request` with the daemon's governance gate
//! (ADR-103 P3-U). The service only caches the answer; the decision, and the
//! rule that made it, belong to the gate.
//!
//! Fails closed: no gate, a deferral and an unknown subject all deny with a
//! zero TTL, so nothing is cached.

use clawft_kernel::gate::{GateBackend, GateDecision};
use clawft_mesh_local::proto::{VerdictRequest, VerdictSubject};
use sha2::{Digest, Sha256};

/// How long the service may cache a permit.
pub const PERMIT_TTL_S: u64 = 300;
/// How long it may cache a denial.
pub const DENY_TTL_S: u64 = 30;

/// The reply to a verdict request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdictAnswer {
    /// Permit or deny.
    pub allow: bool,
    /// Cache lifetime for the service.
    pub ttl_s: u64,
    /// Why.
    pub reason: String,
    /// Hex SHA-256 of the gate's witness (permit token or deny receipt) when
    /// it produced one, else empty.
    pub rule_hash: String,
}

/// The gate action a subject maps to (`None` for a subject this build does
/// not know, which is denied).
pub fn action_for(subject: &VerdictSubject) -> Option<&'static str> {
    match subject {
        VerdictSubject::PeerAdmit => Some("peer.admit"),
        VerdictSubject::ClusterJoin => Some("cluster.join"),
        VerdictSubject::Publish => Some("publish"),
        VerdictSubject::Subscribe => Some("subscribe"),
        _ => None,
    }
}

fn witness_hash(bytes: Option<&[u8]>) -> String {
    bytes.map_or_else(String::new, |b| {
        Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
    })
}

fn deny(reason: impl Into<String>) -> VerdictAnswer {
    VerdictAnswer { allow: false, ttl_s: 0, reason: reason.into(), rule_hash: String::new() }
}

/// Decide `req` with `gate` on behalf of `user_id`.
pub fn answer(gate: Option<&dyn GateBackend>, user_id: &str, req: &VerdictRequest) -> VerdictAnswer {
    let Some(action) = action_for(&req.subject) else {
        return deny("unknown verdict subject");
    };
    let Some(gate) = gate else {
        return deny("no governance gate available");
    };
    let ctx = serde_json::json!({
        "node_id": req.peer.node_id,
        "pubkey": req.peer.pubkey,
        "platform": req.peer.platform,
        "capabilities": req.peer.capabilities,
        "genesis_hash": req.peer.genesis_hash,
        "chain_seq": req.peer.chain_seq,
        "topic": req.topic,
        "via": "mesh-service",
    });
    // The agent id is the user: the gate rules are the user's.
    match gate.check(user_id, action, &ctx) {
        GateDecision::Permit { token } => VerdictAnswer {
            allow: true,
            ttl_s: PERMIT_TTL_S,
            reason: String::new(),
            rule_hash: witness_hash(token.as_deref()),
        },
        GateDecision::Defer { reason } => deny(format!("deferred: {reason}")),
        GateDecision::Deny { reason, receipt } => VerdictAnswer {
            allow: false,
            ttl_s: DENY_TTL_S,
            reason,
            rule_hash: witness_hash(receipt.as_deref()),
        },
        _ => deny("unrecognised gate decision"),
    }
}
