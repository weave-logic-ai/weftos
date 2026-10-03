//! ADR-106 phase 1d: the steward profile over an operator-signed v2 binding,
//! against a stub Seed. Never contacts a real Seed.

use std::sync::Arc;

use ed25519_dalek::SigningKey;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::seed::{SEED_CONCURRENCY_CAP, SeedApiRuntime, SeedConfig, SeedPin};
use super::seed_bind::{BindError, SeedBinder, StewardBind, grant_fingerprint, seed_node_id};
use super::seed_http::HttpSeedTransport;
use super::seed_tls::SeedTls;
use super::test_support::MemoryCredentials;
use crate::chain::{
    ChainManager, EVENT_KIND_WORKLOAD_NODE_BIND, EVENT_KIND_WORKLOAD_NODE_UNBIND,
    EVENT_KIND_WORKLOAD_REFUSE,
};
use crate::licence::{
    AdmissionPosture, BindState, BindingRecord, CheckoutGrantStore, LicenceError, LocalMeshId,
    MeshId, SignedBinding, sign_binding,
};
use crate::workload_pkg::codec::hex_encode;
use crate::workload_pkg::{KeyOrigin, TrustAnchors};

const DEVICE: &str = "seed-dev-1";
const NOW: u64 = 1_790_000_000;

fn sk(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}
fn pk(k: &SigningKey) -> String {
    hex_encode(&k.verifying_key().to_bytes())
}
fn mesh_a() -> MeshId {
    MeshId::derive(&[9; 32], &[7; 32])
}
fn mesh_b() -> MeshId {
    MeshId::derive(&[9; 32], &[8; 32])
}
fn posture() -> AdmissionPosture {
    AdmissionPosture { enforce: true, verdict_source_bound: true, open_membership: false }
}

struct Rig {
    _server: MockServer,
    _dir: tempfile::TempDir,
    rt: SeedApiRuntime,
    store: CheckoutGrantStore,
    local: LocalMeshId,
    chain: Arc<ChainManager>,
    binder: SeedBinder,
    state_file: std::path::PathBuf,
    operator: SigningKey,
}

async fn rig_with(lab_link: bool) -> Rig {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/identity"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"device_id": DEVICE, "public_key": pk(&sk(20))}),
        ))
        .mount(&server)
        .await;
    let node = seed_node_id(&sk(61));
    let mut transport = HttpSeedTransport::new(&server.uri(), SeedTls::WebPki).unwrap();
    if lab_link {
        transport = transport.allow_unpinned_lab_link();
    }
    let rt = SeedApiRuntime::new(
        SeedConfig {
            node_id: node.clone(),
            pins: vec![SeedPin::new("fall-detect", "1.0.0")],
            concurrency_cap: SEED_CONCURRENCY_CAP,
        },
        Arc::new(transport),
        Arc::new(MemoryCredentials::with(&node, "seed-token-v2-0123456789abcdef")),
    )
    .unwrap();
    let operator = sk(1);
    let mut anchors = TrustAnchors::default();
    anchors.push_signer("op", &pk(&operator), KeyOrigin::Operator).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let local = LocalMeshId::new(mesh_a());
    let store = CheckoutGrantStore::open(
        &dir.path().join("licence"),
        Arc::new(anchors),
        local.clone(),
        Arc::new(|| NOW),
    )
    .unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let state_file = dir.path().join("binds.json");
    let binder = SeedBinder::new(vec![], chain.clone()).with_state_file(&state_file).unwrap();
    Rig { _server: server, _dir: dir, rt, store, local, chain, binder, state_file, operator }
}

async fn seed_rt(device: &str, key: &SigningKey) -> (MockServer, SeedApiRuntime) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/identity"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"device_id": device, "public_key": pk(key)})),
        )
        .mount(&server)
        .await;
    let node = format!("adapter-{device}");
    let rt = SeedApiRuntime::new(
        SeedConfig {
            node_id: node.clone(),
            pins: vec![SeedPin::new("fall-detect", "1.0.0")],
            concurrency_cap: SEED_CONCURRENCY_CAP,
        },
        Arc::new(
            HttpSeedTransport::new(&server.uri(), SeedTls::WebPki).unwrap().allow_unpinned_lab_link(),
        ),
        Arc::new(MemoryCredentials::with(&node, "seed-token-v2-0123456789abcdef")),
    )
    .unwrap();
    (server, rt)
}

async fn rig() -> Rig {
    rig_with(true).await
}

impl Rig {
    fn record(&self, mesh: &MeshId, seq: u64, state: BindState, bound_at: u64) -> BindingRecord {
        BindingRecord {
            v: 2,
            device_id: DEVICE.into(),
            device_pubkey: pk(&sk(20)),
            mesh_id: mesh.to_hex(),
            grant_pubkey: pk(&sk(2)),
            steward_node_id: "node-steward".into(),
            steward_pubkey: pk(&sk(21)),
            state,
            seq,
            bound_at,
        }
    }

