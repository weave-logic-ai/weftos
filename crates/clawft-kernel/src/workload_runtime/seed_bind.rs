//! Fleet identity binding for Cognitum Seeds (card mesh-placement-22; ADR-100
//! section 5; `docs/research/mesh-placement/fleet-compat.md` section 2c).
//!
//! A Seed has a device identity (`GET /api/v1/identity`: a device id and an
//! Ed25519 public key) but no WeftOS node key. The operator binds the two
//! with a signed record `{device_id, device_pubkey, node_id, bound_at}`;
//! [`SeedBinder::bind`] verifies it against the pinned operator keys and
//! against the Seed itself, then chains `workload.node.bind`. Anything
//! else (an unpinned signer, a changed record, a key that is not the
//! device's, a record for another node, an expired or replayed record) is
//! refused with a chained `workload.refuse`.
//!
//! The node id of a Seed is the id derived from a per-Seed *adapter key*
//! held by WeftOS (never by the Seed): the Seed cannot sign for itself, so
//! the adapter signs the Seed's facts ([`attest_seed_facts`]) and every
//! capability carries provenance `claimed`, since the device's own report
//! is not something WeftOS measured.
//!
//! Bearer tokens never reach a record, an error or a chain event: the only
//! Seed data used is the (public) identity.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use clawft_types::placement::{Capability, NodeFacts, ProbeNote, Provenance};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::seed::{API_TIMEOUT, SeedApiRuntime};
use super::seed_http::Method;
use super::types::{RuntimeError, WorkloadRuntime};
use crate::chain::{ChainManager, EVENT_KIND_WORKLOAD_NODE_BIND, EVENT_KIND_WORKLOAD_REFUSE};
use crate::node_facts_advert::{NodeFactsAdvertError, SignedNodeFacts, sign_node_facts};
use crate::node_registry::node_id_from_pubkey;
use crate::workload_pkg::codec::hex_encode;
use crate::workload_pkg::manifest::valid_token;

/// Chain source of bind events.
pub const BIND_CHAIN_SOURCE: &str = "workload.bind";
/// Domain tag signed before a bind record.
pub const BIND_DOMAIN: &[u8] = b"weftos.workload.node.bind.v1\0";
/// Default age after which a bind record is no longer accepted.
pub const DEFAULT_MAX_BIND_AGE_SECS: u64 = 24 * 3600;
/// Tolerated clock skew for `bound_at`.
pub const BIND_CLOCK_SKEW_SECS: u64 = 60;
/// Facts lifetime for an adapter-attested Seed.
pub const SEED_FACTS_TTL_SECS: u64 = 300;
const MAX_RECORD_BYTES: usize = 4096;

/// What the operator signs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindRecord {
    /// The Seed's device id, as `GET /api/v1/identity` reports it.
    pub device_id: String,
    /// The Seed's device public key, exactly as `GET /api/v1/identity`
    /// reports it.
    pub device_pubkey: String,
    /// The WeftOS node id the Seed is bound to (derived from the adapter
    /// key, [`seed_node_id`]).
    pub node_id: String,
    /// Unix seconds the operator made the binding.
    pub bound_at: u64,
}

/// A bind record with the operator's signature over the exact bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedBind {
    /// Exact JSON bytes of the [`BindRecord`] that were signed.
    pub record: String,
    /// Operator Ed25519 public key (32 bytes).
    pub operator_key: Vec<u8>,
    /// Ed25519 signature (64 bytes) over [`BIND_DOMAIN`] || record.
    pub signature: Vec<u8>,
}

/// Node id of a Seed whose adapter key is `adapter`.
pub fn seed_node_id(adapter: &SigningKey) -> String {
    node_id_from_pubkey(&adapter.verifying_key().to_bytes())
}

fn signed_bytes(record: &str) -> Vec<u8> {
    let mut v = BIND_DOMAIN.to_vec();
    v.extend_from_slice(record.as_bytes());
    v
}

/// Sign `record` as the operator.
pub fn sign_bind(record: &BindRecord, operator: &SigningKey) -> SignedBind {
    let record = serde_json::to_string(record).unwrap_or_default();
    let signature = operator.sign(&signed_bytes(&record)).to_bytes().to_vec();
    SignedBind {
        record,
        operator_key: operator.verifying_key().to_bytes().to_vec(),
        signature,
    }
}

