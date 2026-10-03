//! `workload.node.bind | unbind | binding` and the licence boot, against a
//! stub Seed. Every test pins its own runtime dir (a tempdir) and never
//! contacts a real Seed.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use clawft_kernel::chain::ChainManager;
use clawft_kernel::licence::{
    BindState, BindingRecord, MeshId, SignedBinding, key_id, sign_binding,
};
use clawft_kernel::revocation::RevocationList;
use clawft_kernel::workload_pkg::codec::hex_encode;
use clawft_kernel::workload_pkg::{KeyOrigin, TrustAnchors};
use clawft_kernel::workload_runtime::seed_http::{Method, SeedCredentials, SeedTransport};
use clawft_kernel::workload_runtime::{
    LinkSecurity, RuntimeError, SeedApiRuntime, SeedConfig, SeedPin,
};
use clawft_types::config::{MeshAdmissionMode, MeshConfig};
use clawft_types::secret::SecretString;
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};

use super::*;
use crate::capability::{CallerCapabilities, Capability, required_capability};
use crate::licence_boot::{InitArgs, LicenceRuntime, build};

const NOW: u64 = 1_790_000_000;
const DEVICE: &str = "seed-dev-1";
const PIN: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
const NONCE_A: &str = "bb00000000000000000000000000000000000000000000000000000000000002";
const NONCE_B: &str = "cc00000000000000000000000000000000000000000000000000000000000003";

fn sk(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}
fn pk(k: &SigningKey) -> String {
    hex_encode(&k.verifying_key().to_bytes())
}

/// A Seed that answers `GET /api/v1/identity` and nothing else, over a link
/// that counts as pinned.
struct StubSeed;

#[async_trait]
impl SeedTransport for StubSeed {
    async fn request(
        &self,
        _m: Method,
        path: &str,
        _b: Option<&Value>,
        _t: &SecretString,
        _to: Duration,
    ) -> Result<(u16, Value), RuntimeError> {
        assert_eq!(path, "/api/v1/identity", "a bind reads only the identity");
        Ok((200, json!({"device_id": DEVICE, "public_key": pk(&sk(20))})))
    }
    fn link_security(&self) -> LinkSecurity {
        LinkSecurity::Pinned
    }
}

struct Token;
impl SeedCredentials for Token {
    fn get(&self, _: &str) -> Result<SecretString, RuntimeError> {
        Ok(SecretString::new("stub-token-0123456789abcdef".to_owned()))
    }
    fn put(&self, _: &str, _: SecretString) -> Result<(), RuntimeError> {
        Ok(())
    }
}

fn stub_runtime(node: &str) -> SeedApiRuntime {
    SeedApiRuntime::new(
        SeedConfig {
            node_id: node.into(),
            pins: vec![SeedPin::new("fall-detect", "1.0.0")],
            concurrency_cap: 2,
        },
        Arc::new(StubSeed),
        Arc::new(Token),
    )
    .unwrap()
}

fn mesh_cfg(nonce: Option<&str>) -> MeshConfig {
    MeshConfig {
        enabled: true,
        admission: MeshAdmissionMode::Enforce,
        genesis_hash: Some(PIN.into()),
        mesh_nonce: nonce.map(str::to_owned),
        ..MeshConfig::default()
    }
}

fn boot(dir: &Path, chain: &Arc<ChainManager>, mesh: &MeshConfig) -> LicenceRuntime {
    let mut anchors = TrustAnchors::default();
    anchors.push_signer("op", &pk(&sk(1)), KeyOrigin::Operator).unwrap();
    build(InitArgs {
        dir,
        anchors,
        revocations: Arc::new(RevocationList::new(dir.join("revoked.json"))),
        chain: chain.clone(),
        mesh: Some(mesh),
        steward_node_id: "node-steward".into(),
        steward_pubkey: pk(&sk(21)),
    })
}

fn mesh_of(nonce: &str) -> MeshId {
    MeshId::derive(&[0xaa, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
        &clawft_kernel::workload_pkg::codec::hex_decode_exact::<32>(nonce).unwrap())
}

fn posture() -> AdmissionPosture {
    licence_boot::posture(Some(&mesh_cfg(Some(NONCE_A))), true)
}

fn lookup(id: &str) -> Result<SeedApiRuntime, String> {
    if id == "seed-kitchen" { Ok(stub_runtime(id)) } else { Err(format!("no Seed {id:?}")) }
}

async fn call(rt: &LicenceRuntime, p: AdmissionPosture, m: &str, params: Value) -> Response {
    super::route(&Ctx { rt, posture: p, now: NOW, seed: &lookup }, m, params).await
}

fn record(mesh: &MeshId, seq: u64, state: BindState) -> BindingRecord {
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
        bound_at: NOW + seq, // strictly increasing: the replay memory refuses an equal one
    }
}

fn signed(r: &BindingRecord) -> SignedBinding {
    sign_binding(r, &sk(1)).unwrap()
}

