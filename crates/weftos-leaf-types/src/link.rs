//! ADR-103 leaf identity and offline publish wire contract.
//!
//! All signed bytes use explicit length prefixes and domain separation.
//! The same module builds firmware frames and verifies them on the parent.

use alloc::{string::String, vec::Vec};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CERT_DOMAIN: &[u8] = b"weftos/leaf-cert/v1\0";
const PUBLISH_DOMAIN: &[u8] = b"weftos/leaf-publish/v1\0";
const ACK_DOMAIN: &[u8] = b"weftos/leaf-ack/v1\0";
const DISCOVERY_DOMAIN: &[u8] = b"weftos/leaf-discovery/v1\0";
pub const VERSION: u8 = 1;
pub const FRAME_MAGIC: &[u8; 4] = b"WLF1";
pub const ACK_MAGIC: &[u8; 4] = b"WLA1";

fn field(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
    out.extend_from_slice(value);
}

fn verify(pubkey: &[u8; 32], bytes: &[u8], signature: &[u8]) -> Result<(), LinkError> {
    let key = VerifyingKey::from_bytes(pubkey).map_err(|_| LinkError::BadKey)?;
    let sig: [u8; 64] = signature.try_into().map_err(|_| LinkError::BadSignature)?;
    key.verify_strict(bytes, &Signature::from_bytes(&sig)).map_err(|_| LinkError::BadSignature)
}

/// Same identity rule as the machine mesh: hex(SHA-256(pubkey)[..16]).
pub fn node_id(pubkey: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let hash = Sha256::digest(pubkey);
    let mut out = String::with_capacity(32);
    for b in &hash[..16] {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 15) as usize] as char);
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    BadVersion,
    BadKey,
    BadSignature,
    WrongParent,
    WrongLeaf,
    Expired,
    NotYetValid,
    InvalidLifetime,
    InvalidTarget,
    InvalidSequence,
    InvalidEndpoint,
    MissingCapability,
    InvalidScope,
    JournalFull,
    WrongAckOrder,
}

/// Issued by a user or project key. The parent's public key must be pinned
/// separately by the receiver; a self-provided issuer key grants no trust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeafCertificate {
    pub v: u8,
    pub parent_scope: String,
    pub parent_pubkey: [u8; 32],
    /// Mesh service key that signs acknowledgments and discovery replies.
    pub mesh_pubkey: [u8; 32],
    pub leaf_pubkey: [u8; 32],
    pub serial: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub capabilities: Vec<String>,
    pub sig: Vec<u8>,
}

/// Canonical tenant address used by the machine mesh router. The user id is
/// always present so a certified leaf can never fall back to prefix routing.
pub fn parse_parent_scope(scope: &str) -> Result<(&str, Option<&str>), LinkError> {
    let (kind, rest) = scope.split_once(':').ok_or(LinkError::InvalidScope)?;
    let (user, project) = match kind {
        "user" => (rest, None),
        "project" => {
            let (user, project) = rest.split_once(':').ok_or(LinkError::InvalidScope)?;
            (user, Some(project))
        }
        _ => return Err(LinkError::InvalidScope),
    };
    if user.len() != 32 || !user.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return Err(LinkError::InvalidScope);
    }
    if project.is_some_and(|p| p.len() != 26 || !p.bytes().all(|b| b.is_ascii_alphanumeric() && !b.is_ascii_lowercase())) {
        return Err(LinkError::InvalidScope);
    }
    Ok((user, project))
}

impl LeafCertificate {
    #[allow(clippy::too_many_arguments)]
    pub fn issue(
        parent: &SigningKey,
        parent_scope: String,
        mesh_pubkey: [u8; 32],
        leaf_pubkey: [u8; 32],
        serial: u64,
        issued_at: u64,
        expires_at: u64,
        capabilities: Vec<String>,
    ) -> Result<Self, LinkError> {
        if parse_parent_scope(&parent_scope).is_err() || issued_at >= expires_at || serial == 0 {
            return Err(LinkError::InvalidLifetime);
        }
        let mut cert = Self {
            v: VERSION, parent_scope, parent_pubkey: parent.verifying_key().to_bytes(), mesh_pubkey,
            leaf_pubkey, serial, issued_at, expires_at, capabilities, sig: Vec::new(),
        };
        cert.sig = parent.sign(&cert.signed_bytes()).to_bytes().to_vec();
        Ok(cert)
    }

    fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::from(CERT_DOMAIN);
        out.push(self.v);
        field(&mut out, self.parent_scope.as_bytes());
        out.extend_from_slice(&self.parent_pubkey);
        out.extend_from_slice(&self.mesh_pubkey);
        out.extend_from_slice(&self.leaf_pubkey);
        out.extend_from_slice(&self.serial.to_be_bytes());
        out.extend_from_slice(&self.issued_at.to_be_bytes());
        out.extend_from_slice(&self.expires_at.to_be_bytes());
        out.extend_from_slice(&(self.capabilities.len() as u32).to_be_bytes());
        for cap in &self.capabilities { field(&mut out, cap.as_bytes()); }
        out
    }

    pub fn verify(&self, pinned_parent: &[u8; 32], now: u64) -> Result<(), LinkError> {
        if self.v != VERSION { return Err(LinkError::BadVersion); }
        if &self.parent_pubkey != pinned_parent { return Err(LinkError::WrongParent); }
        if parse_parent_scope(&self.parent_scope).is_err() || self.serial == 0 || self.issued_at >= self.expires_at {
            return Err(LinkError::InvalidLifetime);
        }
        if now < self.issued_at { return Err(LinkError::NotYetValid); }
        if now >= self.expires_at { return Err(LinkError::Expired); }
        verify(pinned_parent, &self.signed_bytes(), &self.sig)
    }

    pub fn leaf_id(&self) -> String { node_id(&self.leaf_pubkey) }
}

/// One journal entry. The leaf signs it before placing it in durable storage.
/// It remains byte-identical across retries and parent changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedPublish {
    pub v: u8,
    pub cert: LeafCertificate,
    pub seq: u64,
    pub target: String,
    pub payload: Vec<u8>,
    pub sig: Vec<u8>,
}

impl SignedPublish {
    pub fn sign(cert: LeafCertificate, leaf: &SigningKey, seq: u64, target: String, payload: Vec<u8>) -> Result<Self, LinkError> {
        if seq == 0 { return Err(LinkError::InvalidSequence); }
        if leaf.verifying_key().to_bytes() != cert.leaf_pubkey { return Err(LinkError::WrongLeaf); }
        let mut frame = Self { v: VERSION, cert, seq, target, payload, sig: Vec::new() };
        frame.check_target()?;
        frame.sig = leaf.sign(&frame.signed_bytes()).to_bytes().to_vec();
        Ok(frame)
    }

    pub fn digest(&self) -> [u8; 32] {
        let mut bytes = self.signed_bytes();
        field(&mut bytes, &self.sig);
        Sha256::digest(bytes).into()
    }

    fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::from(PUBLISH_DOMAIN);
        out.push(self.v);
        field(&mut out, &self.cert.signed_bytes());
        field(&mut out, &self.cert.sig);
        out.extend_from_slice(&self.seq.to_be_bytes());
        field(&mut out, self.target.as_bytes());
        field(&mut out, &self.payload);
        out
    }

    fn check_target(&self) -> Result<(), LinkError> {
        let id = self.cert.leaf_id();
        let (valid, capability) = if let Some(path) = self.target.strip_prefix(&alloc::format!("substrate/{id}/")) {
            let valid = !path.is_empty() && path.split('/').all(|part| !part.is_empty() && part != "." && part != ".."
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')));
            (valid, "substrate.publish")
        } else if self.target == alloc::format!("mesh.leaf.{id}.input") {
            (true, "input.publish")
        } else if self.target == alloc::format!("mesh.leaf.{id}.announce") {
            (true, "announce.publish")
        } else if self.target == "mesh.subscribe" {
            (true, "push.subscribe")
        } else { (false, "") };
        if !valid { return Err(LinkError::InvalidTarget); }
        if !self.cert.capabilities.iter().any(|c| c == capability) { return Err(LinkError::MissingCapability); }
        Ok(())
    }

    pub fn verify(&self, pinned_parent: &[u8; 32], now: u64) -> Result<(), LinkError> {
        if self.v != VERSION { return Err(LinkError::BadVersion); }
        self.cert.verify(pinned_parent, now)?;
        if self.seq == 0 { return Err(LinkError::InvalidSequence); }
        self.check_target()?;
        verify(&self.cert.leaf_pubkey, &self.signed_bytes(), &self.sig)
    }
}