/// Why a bind was refused. [`Self::code`] is the stable chain code.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindError {
    /// Not a well-formed record or envelope.
    #[error("malformed bind record: {0}")]
    Malformed(String),
    /// Signed by a key that is not a pinned operator key.
    #[error("bind record is not signed by a pinned operator key")]
    UntrustedOperator,
    /// The signature does not verify: the record was altered.
    #[error("bind record signature does not verify")]
    BadSignature,
    /// The record names another node than this adapter's.
    #[error("bind record is for node {record}, this adapter is node {adapter}")]
    WrongNode {
        /// Node id in the record.
        record: String,
        /// This adapter's node id.
        adapter: String,
    },
    /// The Seed's own identity differs from the record.
    #[error("the Seed's identity does not match the bind record ({0})")]
    IdentityMismatch(&'static str),
    /// `bound_at` is in the future or older than the accepted age.
    #[error("bind record is outside its validity window")]
    Expired,
    /// This exact record was already accepted, or is older than the
    /// binding held for the device.
    #[error("bind record replayed or older than the current binding")]
    Replayed,
    /// The node is already bound to a different device.
    #[error("node {0} is already bound to another device")]
    NodeTaken(String),
    /// The Seed could not be asked.
    #[error("could not read the Seed identity: {0}")]
    Seed(String),
}

impl BindError {
    /// Stable code recorded in the chained refusal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Malformed(_) => "malformed",
            Self::UntrustedOperator => "untrusted_operator",
            Self::BadSignature => "bad_signature",
            Self::WrongNode { .. } => "wrong_node",
            Self::IdentityMismatch(_) => "identity_mismatch",
            Self::Expired => "expired",
            Self::Replayed => "replayed",
            Self::NodeTaken(_) => "node_taken",
            Self::Seed(_) => "seed_unreachable",
        }
    }
}

/// A verified binding. Only [`SeedBinder::bind`] makes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    record: BindRecord,
}

impl Binding {
    /// The bound record.
    pub fn record(&self) -> &BindRecord {
        &self.record
    }
}

/// Verifies and remembers bindings.
pub struct SeedBinder {
    operators: Vec<[u8; 32]>,
    chain: Arc<ChainManager>,
    max_age_secs: u64,
    seen: Mutex<HashSet<[u8; 32]>>,
    /// Device id to (bound_at, node id).
    bound: Mutex<BTreeMap<String, (u64, String)>>,
}

impl SeedBinder {
    /// A binder trusting `operators` (pinned operator public keys).
    pub fn new(operators: Vec<[u8; 32]>, chain: Arc<ChainManager>) -> Self {
        Self {
            operators,
            chain,
            max_age_secs: DEFAULT_MAX_BIND_AGE_SECS,
            seen: Mutex::new(HashSet::new()),
            bound: Mutex::new(BTreeMap::new()),
        }
    }

    /// Accept records for at most `secs` after `bound_at`.
    pub fn with_max_age_secs(mut self, secs: u64) -> Self {
        self.max_age_secs = secs;
        self
    }

    /// The device id bound to `node_id`, if any.
    pub fn device_of(&self, node_id: &str) -> Option<String> {
        self.bound
            .lock()
            .ok()?
            .iter()
            .find(|(_, (_, n))| n == node_id)
            .map(|(d, _)| d.clone())
    }

    fn refuse(&self, signed: &SignedBind, e: &BindError) {
        let record_hash = hex_encode(&Sha256::digest(signed.record.as_bytes()));
        // Only identifiers, clipped: never the record or any token.
        let claimed = serde_json::from_str::<Value>(&signed.record).unwrap_or(Value::Null);
        let clip = |k: &str| {
            claimed
                .get(k)
                .and_then(Value::as_str)
                .map(|s| s.chars().take(64).collect::<String>())
        };
        self.chain.append(
            BIND_CHAIN_SOURCE,
            EVENT_KIND_WORKLOAD_REFUSE,
            Some(json!({
                "phase": "node.bind", "code": e.code(), "reason": e.to_string(),
                "record_hash": record_hash,
                "claimed_node_id": clip("node_id"), "claimed_device_id": clip("device_id"),
            })),
        );
    }

    /// Verify `signed` for `rt` at `now` (unix seconds) and chain the
    /// outcome (`workload.node.bind`, or `workload.refuse` on any failure).
    pub async fn bind(
        &self,
        signed: &SignedBind,
        rt: &SeedApiRuntime,
        now: u64,
    ) -> Result<Binding, BindError> {
        match self.verify(signed, rt, now).await {
            Ok(b) => Ok(b),
            Err(e) => {
                self.refuse(signed, &e);
                Err(e)
            }
        }
    }

