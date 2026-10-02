//! Mesh peer admission (P3-K1, ADR-103, plan section 1.5).
//!
//! After the optional Noise handshake, a peer that wants a verified
//! identity sends [`AdmitHello`] as its first frame. The hello signs the
//! *session's* Noise handshake hash, so a captured hello cannot be
//! replayed on another session, and binds the node id to the Ed25519 key
//! that the id is derived from ([`node_id_from_pubkey`]).
//!
//! This module is policy-free plumbing: [`AdmitHello::verify`] is pure
//! cryptography; the policy (genesis pin, revocation, verdict, mode) lives
//! behind [`AdmissionGate`]. [`AllowAll`] is today's behaviour;
//! [`CryptoGate`] implements `off | observe | enforce`.
//!
//! # Trust of scopes
//!
//! A peer's `dest_scope` is a *request*; the kernel ignores it and the
//! [`LocalDelivery`](crate::mesh_delivery::LocalDelivery) implementor must
//! treat it as untrusted. A peer's `src_scope` is kept only when
//! admission verified the peer ([`Grant::trust_scope`]); for every other
//! peer `mesh_serve` strips it before delivery.

use async_trait::async_trait;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::node_id::node_id_from_pubkey;

pub use crate::mesh_admit_gate::{
    AdmissionRecord, CryptoGate, OpenVerdicts, Verdict, VerdictRequest, VerdictSource,
    VERDICT_TTL,
};
#[cfg(feature = "exochain")]
pub use crate::mesh_admit_gate::GateVerdictSource;

/// Discriminator that tells a hello apart from a `MeshIpcEnvelope`.
pub const HELLO_KIND: &str = "mesh.admit.hello";
/// Domain-separation prefix of the signed bytes.
pub const HELLO_DOMAIN: &[u8] = b"weftos/mesh-admit/v1\0";
/// Allowed clock skew between hello `ts` and local time.
pub const MAX_SKEW_SECS: u64 = 60;
/// Capability string marking a leaf-class peer.
pub const CAP_LEAF: &str = "leaf";

/// Wire form of the first frame (JSON). Binary fields are lowercase hex.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdmitHello {
    /// Always [`HELLO_KIND`].
    pub kind: String,
    /// Protocol version, currently 1.
    pub v: u32,
    /// `node_id_from_pubkey(pubkey)`.
    pub node_id: String,
    /// Ed25519 public key (64 hex).
    pub pubkey: String,
    /// Free-form platform string.
    #[serde(default)]
    pub platform: String,
    /// Capability tags; [`CAP_LEAF`] marks a leaf-class peer.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Cluster genesis hash the peer believes in (64 hex).
    pub genesis_hash: String,
    /// Unix seconds when signed.
    pub ts: u64,
    /// The peer's Noise static public key (64 hex), or empty.
    #[serde(default)]
    pub noise_static_pub: String,
    /// Signature (128 hex) over [`signed_bytes`].
    pub sig: String,
}

/// What the channel itself proves: the session hash and the remote static.
#[derive(Debug, Clone, Copy)]
pub struct ChannelBinding<'a> {
    /// Noise handshake hash of this session.
    pub handshake_hash: &'a [u8],
    /// Remote Noise static key as seen by the handshake.
    pub remote_static: Option<&'a [u8]>,
}

/// A hello whose node id, key, session binding and timestamp checked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedHello {
    /// The verified node id.
    pub node_id: String,
    /// The verified Ed25519 key.
    pub pubkey: [u8; 32],
    /// Platform string as sent.
    pub platform: String,
    /// Capabilities as sent.
    pub capabilities: Vec<String>,
    /// Genesis hash as signed (not yet compared to a pin).
    pub genesis_hash: [u8; 32],
    /// Signed timestamp.
    pub ts: u64,
}

impl VerifiedHello {
    /// Class implied by the capabilities.
    pub fn class(&self) -> PeerClass {
        if self.capabilities.iter().any(|c| c == CAP_LEAF) {
            PeerClass::Leaf
        } else {
            PeerClass::Node
        }
    }
}