/// Parent signature makes a forged plaintext acknowledgment unable to erase
/// the leaf's durable queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishAck {
    pub v: u8,
    pub leaf_id: String,
    pub seq: u64,
    pub frame_digest: [u8; 32],
    pub sig: Vec<u8>,
}

impl PublishAck {
    pub fn sign(parent: &SigningKey, frame: &SignedPublish) -> Self {
        let mut ack = Self { v: VERSION, leaf_id: frame.cert.leaf_id(), seq: frame.seq, frame_digest: frame.digest(), sig: Vec::new() };
        ack.sig = parent.sign(&ack.signed_bytes()).to_bytes().to_vec();
        ack
    }
    fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::from(ACK_DOMAIN);
        out.push(self.v);
        field(&mut out, self.leaf_id.as_bytes());
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.frame_digest);
        out
    }
    pub fn verify(&self, parent: &[u8; 32], frame: &SignedPublish) -> Result<(), LinkError> {
        if self.v != VERSION { return Err(LinkError::BadVersion); }
        if parent != &frame.cert.mesh_pubkey { return Err(LinkError::WrongParent); }
        if self.leaf_id != frame.cert.leaf_id() || self.seq != frame.seq || self.frame_digest != frame.digest() {
            return Err(LinkError::WrongLeaf);
        }
        verify(parent, &self.signed_bytes(), &self.sig)
    }
}

/// Parent advertises a routable address; a leaf accepts it only when signed
/// by its pinned parent and fresh. `endpoint` is `host:port` or `[v6]:port`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentAdvertisement {
    pub v: u8,
    pub parent_scope: String,
    pub endpoint: String,
    pub expires_at: u64,
    pub nonce: [u8; 16],
    pub sig: Vec<u8>,
}

impl ParentAdvertisement {
    pub fn sign(parent: &SigningKey, parent_scope: String, endpoint: String, expires_at: u64, nonce: [u8; 16]) -> Self {
        let mut ad = Self { v: VERSION, parent_scope, endpoint, expires_at, nonce, sig: Vec::new() };
        ad.sig = parent.sign(&ad.signed_bytes()).to_bytes().to_vec();
        ad
    }
    fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::from(DISCOVERY_DOMAIN);
        out.push(self.v);
        field(&mut out, self.parent_scope.as_bytes());
        field(&mut out, self.endpoint.as_bytes());
        out.extend_from_slice(&self.expires_at.to_be_bytes());
        out.extend_from_slice(&self.nonce);
        out
    }
    pub fn verify(&self, parent: &[u8; 32], scope: &str, now: u64) -> Result<(), LinkError> {
        if self.v != VERSION { return Err(LinkError::BadVersion); }
        if self.parent_scope != scope { return Err(LinkError::WrongParent); }
        if now >= self.expires_at { return Err(LinkError::Expired); }
        if self.endpoint.is_empty() || !self.endpoint.contains(':') { return Err(LinkError::InvalidEndpoint); }
        verify(parent, &self.signed_bytes(), &self.sig)
    }
    pub fn verify_for(&self, parent: &[u8; 32], scope: &str, now: u64, nonce: &[u8; 16]) -> Result<(), LinkError> {
        self.verify(parent, scope, now)?;
        if &self.nonce != nonce { return Err(LinkError::BadSignature); }
        Ok(())
    }
}

