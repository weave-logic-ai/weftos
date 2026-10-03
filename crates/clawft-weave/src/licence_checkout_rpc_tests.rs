//! `workload.cog.checkout | approve | status` against the boot-owned stores,
//! a real licence exchange on an in-process mesh runtime, and a cog mesh
//! whose link to the steward is down. Every test pins its own runtime dir.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use clawft_kernel::artifact_store::ArtifactStore;
use clawft_kernel::chain::ChainManager;
use clawft_kernel::error::{KernelError, KernelResult};
use clawft_kernel::ipc::KernelMessage;
use clawft_kernel::licence::{
    AdmissionPosture, Approval, ApprovalStore, BindState, BindingRecord, CheckoutGrant, CtxAdmission,
    GrantArtifact, LicenceExchange, LicenceExchangeConfig, LicenceExchangeParts, LicenceRef, NoExtraChecks,
    NoopSink, sha256_hex, sign_approval, sign_binding, sign_grant,
};
use clawft_kernel::mesh_artifact::{ArtifactExchange, ExchangeConfig};
use clawft_kernel::mesh_artifact_tunnel::PeerSender;
use clawft_kernel::mesh_runtime::MeshRuntime;
use clawft_kernel::revocation::RevocationList;
use clawft_kernel::workload_pkg::codec::hex_encode;
use clawft_kernel::workload_pkg::{KeyOrigin, TrustAnchors};
use clawft_types::config::{MeshAdmissionMode, MeshConfig};
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};

use super::*;
use crate::licence_boot::{InitArgs, build};

const PIN: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
const NONCE: &str = "bb00000000000000000000000000000000000000000000000000000000000002";
const OTHER_NONCE: &str = "cc00000000000000000000000000000000000000000000000000000000000003";
const BIN: &[u8] = b"cog-binary";

fn sk(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}
fn pk(k: &SigningKey) -> String {
    hex_encode(&k.verifying_key().to_bytes())
}
fn now() -> u64 {
    chrono::Utc::now().timestamp() as u64
}
fn posture() -> AdmissionPosture {
    AdmissionPosture { enforce: true, verdict_source_bound: true, open_membership: false }
}

fn anchors() -> TrustAnchors {
    let mut a = TrustAnchors::default();
    a.push_signer("op", &pk(&sk(1)), KeyOrigin::Operator).unwrap();
    a
}

fn boot(dir: &Path, chain: &Arc<ChainManager>, nonce: &str) -> LicenceRuntime {
    let mesh = MeshConfig {
        enabled: true,
        admission: MeshAdmissionMode::Enforce,
        genesis_hash: Some(PIN.into()),
        mesh_nonce: Some(nonce.into()),
        ..MeshConfig::default()
    };
    build(InitArgs {
        dir,
        anchors: anchors(),
        revocations: Arc::new(RevocationList::new(dir.join("revoked.json"))),
        chain: chain.clone(),
        mesh: Some(&mesh),
        steward_node_id: "node-steward".into(),
        steward_pubkey: pk(&sk(21)),
    })
}

fn exchange(dir: &Path, rt: &LicenceRuntime) -> Arc<LicenceExchange> {
    let a = Arc::new(anchors());
    LicenceExchange::start(LicenceExchangeParts {
        store: rt.store().clone(),
        approvals: Arc::new(ApprovalStore::open_or_poisoned(&dir.join("licence"), a.clone(), rt.local.clone())),
        anchors: a,
        runtime: Arc::new(MeshRuntime::new("node-steward".into())),
        posture: Arc::new(posture),
        admission: Arc::new(CtxAdmission),
        sink: Arc::new(NoopSink),
        config: LicenceExchangeConfig { sync_on_connect: false, ..Default::default() },
    })
}

fn bind(rt: &LicenceRuntime, steward: &str) {
    let rec = BindingRecord {
        v: 2,
        device_id: "seed-1".into(),
        device_pubkey: pk(&sk(20)),
        mesh_id: rt.local.get().unwrap().to_hex(),
        grant_pubkey: pk(&sk(2)),
        steward_node_id: steward.into(),
        steward_pubkey: pk(&sk(21)),
        state: BindState::Bound,
        seq: 1,
        bound_at: now(),
    };
    rt.store().accept_binding(&sign_binding(&rec, &sk(1)).unwrap(), posture(), &NoExtraChecks).unwrap();
}