fn fp() -> String {
    key_id(&sk(2).verifying_key().to_bytes())
}

fn bind_params(r: &BindingRecord) -> Value {
    json!({"seed_node_id": "seed-kitchen", "signed": signed(r), "grant_fingerprint": fp()})
}

fn chain_kinds(chain: &ChainManager) -> Vec<String> {
    chain.tail(chain.len()).into_iter().map(|e| e.kind).collect()
}

fn err_of(r: &Response) -> String {
    assert!(!r.ok, "expected an error, got {:?}", r.result);
    r.error.clone().unwrap_or_default()
}

#[test]
fn the_nonce_derives_the_mesh_id_at_boot_and_a_missing_one_leaves_it_unset() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rt = boot(dir.path(), &chain, &mesh_cfg(Some(NONCE_A)));
    assert_eq!(rt.local.get(), Some(mesh_of(NONCE_A)));
    assert!(rt.genesis_pinned && rt.nonce_set && rt.config_error.is_none());
    assert_eq!(licence_boot::status(&rt)["mesh_id"], mesh_of(NONCE_A).to_hex().as_str());

    let none = boot(dir.path(), &chain, &mesh_cfg(None));
    assert_eq!(none.local.get(), None, "no nonce: no mesh id, the path is inert");
    assert!(!none.nonce_set);
    assert!(licence_boot::status(&none)["mesh_id"].is_null());

    let bad = boot(dir.path(), &chain, &mesh_cfg(Some("not-hex")));
    assert_eq!(bad.local.get(), None);
    assert!(bad.config_error.as_deref().unwrap().contains("mesh_nonce"));
}

#[tokio::test]
async fn prepare_reads_the_seed_and_names_what_the_operator_signs() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rt = boot(dir.path(), &chain, &mesh_cfg(Some(NONCE_A)));
    let r = call(&rt, posture(), "workload.node.bind", json!({"seed_node_id": "seed-kitchen", "prepare": true})).await;
    assert!(r.ok, "{:?}", r.error);
    let v = r.result.unwrap();
    assert_eq!(v["device_id"], DEVICE);
    assert_eq!(v["mesh_id"], mesh_of(NONCE_A).to_hex().as_str());
    assert_eq!(v["steward_node_id"], "node-steward");
    assert_eq!(v["next_seq"], 1);
    // An unknown Seed is an error, not a panic.
    let e = call(&rt, posture(), "workload.node.bind", json!({"seed_node_id": "nope", "prepare": true})).await;
    assert!(err_of(&e).contains("no Seed"));
}

#[tokio::test]
async fn bind_persists_replay_state_and_a_restart_refuses_a_replay() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rt = boot(dir.path(), &chain, &mesh_cfg(Some(NONCE_A)));
    let rec = record(&mesh_of(NONCE_A), 1, BindState::Bound);
    let r = call(&rt, posture(), "workload.node.bind", bind_params(&rec)).await;
    assert!(r.ok, "{:?}", r.error);
    assert!(dir.path().join(clawft_kernel::workload_runtime::seed_bind::BIND_STATE_FILE).is_file());
    assert!(chain_kinds(&chain).contains(&"workload.node.bind".to_owned()));
    assert_eq!(rt.policy.store().active_binding(), Some(rec.clone()));
    let st = call(&rt, posture(), "workload.node.binding", json!({})).await.result.unwrap();
    assert_eq!(st["binding"]["state"], "bound");
    assert_eq!(st["next_seq"], 2);

    // The daemon restarts: the stored binding and the replay memory remain.
    let again = boot(dir.path(), &chain, &mesh_cfg(Some(NONCE_A)));
    assert_eq!(again.policy.store().active_binding(), Some(rec.clone()), "binding survives a restart");
    let e = call(&again, posture(), "workload.node.bind", bind_params(&rec)).await;
    assert!(err_of(&e).contains("seq_not_greater") || err_of(&e).contains("replayed"), "{e:?}");
}

#[tokio::test]
async fn a_second_mesh_is_refused_with_seed_bound_elsewhere() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rt = boot(dir.path(), &chain, &mesh_cfg(Some(NONCE_A)));
    let a = record(&mesh_of(NONCE_A), 1, BindState::Bound);
    assert!(call(&rt, posture(), "workload.node.bind", bind_params(&a)).await.ok);
    // The same Seed offered to another mesh id (the node now computes it).
    rt.local.set(Some(mesh_of(NONCE_B)));
    let b = record(&mesh_of(NONCE_B), 2, BindState::Bound);
    let e = call(&rt, posture(), "workload.node.bind", bind_params(&b)).await;
    assert!(err_of(&e).contains("seed_bound_elsewhere"), "{e:?}");
    // Unbind, then the bind goes through.
    let u = record(&mesh_of(NONCE_B), 2, BindState::Unbound);
    let r = call(&rt, posture(), "workload.node.unbind", json!({"signed": signed(&u)})).await;
    assert!(r.ok, "{:?}", r.error);
    assert!(chain_kinds(&chain).contains(&"workload.node.unbind".to_owned()));
    let b3 = record(&mesh_of(NONCE_B), 3, BindState::Bound);
    assert!(call(&rt, posture(), "workload.node.bind", bind_params(&b3)).await.ok);
}