/// Why a hello did not verify, or that none was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelloFailure {
    /// First frame was not a hello (legacy / ESP32 peers).
    Missing,
    /// Hello present but unparsable or wrong version.
    Malformed(String),
    /// `node_id` is not the id derived from `pubkey`.
    NodeIdMismatch,
    /// Channel has no handshake hash (plaintext passthrough).
    NoHandshakeBinding,
    /// `noise_static_pub` differs from the handshake's remote static.
    StaticMismatch,
    /// Signature does not verify over this session's hash.
    BadSignature,
    /// `ts` outside the allowed skew.
    ClockSkew,
}

impl HelloFailure {
    /// Stable machine-readable code for records and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Missing => "hello_missing",
            Self::Malformed(_) => "hello_malformed",
            Self::NodeIdMismatch => "node_id_mismatch",
            Self::NoHandshakeBinding => "no_handshake_binding",
            Self::StaticMismatch => "static_mismatch",
            Self::BadSignature => "bad_signature",
            Self::ClockSkew => "clock_skew",
        }
    }
}

/// The exact bytes a hello signs.
pub fn signed_bytes(handshake_hash: &[u8], genesis: &[u8; 32], ts: u64) -> Vec<u8> {
    let mut b = Vec::with_capacity(HELLO_DOMAIN.len() + handshake_hash.len() + 40);
    b.extend_from_slice(HELLO_DOMAIN);
    b.extend_from_slice(handshake_hash);
    b.extend_from_slice(genesis);
    b.extend_from_slice(&ts.to_be_bytes());
    b
}

impl AdmitHello {
    /// Build and sign a hello for the session identified by
    /// `handshake_hash`. `noise_static_pub` is this side's Noise static
    /// public key (what the peer sees as its remote static).
    pub fn sign(
        key: &SigningKey,
        handshake_hash: &[u8],
        noise_static_pub: &[u8],
        genesis: &[u8; 32],
        ts: u64,
        platform: &str,
        capabilities: Vec<String>,
    ) -> Self {
        let pk = key.verifying_key().to_bytes();
        let sig = key.sign(&signed_bytes(handshake_hash, genesis, ts));
        Self {
            kind: HELLO_KIND.to_owned(),
            v: 1,
            node_id: node_id_from_pubkey(&pk),
            pubkey: hex_encode(&pk),
            platform: platform.to_owned(),
            capabilities,
            genesis_hash: hex_encode(genesis),
            ts,
            noise_static_pub: hex_encode(noise_static_pub),
            sig: hex_encode(&sig.to_bytes()),
        }
    }

    /// Serialize for the wire.
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Classify a first frame: `None` when it is not a hello at all (the
    /// legacy path), `Some(Err)` when it claims to be one but is broken.
    pub fn parse_frame(data: &[u8]) -> Option<Result<Self, HelloFailure>> {
        if data.first() != Some(&b'{') {
            return None;
        }
        let v: serde_json::Value = serde_json::from_slice(data).ok()?;
        if v.get("kind").and_then(|k| k.as_str()) != Some(HELLO_KIND) {
            return None;
        }
        Some(serde_json::from_value(v).map_err(|e| HelloFailure::Malformed(e.to_string())))
    }

    /// Verify everything that needs no policy: id/key binding, session
    /// binding, signature, timestamp. Order follows plan section 1.5.
    pub fn verify(
        &self,
        binding: Option<&ChannelBinding<'_>>,
        now: u64,
    ) -> Result<VerifiedHello, HelloFailure> {
        if self.v != 1 {
            return Err(HelloFailure::Malformed(format!("unsupported version {}", self.v)));
        }
        let pubkey: [u8; 32] = hex_decode_fixed(&self.pubkey)
            .ok_or_else(|| HelloFailure::Malformed("pubkey".into()))?;
        let genesis: [u8; 32] = hex_decode_fixed(&self.genesis_hash)
            .ok_or_else(|| HelloFailure::Malformed("genesis_hash".into()))?;
        let sig: [u8; 64] =
            hex_decode_fixed(&self.sig).ok_or_else(|| HelloFailure::Malformed("sig".into()))?;
        if node_id_from_pubkey(&pubkey) != self.node_id {
            return Err(HelloFailure::NodeIdMismatch);
        }
        let binding = binding.ok_or(HelloFailure::NoHandshakeBinding)?;
        let claimed_static = hex_decode(&self.noise_static_pub)
            .ok_or_else(|| HelloFailure::Malformed("noise_static_pub".into()))?;
        if binding.remote_static != Some(claimed_static.as_slice()) {
            return Err(HelloFailure::StaticMismatch);
        }
        let vk = VerifyingKey::from_bytes(&pubkey)
            .map_err(|_| HelloFailure::Malformed("pubkey not on curve".into()))?;
        vk.verify_strict(
            &signed_bytes(binding.handshake_hash, &genesis, self.ts),
            &Signature::from_bytes(&sig),
        )
        .map_err(|_| HelloFailure::BadSignature)?;
        if now.abs_diff(self.ts) > MAX_SKEW_SECS {
            return Err(HelloFailure::ClockSkew);
        }
        Ok(VerifiedHello {
            node_id: self.node_id.clone(),
            pubkey,
            platform: self.platform.clone(),
            capabilities: self.capabilities.clone(),
            genesis_hash: genesis,
            ts: self.ts,
        })
    }
}

/// How the peer presented itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerClass {
    /// Verified full node.
    Node,
    /// Verified leaf device (hello carried [`CAP_LEAF`]).
    Leaf,
    /// Sent no verifiable hello (old build, ESP32 firmware).
    Legacy,
}