    fn sign(&self, r: &BindingRecord) -> SignedBinding {
        sign_binding(r, &self.operator).unwrap()
    }

    fn fp(&self) -> String {
        grant_fingerprint(&pk(&sk(2))).unwrap()
    }

    async fn bind(&self, s: &SignedBinding, fp: &str, now: u64) -> Result<BindingRecord, BindError> {
        let steward_pk = pk(&sk(21));
        self.binder
            .bind_v2(&StewardBind {
                signed: s,
                rt: &self.rt,
                store: &self.store,
                posture: posture(),
                confirmed_fingerprint: fp,
                steward_node_id: "node-steward",
                steward_pubkey: &steward_pk,
                now,
            })
            .await
    }

    fn events(&self, kind: &str) -> Vec<serde_json::Value> {
        self.chain
            .tail(self.chain.len())
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.payload.unwrap_or_default())
            .collect()
    }

    fn refusals(&self) -> Vec<String> {
        self.events(EVENT_KIND_WORKLOAD_REFUSE)
            .iter()
            .map(|p| p["code"].as_str().unwrap_or("").to_owned())
            .collect()
    }
}

#[tokio::test]
async fn a_confirmed_record_that_matches_the_seed_binds_and_is_chained() {
    let r = rig().await;
    let rec = r.record(&mesh_a(), 1, BindState::Bound, NOW);
    let got = r.bind(&r.sign(&rec), &r.fp(), NOW).await.unwrap();
    assert_eq!(got, rec);
    assert_eq!(r.store.active_binding(), Some(rec.clone()));
    let ev = r.events(EVENT_KIND_WORKLOAD_NODE_BIND);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["mesh_id"], mesh_a().to_hex().as_str());
    assert_eq!(ev[0]["grant_fingerprint"], r.fp().as_str());
}

#[tokio::test]
async fn the_replay_state_is_persisted_and_survives_a_restart() {
    let r = rig().await;
    let s = r.sign(&r.record(&mesh_a(), 1, BindState::Bound, NOW));
    r.bind(&s, &r.fp(), NOW).await.unwrap();
    // A new binder over the same state file refuses the same record, and an
    // older one, even against a fresh store.
    let again = SeedBinder::new(vec![], r.chain.clone()).with_state_file(&r.state_file).unwrap();
    let steward_pk = pk(&sk(21));
    let fresh_dir = tempfile::tempdir().unwrap();
    let mut anchors = TrustAnchors::default();
    anchors.push_signer("op", &pk(&r.operator), KeyOrigin::Operator).unwrap();
    let fresh = CheckoutGrantStore::open(
        fresh_dir.path(),
        Arc::new(anchors),
        LocalMeshId::new(mesh_a()),
        Arc::new(|| NOW),
    )
    .unwrap();
    let e = again
        .bind_v2(&StewardBind {
            signed: &s,
            rt: &r.rt,
            store: &fresh,
            posture: posture(),
            confirmed_fingerprint: &r.fp(),
            steward_node_id: "node-steward",
            steward_pubkey: &steward_pk,
            now: NOW,
        })
        .await
        .unwrap_err();
    assert_eq!(e, BindError::Replayed);
    assert!(fresh.active_binding().is_none());
}

#[tokio::test]
async fn a_wrong_fingerprint_is_refused_and_nothing_is_stored() {
    let r = rig().await;
    let s = r.sign(&r.record(&mesh_a(), 1, BindState::Bound, NOW));
    let e = r.bind(&s, "ed25519:0000000000000000", NOW).await.unwrap_err();
    assert_eq!(e, BindError::Licence(LicenceError::CheckFailed("fingerprint_mismatch".into())));
    assert_eq!(r.refusals(), ["fingerprint_mismatch"]);
    assert!(r.store.held_binding().is_none());
    assert!(!r.state_file.exists(), "no replay state was spent");
}

#[tokio::test]
async fn the_seeds_live_identity_must_match() {
    let r = rig().await;
    let mut rec = r.record(&mesh_a(), 1, BindState::Bound, NOW);
    rec.device_pubkey = pk(&sk(77));
    let e = r.bind(&r.sign(&rec), &r.fp(), NOW).await.unwrap_err();
    assert_eq!(
        e,
        BindError::Licence(LicenceError::CheckFailed("identity_mismatch_device_key".into()))
    );
    assert_eq!(r.refusals(), ["identity_mismatch"]);
    assert!(r.store.held_binding().is_none());
    assert!(!r.state_file.exists());
}