/// Durable outbound queue state. Store a CBOR snapshot before attempting a
/// send and again after a verified ACK. `next_seq` survives an empty queue,
/// so reconnect or reboot never reuses a sequence number. A full queue
/// refuses new data instead of silently dropping old observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfflineJournal {
    pub next_seq: u64,
    pub pending: Vec<SignedPublish>,
}

impl Default for OfflineJournal {
    fn default() -> Self { Self { next_seq: 1, pending: Vec::new() } }
}

impl OfflineJournal {
    pub const MAX_PENDING: usize = 64;
    pub const MAX_BYTES: usize = 256 * 1024;

    pub fn enqueue(&mut self, cert: &LeafCertificate, leaf: &SigningKey, target: String, payload: Vec<u8>) -> Result<&SignedPublish, LinkError> {
        if self.pending.len() >= Self::MAX_PENDING || payload.len() > 16 * 1024
            || self.pending.iter().map(|f| f.payload.len()).sum::<usize>().saturating_add(payload.len()) > Self::MAX_BYTES {
            return Err(LinkError::JournalFull);
        }
        let seq = self.next_seq;
        let frame = SignedPublish::sign(cert.clone(), leaf, seq, target, payload)?;
        self.next_seq = seq.checked_add(1).ok_or(LinkError::InvalidSequence)?;
        self.pending.push(frame);
        Ok(self.pending.last().expect("pushed"))
    }

    pub fn oldest(&self) -> Option<&SignedPublish> { self.pending.first() }

    pub fn acknowledge(&mut self, ack: &PublishAck) -> Result<(), LinkError> {
        let Some(first) = self.pending.first() else { return Err(LinkError::WrongAckOrder); };
        if first.seq != ack.seq { return Err(LinkError::WrongAckOrder); }
        ack.verify(&first.cert.mesh_pubkey, first)?;
        self.pending.remove(0);
        Ok(())
    }

