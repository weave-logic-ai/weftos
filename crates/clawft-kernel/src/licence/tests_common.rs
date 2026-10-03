//! Shared fixtures for the licence tests.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ed25519_dalek::SigningKey;

use super::*;
use crate::workload_pkg::{KeyOrigin, TrustAnchors};

pub(super) const T0: u64 = 1_000_000;

pub(super) fn sk(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

pub(super) fn pk_hex(k: &SigningKey) -> String {
    hex_encode(&k.verifying_key().to_bytes())
}

pub(super) fn op() -> SigningKey {
    sk(1)
}

pub(super) fn grant_key() -> SigningKey {
    sk(2)
}

pub(super) fn mesh() -> MeshId {
    MeshId::derive(&[9; 32], &[7; 32])
}

pub(super) fn other_mesh() -> MeshId {
    MeshId::derive(&[9; 32], &[8; 32])
}

pub(super) fn anchors() -> Arc<TrustAnchors> {
    let mut a = TrustAnchors::default();
    a.push_signer("operator-1", &pk_hex(&op()), KeyOrigin::Operator).unwrap();
    // A pinned WeftOS key is not an operator key and must not sign these.
    a.push_signer("weftos-1", &pk_hex(&sk(5)), KeyOrigin::Weftos).unwrap();
    Arc::new(a)
}

pub(super) fn posture() -> AdmissionPosture {
    AdmissionPosture { enforce: true, verdict_source_bound: true, open_membership: false }
}

pub(super) fn binding_rec(seq: u64, state: BindState, gk: &SigningKey, m: &MeshId) -> BindingRecord {
    BindingRecord {
        v: 2,
        device_id: "seed-test".into(),
        device_pubkey: pk_hex(&sk(20)),
        mesh_id: m.to_hex(),
        grant_pubkey: pk_hex(gk),
        steward_node_id: "node-steward".into(),
        steward_pubkey: pk_hex(&sk(21)),
        state,
        seq,
        bound_at: T0,
    }
}

pub(super) fn binding(seq: u64, state: BindState) -> SignedBinding {
    sign_binding(&binding_rec(seq, state, &grant_key(), &mesh()), &op()).unwrap()
}

pub(super) fn sha_of(arch: &str) -> String {
    sha256_hex(format!("bin-{arch}").as_bytes())
}

pub(super) fn b3_of(arch: &str) -> String {
    hex_encode(blake3::hash(format!("bin-{arch}").as_bytes()).as_bytes())
}

pub(super) fn b3_bytes(arch: &str) -> [u8; 32] {
    *blake3::hash(format!("bin-{arch}").as_bytes()).as_bytes()
}

pub(super) fn grant_rec(seq: u64, issued: u64, ttl: u64, arches: &[&str]) -> CheckoutGrant {
    CheckoutGrant {
        v: 1,
        grant_id: String::new(),
        mesh_id: mesh().to_hex(),
        seed_device_id: "seed-test".into(),
        grant_key_id: String::new(),
        source: "cognitum".into(),
        registry: "registry.example".into(),
        cog_id: "fall-detect".into(),
        version: "1.2.0".into(),
        artifacts: arches
            .iter()
            .map(|a| GrantArtifact {
                arch: (*a).into(),
                size: 1024,
                sha256: sha_of(a),
                blake3: b3_of(a),
            })
            .collect(),
        manifest_sha256: sha256_hex(b"manifest"),
        licence: LicenceRef { ref_sha256: sha256_hex(b"licence"), expires: issued + 365 * 86400 },
        seq,
        issued_at: issued,
        expires_at: issued + ttl,
    }
}

pub(super) fn grant(seq: u64, issued: u64, ttl: u64, arches: &[&str]) -> SignedGrant {
    sign_grant(&grant_rec(seq, issued, ttl, arches), &grant_key()).unwrap()
}

pub(super) fn approval_rec(shas: &[String], m: &MeshId) -> Approval {
    Approval {
        v: 1,
        mesh_id: m.to_hex(),
        cog_id: "fall-detect".into(),
        version: "1.2.0".into(),
        sha256: shas.to_vec(),
        approved_at: T0,
    }
}

pub(super) fn approval(shas: &[String]) -> SignedApproval {
    sign_approval(&approval_rec(shas, &mesh()), &op()).unwrap()
}

/// A node's licence state in a temp dir, with a clock the test moves.
pub(super) struct Fx {
    pub dir: tempfile::TempDir,
    pub clock: Arc<AtomicU64>,
    pub local: LocalMeshId,
    pub sink: Arc<RecordingSink>,
    pub store: Arc<CheckoutGrantStore>,
    pub approvals: Arc<ApprovalStore>,
}

impl Fx {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(AtomicU64::new(T0));
        let local = LocalMeshId::new(mesh());
        let sink = Arc::new(RecordingSink::default());
        let (store, approvals) = open_in(dir.path(), &clock, &local, &sink);
        Self { dir, clock, local, sink, store, approvals }
    }

    pub fn set_now(&self, t: u64) {
        self.clock.store(t, Ordering::SeqCst);
    }

    /// Drop and reopen both stores from disk (a restart).
    pub fn restart(&mut self) {
        let (s, a) = open_in(self.dir.path(), &self.clock, &self.local, &self.sink);
        self.store = s;
        self.approvals = a;
    }

    pub fn bind(&self) {
        self.store.accept_binding(&binding(1, BindState::Bound), posture(), &NoExtraChecks).unwrap();
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.sink.events().iter().map(LicenceEvent::name).collect()
    }
}

pub(super) fn clock_of(c: &Arc<AtomicU64>) -> Clock {
    let c = c.clone();
    Arc::new(move || c.load(Ordering::SeqCst))
}

fn open_in(
    dir: &std::path::Path,
    clock: &Arc<AtomicU64>,
    local: &LocalMeshId,
    sink: &Arc<RecordingSink>,
) -> (Arc<CheckoutGrantStore>, Arc<ApprovalStore>) {
    let s = CheckoutGrantStore::open(dir, anchors(), local.clone(), clock_of(clock)).unwrap();
    s.set_sink(sink.clone());
    let a = ApprovalStore::open(dir, anchors(), local.clone()).unwrap();
    (Arc::new(s), Arc::new(a))
}