#[tokio::test]
async fn the_age_window_is_600_seconds() {
    let r = rig().await;
    let old = r.sign(&r.record(&mesh_a(), 1, BindState::Bound, NOW - 601));
    let e = r.bind(&old, &r.fp(), NOW).await.unwrap_err();
    assert_eq!(e, BindError::Licence(LicenceError::CheckFailed("expired".into())));
    let edge = r.sign(&r.record(&mesh_a(), 1, BindState::Bound, NOW - 600));
    r.bind(&edge, &r.fp(), NOW).await.unwrap();
}

#[tokio::test]
async fn a_record_for_another_mesh_or_an_unpinned_signer_is_refused() {
    let r = rig().await;
    let other = r.sign(&r.record(&mesh_b(), 1, BindState::Bound, NOW));
    assert_eq!(
        r.bind(&other, &r.fp(), NOW).await.unwrap_err(),
        BindError::Licence(LicenceError::WrongMesh)
    );
    let stranger = sign_binding(&r.record(&mesh_a(), 1, BindState::Bound, NOW), &sk(44)).unwrap();
    assert_eq!(
        r.bind(&stranger, &r.fp(), NOW).await.unwrap_err(),
        BindError::Licence(LicenceError::UntrustedKey)
    );
    assert_eq!(r.refusals(), ["wrong_mesh", "untrusted_operator"]);
}

#[tokio::test]
async fn an_unpinned_link_and_open_membership_are_refused() {
    let r = rig_with(false).await;
    let s = r.sign(&r.record(&mesh_a(), 1, BindState::Bound, NOW));
    let e = r.bind(&s, &r.fp(), NOW).await.unwrap_err();
    assert!(matches!(e, BindError::UnpinnedTransport(_)), "{e:?}");

    let r = rig().await;
    let steward_pk = pk(&sk(21));
    let e = r
        .binder
        .bind_v2(&StewardBind {
            signed: &s,
            rt: &r.rt,
            store: &r.store,
            posture: AdmissionPosture { open_membership: true, ..posture() },
            confirmed_fingerprint: &r.fp(),
            steward_node_id: "node-steward",
            steward_pubkey: &steward_pk,
            now: NOW,
        })
        .await
        .unwrap_err();
    assert_eq!(e, BindError::Licence(LicenceError::BindingRefused("open_membership")));
    assert_eq!(r.refusals(), ["binding_refused"]);
}

#[tokio::test]
async fn seq_must_exceed_the_held_one() {
    let r = rig().await;
    r.bind(&r.sign(&r.record(&mesh_a(), 5, BindState::Bound, NOW)), &r.fp(), NOW).await.unwrap();
    let same = r.sign(&r.record(&mesh_a(), 5, BindState::Bound, NOW + 1));
    assert_eq!(
        r.bind(&same, &r.fp(), NOW + 1).await.unwrap_err(),
        BindError::Licence(LicenceError::CheckFailed("seq_not_greater".into()))
    );
    r.bind(&r.sign(&r.record(&mesh_a(), 6, BindState::Bound, NOW + 2)), &r.fp(), NOW + 2)
        .await
        .unwrap();
    assert_eq!(r.store.active_binding().unwrap().seq, 6);
}

#[tokio::test]
async fn a_second_mesh_is_refused_until_the_seed_is_unbound() {
    let r = rig().await;
    r.bind(&r.sign(&r.record(&mesh_a(), 1, BindState::Bound, NOW)), &r.fp(), NOW).await.unwrap();

    // The mesh nonce changes: this node now computes another mesh id.
    r.local.set(Some(mesh_b()));
    let second = r.sign(&r.record(&mesh_b(), 2, BindState::Bound, NOW + 1));
    assert_eq!(
        r.bind(&second, &r.fp(), NOW + 1).await.unwrap_err(),
        BindError::SeedBoundElsewhere { device_id: DEVICE.into() }
    );
    assert_eq!(r.refusals(), ["seed_bound_elsewhere"]);
    assert_eq!(r.store.held_binding().unwrap().mesh_id, mesh_a().to_hex(), "unchanged");

    // Unbind (an operator-signed record, no Seed needed), then bind again.
    let unbind = r.sign(&r.record(&mesh_b(), 2, BindState::Unbound, NOW + 2));
    r.binder.unbind_v2(&unbind, &r.store).unwrap();
    assert_eq!(r.events(EVENT_KIND_WORKLOAD_NODE_UNBIND).len(), 1);
    assert!(r.store.active_binding().is_none(), "unbound: grants stop at once");
    let again = r.sign(&r.record(&mesh_b(), 3, BindState::Bound, NOW + 3));
    r.bind(&again, &r.fp(), NOW + 3).await.unwrap();
    assert_eq!(r.store.active_binding().unwrap().mesh_id, mesh_b().to_hex());
}