fn grant(rt: &LicenceRuntime) {
    let t = now();
    let g = CheckoutGrant {
        v: 1,
        grant_id: String::new(),
        mesh_id: rt.local.get().unwrap().to_hex(),
        seed_device_id: "seed-1".into(),
        grant_key_id: String::new(),
        source: "cognitum".into(),
        registry: "registry.example".into(),
        cog_id: "fall-detect".into(),
        version: "1.2.0".into(),
        artifacts: vec![GrantArtifact {
            arch: "aarch64".into(),
            size: BIN.len() as u64,
            sha256: sha256_hex(BIN),
            blake3: hex_encode(blake3::hash(BIN).as_bytes()),
        }],
        manifest_sha256: sha256_hex(b"manifest"),
        licence: LicenceRef { ref_sha256: sha256_hex(b"licence"), expires: t + 30 * 86_400 },
        seq: 1,
        issued_at: t,
        expires_at: t + 72 * 3600,
    };
    rt.store().accept_grant(&sign_grant(&g, &sk(2)).unwrap()).unwrap();
}

/// A link to the steward that is down.
struct Down;
#[async_trait]
impl PeerSender for Down {
    async fn send_to_node(&self, _: &str, _: KernelMessage) -> KernelResult<()> {
        Err(KernelError::Mesh("the mesh service link is not up".into()))
    }
}

fn cog_mesh(rt: &LicenceRuntime) -> Arc<CogMesh> {
    let ex = Arc::new(ArtifactExchange::new("node-steward", Arc::new(ArtifactStore::new_memory()), ExchangeConfig::default()).unwrap());
    CogMesh::new(ex, rt.store().clone(), Arc::new(Down), None)
}

fn never(_: &str) -> bool {
    false
}

async fn call(
    rt: &LicenceRuntime,
    mesh: Option<Arc<CogMesh>>,
    ex: Option<Arc<LicenceExchange>>,
    m: &str,
    params: Value,
) -> Response {
    let ctx = Ctx { rt, mesh, exchange: ex, relay: None, reachable: &never, arch: Some("aarch64"), now: now(), principal: "operator", renewer: None };
    route(&ctx, m, params).await
}

fn ok(r: Response) -> Value {
    assert!(r.ok, "{:?}", r.error);
    r.result.unwrap()
}

fn err(r: Response) -> String {
    assert!(!r.ok, "expected an error, got {:?}", r.result);
    r.error.unwrap_or_default()
}

fn kinds(chain: &ChainManager) -> Vec<String> {
    chain.tail(chain.len()).into_iter().map(|e| e.kind).collect()
}

#[tokio::test]
async fn approve_names_the_grants_hashes_then_verifies_stores_and_chains_the_signed_approval() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rt = boot(dir.path(), &chain, NONCE);
    let ex = exchange(dir.path(), &rt);
    bind(&rt, "node-steward");
    let e = err(call(&rt, None, Some(ex.clone()), "workload.cog.checkout.approve",
        json!({"cog_id": "fall-detect", "version": "1.2.0", "prepare": true})).await);
    assert!(e.contains("no grant is held"), "{e}");
    grant(&rt);
    let prep = ok(call(&rt, None, Some(ex.clone()), "workload.cog.checkout.approve",
        json!({"cog_id": "fall-detect", "version": "1.2.0", "prepare": true})).await);
    assert_eq!(prep["sha256"], json!([sha256_hex(BIN)]));
    let pinned = Approval { v: 1, mesh_id: rt.local.get().unwrap().to_hex(), cog_id: "fall-detect".into(),
        version: "1.2.0".into(), sha256: vec![sha256_hex(BIN)], approved_at: 12345 };
    assert_eq!(prep["content_key"], pinned.content_key().as_str());
    assert_eq!(prep["mesh_id"], rt.local.get().unwrap().to_hex().as_str());

    // Status before: the run gate says no_approval, with the remedy.
    let st = ok(call(&rt, None, Some(ex.clone()), "workload.cog.checkout.status", json!({})).await);
    let gate = &st["grants"][0]["run_gate"][0]["run_gate"];
    assert_eq!(gate["verdict"], "no_approval");
    assert!(gate["remedy"].as_str().unwrap().contains("checkout approve fall-detect@1.2.0"));
    assert_eq!((st["is_steward"].as_bool(), st["steward_reachable"].as_bool()), (Some(true), Some(false)));

    // The CLI signs what prepare named.
    let a = Approval {
        v: 1,
        mesh_id: prep["mesh_id"].as_str().unwrap().into(),
        cog_id: "fall-detect".into(),
        version: "1.2.0".into(),
        sha256: vec![sha256_hex(BIN)],
        approved_at: now(),
    };
    let done = ok(call(&rt, None, Some(ex.clone()), "workload.cog.checkout.approve",
        json!({"signed": [sign_approval(&a, &sk(1)).unwrap()]})).await);
    assert_eq!(done["approved"][0]["approval_id"], a.content_key().as_str());
    assert!(kinds(&chain).contains(&"cog.checkout.approved".to_string()));
    let st = ok(call(&rt, None, Some(ex.clone()), "workload.cog.checkout.status", json!({})).await);
    assert_eq!(st["grants"][0]["run_gate"][0]["run_gate"]["verdict"], "permit");
    assert_eq!(st["approvals"][0]["active"], true);

    // Not an operator key: refused, nothing stored.
    let rogue = sign_approval(&Approval { sha256: vec!["ab".repeat(32)], ..a.clone() }, &sk(9)).unwrap();
    let e = err(call(&rt, None, Some(ex.clone()), "workload.cog.checkout.approve", json!({"signed": [rogue]})).await);
    assert!(e.contains("approval refused"), "{e}");
    assert_eq!(ex.approvals().len(), 1);
}