    pub fn validate(&self, cert: &LeafCertificate) -> Result<(), LinkError> {
        if self.next_seq == 0 || self.pending.len() > Self::MAX_PENDING { return Err(LinkError::JournalFull); }
        let mut prior = None;
        let mut total = 0usize;
        for frame in &self.pending {
            if &frame.cert != cert || frame.seq >= self.next_seq || prior.is_some_and(|p| frame.seq <= p) {
                return Err(LinkError::InvalidSequence);
            }
            frame.verify(&cert.parent_pubkey, cert.issued_at)?;
            total = total.saturating_add(frame.payload.len());
            if frame.payload.len() > 16 * 1024 || total > Self::MAX_BYTES { return Err(LinkError::JournalFull); }
            prior = Some(frame.seq);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    #[test]
    fn parent_scope_requires_canonical_user_and_project_address() {
        let user = "a".repeat(32);
        assert_eq!(parse_parent_scope(&alloc::format!("user:{user}")), Ok((user.as_str(), None)));
        let project = "01HZY7Y9M3MZQ5RSY3MTEP6F3C";
        assert_eq!(parse_parent_scope(&alloc::format!("project:{user}:{project}")), Ok((user.as_str(), Some(project))));
        assert_eq!(parse_parent_scope("project:other"), Err(LinkError::InvalidScope));
        assert_eq!(parse_parent_scope(&alloc::format!("user:{}", "A".repeat(32))), Err(LinkError::InvalidScope));
    }
    fn keys() -> (SigningKey, SigningKey) { (SigningKey::from_bytes(&[7; 32]), SigningKey::from_bytes(&[9; 32])) }
    #[test]
    fn certified_publish_and_ack_bind_every_field() {
        let (parent, leaf) = keys();
        let cert = LeafCertificate::issue(&parent, "user:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(), parent.verifying_key().to_bytes(), leaf.verifying_key().to_bytes(), 3, 100, 200, vec!["substrate.publish".into()]).unwrap();
        let frame = SignedPublish::sign(cert, &leaf, 1, alloc::format!("substrate/{}/sensor/touch", node_id(&leaf.verifying_key().to_bytes())), vec![1,2,3]).unwrap();
        frame.verify(&parent.verifying_key().to_bytes(), 150).unwrap();
        let ack = PublishAck::sign(&parent, &frame);
        ack.verify(&parent.verifying_key().to_bytes(), &frame).unwrap();
        let mut bad = frame.clone(); bad.payload[0] ^= 1;
        assert_eq!(bad.verify(&parent.verifying_key().to_bytes(), 150), Err(LinkError::BadSignature));
        assert!(ack.verify(&parent.verifying_key().to_bytes(), &bad).is_err());
        let mut bad = frame.clone(); bad.target.push('x');
        assert!(bad.verify(&parent.verifying_key().to_bytes(), 150).is_err());
        assert_eq!(frame.verify(&parent.verifying_key().to_bytes(), 200), Err(LinkError::Expired));
        assert_eq!(frame.verify(&leaf.verifying_key().to_bytes(), 150), Err(LinkError::WrongParent));
        let mut wrong_scope = frame.clone(); wrong_scope.cert.parent_scope = "user:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
        assert_eq!(wrong_scope.verify(&parent.verifying_key().to_bytes(), 150), Err(LinkError::BadSignature));
        let mut wrong_path = frame.clone(); wrong_path.target = alloc::format!("substrate/{}/../x", frame.cert.leaf_id());
        assert_eq!(wrong_path.verify(&parent.verifying_key().to_bytes(), 150), Err(LinkError::InvalidTarget));
        let mut no_cap = frame.clone(); no_cap.cert.capabilities.clear();
        assert_eq!(no_cap.verify(&parent.verifying_key().to_bytes(), 150), Err(LinkError::BadSignature));
        let no_cap_cert = LeafCertificate::issue(&parent, "user:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(), parent.verifying_key().to_bytes(), leaf.verifying_key().to_bytes(), 4, 100, 200, vec![]).unwrap();
        assert_eq!(SignedPublish::sign(no_cap_cert, &leaf, 1, frame.target.clone(), vec![1]), Err(LinkError::MissingCapability));
    }
    #[test]
    fn discovery_requires_pin_scope_and_freshness() {
        let (parent, leaf) = keys();
        let ad = ParentAdvertisement::sign(&parent, "user:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(), "192.0.2.1:9489".into(), 120, [1;16]);
        ad.verify(&parent.verifying_key().to_bytes(), "user:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 100).unwrap();
        assert!(ad.verify(&leaf.verifying_key().to_bytes(), "user:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 100).is_err());
        assert_eq!(ad.verify(&parent.verifying_key().to_bytes(), "user:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", 100), Err(LinkError::WrongParent));
        assert_eq!(ad.verify(&parent.verifying_key().to_bytes(), "user:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 120), Err(LinkError::Expired));
    }
    #[test]
    fn offline_queue_preserves_sequences_and_accepts_only_signed_oldest_ack() {
        let (parent, leaf) = keys();
        let cert = LeafCertificate::issue(&parent, "user:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(), parent.verifying_key().to_bytes(), leaf.verifying_key().to_bytes(), 1, 1, 1000, vec!["input.publish".into()]).unwrap();
        let target = alloc::format!("mesh.leaf.{}.input", cert.leaf_id());
        let mut q = OfflineJournal::default();
        let first = q.enqueue(&cert, &leaf, target.clone(), vec![1]).unwrap().clone();
        q.enqueue(&cert, &leaf, target, vec![2]).unwrap();
        let bytes = crate::encode(&q).unwrap();
        let mut restored: OfflineJournal = crate::decode(&bytes).unwrap();
        restored.validate(&cert).unwrap();
        let second_ack = PublishAck::sign(&parent, &restored.pending[1]);
        assert_eq!(restored.acknowledge(&second_ack), Err(LinkError::WrongAckOrder));
        let first_ack = PublishAck::sign(&parent, &first);
        restored.acknowledge(&first_ack).unwrap();
        assert_eq!(restored.oldest().unwrap().seq, 2);
        assert_eq!(restored.next_seq, 3);
    }
}