/// Transport protection of the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    /// Noise session (has a handshake hash).
    Noise,
    /// Plaintext.
    Passthrough,
}

/// Facts about the connection handed to the gate.
#[derive(Debug, Clone, Copy)]
pub struct AdmitContext {
    /// Peer class as known so far.
    pub class: PeerClass,
    /// Transport protection.
    pub channel: ChannelKind,
}

/// What a connection may do after admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerLimits {
    /// No restriction beyond the kernel's own.
    None,
    /// Leaf: publish only under `substrate/<own-id>/`, subscribe freely.
    Leaf,
}

/// A refusal reason (also used for would-be refusals under observe).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// Stable code, e.g. `wrong_genesis`, `revoked`, `hello_missing`.
    pub code: &'static str,
    /// Human-readable detail.
    pub detail: String,
}

/// Terms of an admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// Post-admission limits.
    pub limits: PeerLimits,
    /// Keep the peer's `src_scope` (only for fully verified peers).
    pub trust_scope: bool,
    /// Set under `observe` when enforcement would have refused.
    pub observed: Option<Refusal>,
}

/// Gate decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Serve the peer on the given terms.
    Admit(Grant),
    /// Close the connection.
    Refuse(Refusal),
}

/// Admission policy for the mesh listener.
#[async_trait]
pub trait AdmissionGate: Send + Sync + 'static {
    /// A hello verified; decide.
    async fn admit(&self, hello: &VerifiedHello, ctx: &AdmitContext) -> Admission;

    /// No hello, or it failed verification; decide.
    async fn admit_unverified(&self, failure: &HelloFailure, ctx: &AdmitContext) -> Admission;
}

/// Admit everyone: the pre-K1 behaviour. Verified hellos still bind
/// `source_node`, but nothing is ever refused or limited.
#[derive(Debug, Default, Clone, Copy)]
pub struct AllowAll;

#[async_trait]
impl AdmissionGate for AllowAll {
    async fn admit(&self, _: &VerifiedHello, _: &AdmitContext) -> Admission {
        Admission::Admit(Grant { limits: PeerLimits::None, trust_scope: false, observed: None })
    }
    async fn admit_unverified(&self, _: &HelloFailure, _: &AdmitContext) -> Admission {
        Admission::Admit(Grant { limits: PeerLimits::None, trust_scope: false, observed: None })
    }
}

/// Lowercase hex encode.
pub fn hex_encode(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::with_capacity(b.len() * 2), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// Decode hex (empty string decodes to empty).
pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn hex_decode_fixed<const N: usize>(s: &str) -> Option<[u8; N]> {
    hex_decode(s)?.try_into().ok()
}

#[cfg(test)]
#[path = "mesh_admit_tests.rs"]
mod tests;