#[tokio::test]
async fn a_changed_nonce_orphans_the_binding_chains_it_and_the_doctor_reports_it() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rt = boot(dir.path(), &chain, &mesh_cfg(Some(NONCE_A)));
    assert!(call(&rt, posture(), "workload.node.bind", bind_params(&record(&mesh_of(NONCE_A), 1, BindState::Bound))).await.ok);
    assert!(!chain_kinds(&chain).contains(&"binding_orphaned".to_owned()));

    // Restart with another nonce: orphaned at boot, no first use needed.
    let rt2 = boot(dir.path(), &chain, &mesh_cfg(Some(NONCE_B)));
    let kinds = chain_kinds(&chain);
    assert_eq!(kinds.iter().filter(|k| *k == "binding_orphaned").count(), 1, "{kinds:?}");
    assert!(rt2.policy.store().active_binding().is_none(), "checkout is off");
    let st = licence_boot::status(&rt2);
    assert_eq!(st["binding"]["orphaned"], true);
    let found = crate::licence_doctor::findings(&st);
    assert_eq!(found[0].id, "licence.binding_orphaned");

    // No nonce at all, with a binding held: the other doctor case.
    let rt3 = boot(dir.path(), &chain, &mesh_cfg(None));
    let found = crate::licence_doctor::findings(&licence_boot::status(&rt3));
    assert_eq!(found[0].id, "licence.mesh_id_unset");
}

#[tokio::test]
async fn bind_without_a_mesh_id_or_under_open_membership_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let none = boot(dir.path(), &chain, &mesh_cfg(None));
    let rec = record(&mesh_of(NONCE_A), 1, BindState::Bound);
    assert!(err_of(&call(&none, posture(), "workload.node.bind", bind_params(&rec)).await).contains("no local mesh id"));

    let rt = boot(dir.path(), &chain, &mesh_cfg(Some(NONCE_A)));
    let open = AdmissionPosture { open_membership: true, ..posture() };
    let e = call(&rt, open, "workload.node.bind", bind_params(&rec)).await;
    assert!(err_of(&e).contains("open_membership"), "{e:?}");
    assert!(rt.policy.store().held_binding().is_none());
}

#[tokio::test]
async fn a_wrong_fingerprint_or_bad_params_bind_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rt = boot(dir.path(), &chain, &mesh_cfg(Some(NONCE_A)));
    let rec = record(&mesh_of(NONCE_A), 1, BindState::Bound);
    let mut p = bind_params(&rec);
    p["grant_fingerprint"] = json!("ed25519:0000000000000000");
    assert!(err_of(&call(&rt, posture(), "workload.node.bind", p).await).contains("fingerprint_mismatch"));
    let e = call(&rt, posture(), "workload.node.bind", json!({"seed_node_id": "seed-kitchen"})).await;
    assert!(err_of(&e).contains("needs 'signed'"));
    let e = call(&rt, posture(), "workload.node.bind", json!({"seed_node_id": "x", "bogus": 1})).await;
    assert!(err_of(&e).contains("invalid"));
    assert!(rt.policy.store().held_binding().is_none());
}

#[tokio::test]
async fn reset_floor_is_chained_and_needs_a_binding_in_effect() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rt = boot(dir.path(), &chain, &mesh_cfg(Some(NONCE_A)));
    assert!(!call(&rt, posture(), "workload.node.reset-floor", json!({})).await.ok, "no binding yet");
    assert!(call(&rt, posture(), "workload.node.bind", bind_params(&record(&mesh_of(NONCE_A), 1, BindState::Bound))).await.ok);
    let r = call(&rt, posture(), "workload.node.reset-floor", json!({})).await;
    assert!(r.ok, "{:?}", r.error);
    assert!(chain_kinds(&chain).contains(&"floor_reset".to_owned()));
}

#[test]
fn the_three_methods_are_served_here_and_bind_and_unbind_are_admin_only() {
    for m in METHODS {
        assert!(handles(m), "{m}");
    }
    let (anon, write, admin) = (
        CallerCapabilities::anonymous(),
        CallerCapabilities::from_scopes(["write"]),
        CallerCapabilities::from_scopes(["admin"]),
    );
    for m in ["workload.node.bind", "workload.node.unbind", "workload.node.reset-floor"] {
        assert_eq!(required_capability(m), Capability::Admin, "{m}");
        assert!(!anon.allows_method(m) && !write.allows_method(m), "{m} must need admin");
        assert!(admin.allows_method(m), "{m}");
    }
    assert_eq!(required_capability("workload.node.binding"), Capability::Read);
    assert!(anon.allows_method("workload.node.binding"));
}