#[tokio::test]
async fn reapprove_orphaned_lists_what_a_nonce_change_orphaned() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rt = boot(dir.path(), &chain, NONCE);
    let ex = exchange(dir.path(), &rt);
    let a = Approval {
        v: 1,
        mesh_id: rt.local.get().unwrap().to_hex(),
        cog_id: "fall-detect".into(),
        version: "1.2.0".into(),
        sha256: vec![sha256_hex(BIN)],
        approved_at: now(),
    };
    ok(call(&rt, None, Some(ex.clone()), "workload.cog.checkout.approve", json!({"signed": [sign_approval(&a, &sk(1)).unwrap()]})).await);
    drop(ex);
    // The operator changes the nonce: same dir, new mesh id.
    let rt2 = boot(dir.path(), &chain, OTHER_NONCE);
    let ex2 = exchange(dir.path(), &rt2);
    let prep = ok(call(&rt2, None, Some(ex2.clone()), "workload.cog.checkout.approve",
        json!({"reapprove_orphaned": true, "prepare": true})).await);
    let signed: Vec<clawft_kernel::licence::SignedApproval> = serde_json::from_value(prep["orphaned_signed"].clone()).unwrap();
    assert_eq!(signed.len(), 1);
    assert!(signed[0].payload.contains("fall-detect"));
    assert_eq!(prep["mesh_id"], rt2.local.get().unwrap().to_hex().as_str());
    let mut renewed = a.clone();
    renewed.mesh_id = rt2.local.get().unwrap().to_hex();
    ok(call(&rt2, None, Some(ex2.clone()), "workload.cog.checkout.approve",
        json!({"signed": [sign_approval(&renewed, &sk(1)).unwrap()]})).await);
    assert!(ex2.approvals().orphaned_approvals().len() == 1, "the old one stays, orphaned");
    assert_eq!(ex2.approvals().rows().iter().filter(|r| r.active).count(), 1);
}

#[tokio::test]
async fn checkout_needs_a_binding_and_a_cog_mesh_and_chains_a_member_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rt = boot(dir.path(), &chain, NONCE);
    let p = json!({"cog_id": "fall-detect", "version": "latest"});
    assert!(err(call(&rt, None, None, "workload.cog.checkout", p.clone()).await).contains("no Seed binding"));
    bind(&rt, "node-other");
    assert!(err(call(&rt, None, None, "workload.cog.checkout", p.clone()).await).contains("placement has not started"));
    // A member whose steward is unreachable: refused with a stable code, chained.
    let e = err(call(&rt, Some(cog_mesh(&rt)), None, "workload.cog.checkout", p.clone()).await);
    assert!(e.starts_with("[licence_unreachable]"), "{e}");
    let k = kinds(&chain);
    assert!(k.contains(&"cog.checkout.request".to_string()) && k.contains(&"cog.checkout.refused".to_string()), "{k:?}");
    let bad = json!({"cog_id": "fall detect", "version": "1"});
    assert!(err(call(&rt, Some(cog_mesh(&rt)), None, "workload.cog.checkout", bad).await).contains("valid token"));
}

#[tokio::test]
async fn the_bound_steward_without_a_relay_says_how_to_configure_one() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rt = boot(dir.path(), &chain, NONCE);
    bind(&rt, "node-steward");
    let e = err(call(&rt, Some(cog_mesh(&rt)), None, "workload.cog.checkout",
        json!({"cog_id": "fall-detect", "version": "1.2.0", "arch": "aarch64"})).await);
    assert!(e.contains(crate::licence_steward::LINK_FILE), "{e}");
}