#[tokio::test]
async fn unbind_needs_a_held_binding_the_same_device_and_a_higher_seq() {
    let r = rig().await;
    let u = r.sign(&r.record(&mesh_a(), 1, BindState::Unbound, NOW));
    assert_eq!(r.binder.unbind_v2(&u, &r.store).unwrap_err(), BindError::Licence(LicenceError::NoBinding));
    r.bind(&r.sign(&r.record(&mesh_a(), 1, BindState::Bound, NOW)), &r.fp(), NOW).await.unwrap();
    let low = r.sign(&r.record(&mesh_a(), 1, BindState::Unbound, NOW));
    assert_eq!(
        r.binder.unbind_v2(&low, &r.store).unwrap_err(),
        BindError::Licence(LicenceError::CheckFailed("seq_not_greater".into()))
    );
    let mut other = r.record(&mesh_a(), 2, BindState::Unbound, NOW);
    other.device_id = "other-seed".into();
    assert_eq!(
        r.binder.unbind_v2(&r.sign(&other), &r.store).unwrap_err(),
        BindError::Licence(LicenceError::CheckFailed("device_mismatch".into()))
    );
    let bound_rec = r.sign(&r.record(&mesh_a(), 2, BindState::Bound, NOW));
    assert_eq!(
        r.binder.unbind_v2(&bound_rec, &r.store).unwrap_err(),
        BindError::Licence(LicenceError::CheckFailed("not_an_unbind".into()))
    );
    assert!(r.store.active_binding().is_some(), "refused unbinds changed nothing");
}

#[tokio::test]
async fn bind_x_then_y_then_a_nonce_change_then_x_is_not_locked_out() {
    let r = rig().await;
    let (_srv, rt_y) = seed_rt("seed-dev-2", &sk(30)).await;
    r.bind(&r.sign(&r.record(&mesh_a(), 1, BindState::Bound, NOW + 1)), &r.fp(), NOW + 1).await.unwrap();
    // Y replaces X as the mesh's Seed (one Seed per mesh, rebind with a higher seq).
    let mut y = r.record(&mesh_a(), 2, BindState::Bound, NOW + 2);
    y.device_id = "seed-dev-2".into();
    y.device_pubkey = pk(&sk(30));
    let steward_pk = pk(&sk(21));
    r.binder
        .bind_v2(&StewardBind {
            signed: &r.sign(&y),
            rt: &rt_y,
            store: &r.store,
            posture: posture(),
            confirmed_fingerprint: &r.fp(),
            steward_node_id: "node-steward",
            steward_pubkey: &steward_pk,
            now: NOW + 2,
        })
        .await
        .unwrap();
    // The nonce changes, then X is bound again: X is no longer held, so it is
    // not "bound elsewhere" (a copy of the old mesh kept in the binder would
    // have refused it for good).
    r.local.set(Some(mesh_b()));
    let x = r.sign(&r.record(&mesh_b(), 3, BindState::Bound, NOW + 3));
    r.bind(&x, &r.fp(), NOW + 3).await.unwrap();
    assert_eq!(r.store.active_binding().unwrap().device_id, DEVICE);
}

#[tokio::test]
async fn an_unbind_signed_by_a_non_anchor_key_is_refused() {
    let r = rig().await;
    r.bind(&r.sign(&r.record(&mesh_a(), 1, BindState::Bound, NOW)), &r.fp(), NOW).await.unwrap();
    let rogue = sign_binding(&r.record(&mesh_a(), 2, BindState::Unbound, NOW), &sk(44)).unwrap();
    assert_eq!(
        r.binder.unbind_v2(&rogue, &r.store).unwrap_err(),
        BindError::Licence(LicenceError::UntrustedKey)
    );
    assert!(r.store.active_binding().is_some(), "still bound");
    assert!(r.events(EVENT_KIND_WORKLOAD_NODE_UNBIND).is_empty());
    assert_eq!(r.refusals(), ["untrusted_operator"]);
}

#[tokio::test]
async fn an_unbind_whose_save_fails_is_applied_chained_and_flagged_pending() {
    use std::os::unix::fs::PermissionsExt;
    let r = rig().await;
    r.bind(&r.sign(&r.record(&mesh_a(), 1, BindState::Bound, NOW)), &r.fp(), NOW).await.unwrap();
    let dir = r._dir.path().join("licence");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let out = r.binder.unbind_v2(&r.sign(&r.record(&mesh_a(), 2, BindState::Unbound, NOW)), &r.store);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let out = out.unwrap();
    assert!(out.save_pending);
    assert!(r.store.active_binding().is_none(), "applied in memory: grants stop");
    let ev = r.events(EVENT_KIND_WORKLOAD_NODE_UNBIND);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["save_pending"], true);
}