    async fn verify(
        &self,
        signed: &SignedBind,
        rt: &SeedApiRuntime,
        now: u64,
    ) -> Result<Binding, BindError> {
        let bad = |m: &str| BindError::Malformed(m.into());
        if signed.record.len() > MAX_RECORD_BYTES {
            return Err(bad("record too large"));
        }
        let key: [u8; 32] = signed
            .operator_key
            .as_slice()
            .try_into()
            .map_err(|_| bad("operator key is not 32 bytes"))?;
        let sig: [u8; 64] = signed
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| bad("signature is not 64 bytes"))?;
        if !self.operators.contains(&key) {
            return Err(BindError::UntrustedOperator);
        }
        let vk = VerifyingKey::from_bytes(&key).map_err(|_| bad("bad operator key"))?;
        vk.verify(&signed_bytes(&signed.record), &Signature::from_bytes(&sig))
            .map_err(|_| BindError::BadSignature)?;
        let record: BindRecord = serde_json::from_str(&signed.record)
            .map_err(|e| BindError::Malformed(e.to_string()))?;
        if !valid_token(&record.device_id, 128)
            || !valid_token(&record.node_id, 64)
            || record.device_pubkey.is_empty()
            || record.device_pubkey.len() > 256
        {
            return Err(bad("device id, device key or node id is not well formed"));
        }
        let adapter = rt.node_id().to_string();
        if record.node_id != adapter {
            return Err(BindError::WrongNode {
                record: record.node_id,
                adapter,
            });
        }
        if record.bound_at > now.saturating_add(BIND_CLOCK_SKEW_SECS)
            || now.saturating_sub(record.bound_at) > self.max_age_secs
        {
            return Err(BindError::Expired);
        }
        // The Seed is asked last, so an untrusted record costs it nothing.
        let (device_id, device_key) = rt
            .identity()
            .await
            .map_err(|e| BindError::Seed(e.to_string()))?;
        if device_id != record.device_id {
            return Err(BindError::IdentityMismatch("device id"));
        }
        if device_key != record.device_pubkey {
            return Err(BindError::IdentityMismatch("device key"));
        }
        let digest: [u8; 32] =
            Sha256::digest([signed.record.as_bytes(), signed.signature.as_slice()].concat()).into();
        let mut seen = self.seen.lock().map_err(|_| bad("binder poisoned"))?;
        let mut bound = self.bound.lock().map_err(|_| bad("binder poisoned"))?;
        if seen.contains(&digest) {
            return Err(BindError::Replayed);
        }
        if bound
            .get(&record.device_id)
            .is_some_and(|(at, _)| record.bound_at <= *at)
        {
            return Err(BindError::Replayed);
        }
        if bound
            .iter()
            .any(|(d, (_, n))| *n == record.node_id && *d != record.device_id)
        {
            return Err(BindError::NodeTaken(record.node_id));
        }
        seen.insert(digest);
        bound.insert(
            record.device_id.clone(),
            (record.bound_at, record.node_id.clone()),
        );
        self.chain.append(
            BIND_CHAIN_SOURCE,
            EVENT_KIND_WORKLOAD_NODE_BIND,
            Some(json!({
                "device_id": record.device_id, "device_pubkey": record.device_pubkey,
                "node_id": record.node_id, "bound_at": record.bound_at,
                "operator_key": hex_encode(&key),
                "record_hash": hex_encode(&Sha256::digest(signed.record.as_bytes())),
            })),
        );
        Ok(Binding { record })
    }
}

impl SeedApiRuntime {
    /// This adapter's operator-assigned node id.
    pub fn node_id(&self) -> &str {
        &self.cfg.node_id
    }

    /// The Seed's own `(device_id, public_key)` from `GET /api/v1/identity`.
    pub async fn identity(&self) -> Result<(String, String), RuntimeError> {
        let v = self
            .api(Method::Get, "/api/v1/identity", None, API_TIMEOUT)
            .await?;
        let field = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty() && s.len() <= 256)
                .map(str::to_string)
                .ok_or_else(|| RuntimeError::Backend(format!("seed /api/v1/identity: no {k}")))
        };
        Ok((field("device_id")?, field("public_key")?))
    }
}

/// Facts for a bound Seed, signed by the adapter key. Every capability is
/// forced to provenance `claimed`: the Seed's own report, relayed by the
/// adapter, is never `probed` or `measured` (a `measured` value only comes
/// from WeftOS's own conformance runs).
pub fn attest_seed_facts(
    binding: &Binding,
    rt: &SeedApiRuntime,
    adapter: &SigningKey,
    now: u64,
    seq: u64,
) -> Result<SignedNodeFacts, NodeFactsAdvertError> {
    let node_id = seed_node_id(adapter);
    if node_id != binding.record.node_id {
        return Err(NodeFactsAdvertError::NodeMismatch {
            claimed: binding.record.node_id.clone(),
            derived: node_id,
        });
    }
    let mut facts = NodeFacts::new(node_id, now, SEED_FACTS_TTL_SECS, seq);
    facts.capabilities = rt
        .provides()
        .into_iter()
        .map(|c: Capability| Capability {
            provenance: Provenance::Claimed,
            ..c
        })
        .collect();
    facts.notes.push(ProbeNote::new(
        "remote.api",
        format!(
            "claimed by the adapter for Seed device {} (bound by an operator record), not probed",
            binding.record.device_id
        ),
    ));
    sign_node_facts(&facts, adapter)
}
