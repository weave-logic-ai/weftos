//! The ADR-106 run gate in the `workload-host` (phase 3): a Cognitum-origin
//! cog is installed and started only when the gate permits, the refusal
//! reason reaches `--explain` and the chain, and other packages never reach
//! the gate.

use std::sync::{Arc, Mutex};

use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;

use super::host_licence::EVENT_KIND_RUN_PERMIT;
use super::host_service::CtlConfig;
use super::msg::method;
use super::plane::PlacementControlPlane;
use super::plane_place::PlaceOrder;
use super::test_support::*;
use super::transport::MeshConnector;
use crate::artifact_store::ArtifactStore;
use crate::chain::{ChainManager, EVENT_KIND_WORKLOAD_REFUSE};
use crate::licence::{CognitumRunGate, RunPermit, RunRefusal, RunRequest, RunVerdict, sha256_hex};
use crate::mesh_artifact::{ArtifactExchange, ExchangeConfig};
use crate::mesh_swarm_state::{Audience, GrantInfo, RedistributionPolicy};
use crate::node_registry::node_id_from_pubkey;
use crate::workload_pkg::codec::hex_encode;
use crate::workload_runtime::RunMode;

const SCRIPT: &str = "#!/bin/sh\nexec sleep 30\n";
const COGNITUM_URL: &str = "https://cognitum.one/releases/fall-detect";

/// The swarm policy is not under test here: the controller hands its bytes over.
#[derive(Debug)]
struct ServeAll;
impl RedistributionPolicy for ServeAll {
    fn allows(&self, _: &[u8; 32], _: &[GrantInfo], _: &Audience<'_>) -> bool {
        true
    }
}

/// A gate whose answer the test sets, recording what it was asked.
struct TestGate {
    answer: Mutex<Result<RunVerdict, RunRefusal>>,
    asked: Mutex<Vec<(String, String, String, String)>>,
}

impl TestGate {
    fn new(answer: Result<RunVerdict, RunRefusal>) -> Arc<Self> {
        Arc::new(Self { answer: Mutex::new(answer), asked: Mutex::default() })
    }
    fn set(&self, a: Result<RunVerdict, RunRefusal>) {
        *self.answer.lock().unwrap() = a;
    }
}

impl CognitumRunGate for TestGate {
    fn check(&self, r: &RunRequest<'_>) -> Result<RunVerdict, RunRefusal> {
        self.asked.lock().unwrap().push((r.cog_id.into(), r.version.into(), r.sha256.into(), r.blake3.into()));
        self.answer.lock().unwrap().clone()
    }
}

fn permit() -> Result<RunVerdict, RunRefusal> {
    Ok(RunVerdict::Permit(RunPermit { grant_id: "g".repeat(64), approval_id: "a".repeat(64), blake3: "b".repeat(64) }))
}

fn order(pkg: &std::path::Path) -> PlaceOrder {
    PlaceOrder {
        package_dir: pkg.to_path_buf(),
        config: CtlConfig { mode: RunMode::Listener, args: vec![], csi_port: 15016 },
        pin: None,
        prefer: vec![],
        avoid: vec![],
        allow_emulated: false,
        start: true,
        dry_run: false,
        project_id: None,
    }
}

struct Rig {
    plane: PlacementControlPlane,
    host: HostNode,
    gate: Arc<TestGate>,
    _tmp: tempfile::TempDir,
    pkg: std::path::PathBuf,
}

async fn rig(release_url: Option<&str>, answer: Result<RunVerdict, RunRefusal>) -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package_from(tmp.path(), "fall-detect", SCRIPT, &[arch()], release_url);
    let key = SigningKey::from_bytes(&[30; 32]);
    let host = host_node(31, board_caps("pi5"), true, &key);
    let gate = TestGate::new(answer);
    assert!(host.svc.set_licence_gate(gate.clone()));
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("pi", host.svc.clone());
    let chain = Arc::new(ChainManager::new(0, 1000));
    let id = node_id_from_pubkey(&key.verifying_key().to_bytes());
    let cfg = ExchangeConfig { redistribution: Arc::new(ServeAll), ..ExchangeConfig::default() };
    let mut ex = ArtifactExchange::new(&id, Arc::new(ArtifactStore::new_memory()), cfg).unwrap();
    ex.set_chain_manager(chain.clone());
    let plane = PlacementControlPlane::new(key, gate_for(&chain), chain, Arc::new(ex), anchors(), conn);
    plane.add_target(&addr, TrustTier::Paired).await.unwrap();
    Rig { plane, host, gate, _tmp: tmp, pkg }
}

fn gate_for(chain: &Arc<ChainManager>) -> Arc<crate::workload_governance::WorkloadGate> {
    gate(chain)
}

#[tokio::test]
async fn a_cognitum_cog_without_an_approval_is_refused_with_the_reason_in_explain_and_on_the_chain() {
    let r = rig(Some(COGNITUM_URL), Err(RunRefusal::NoApproval)).await;
    let rep = r.plane.place(&order(&r.pkg)).await.unwrap();
    assert!(rep.placed.is_none(), "{}", rep.explain);
    let a = &rep.attempts[0];
    assert_eq!(a.code.as_deref(), Some("governance"));
    let reason = a.reason.clone().unwrap();
    assert!(reason.contains("licence run gate: [no_approval]"), "{reason}");
    assert!(reason.contains("weaver cog checkout approve fall-detect@0.1.0"), "{reason}");
    assert!(rep.explain.contains("[no_approval]"), "{}", rep.explain);
    let refused = events(&r.host.chain, EVENT_KIND_WORKLOAD_REFUSE);
    assert!(refused.iter().any(|(_, p)| p["reason"].as_str().is_some_and(|s| s.contains("[no_approval]"))));
    // The gate was asked with the hashes of the bytes that would run.
    let asked = r.gate.asked.lock().unwrap().clone();
    let want_b3 = hex_encode(blake3::hash(SCRIPT.as_bytes()).as_bytes());
    assert_eq!(asked, vec![("fall-detect".into(), "0.1.0".into(), sha256_hex(SCRIPT.as_bytes()), want_b3)]);
}

#[tokio::test]
async fn a_permitted_cog_runs_and_a_lapse_refuses_its_restart() {
    let r = rig(Some(COGNITUM_URL), permit()).await;
    let rep = r.plane.place(&order(&r.pkg)).await.unwrap();
    let placed = rep.placed.clone().unwrap_or_else(|| panic!("{}", rep.explain));
    let permits = events(&r.host.chain, EVENT_KIND_RUN_PERMIT);
    assert_eq!(permits.len(), 1);
    assert_eq!(permits[0].1["approval_id"], "a".repeat(64));
    assert_eq!(permits[0].1["phase"], "place");

    r.plane.instance(method::STOP, &placed.instance_id).await.unwrap();
    r.gate.set(Err(RunRefusal::GrantLapsed));
    let e = r.plane.instance(method::START, &placed.instance_id).await.unwrap_err();
    assert!(e.to_string().contains("[grant_lapsed]"), "{e}");
    r.gate.set(permit());
    r.plane.instance(method::START, &placed.instance_id).await.unwrap();
    assert_eq!(events(&r.host.chain, EVENT_KIND_RUN_PERMIT).len(), 2);
    r.plane.instance(method::STOP, &placed.instance_id).await.unwrap();
}

#[tokio::test]
async fn other_packages_never_reach_the_gate_and_an_unbound_node_is_not_gated() {
    let r = rig(None, Err(RunRefusal::NoApproval)).await;
    let rep = r.plane.place(&order(&r.pkg)).await.unwrap();
    let placed = rep.placed.clone().unwrap_or_else(|| panic!("{}", rep.explain));
    assert!(r.gate.asked.lock().unwrap().is_empty(), "a signed non-Cognitum package skips the gate");
    r.plane.instance(method::STOP, &placed.instance_id).await.unwrap();

    let r = rig(Some(COGNITUM_URL), Ok(RunVerdict::NotSeedBound)).await;
    let rep = r.plane.place(&order(&r.pkg)).await.unwrap();
    let placed = rep.placed.clone().unwrap_or_else(|| panic!("{}", rep.explain));
    assert_eq!(r.gate.asked.lock().unwrap().len(), 1);
    assert!(events(&r.host.chain, EVENT_KIND_RUN_PERMIT).is_empty());
    r.plane.instance(method::STOP, &placed.instance_id).await.unwrap();
}

/// The real stores behind the gate: a member holding the binding and grant
/// starts the cog only once the operator approval arrives, and not after the
/// artifact hash is revoked.
#[tokio::test]
async fn with_the_real_stores_a_member_runs_the_cog_only_with_grant_and_approval() {
    use crate::licence::{
        AdmissionPosture, Approval, ApprovalStore, BindState, BindingRecord, CheckoutGrant, CheckoutGrantStore,
        GrantArtifact, LicenceRef, LocalMeshId, MeshId, NoExtraChecks, StoreRunGate, sign_approval, sign_binding,
        sign_grant, system_clock,
    };
    use crate::revocation::{RevocationKind, RevocationList};
    use crate::workload_pkg::{KeyOrigin, TrustAnchors};

    let k = |n: u8| SigningKey::from_bytes(&[n; 32]);
    let hex = |s: &SigningKey| hex_encode(&s.verifying_key().to_bytes());
    let now = chrono::Utc::now().timestamp() as u64;
    let dir = tempfile::tempdir().unwrap();
    let mesh = MeshId::derive(&[9; 32], &[7; 32]);
    let mut ops = TrustAnchors::default();
    ops.push_signer("op", &hex(&k(1)), KeyOrigin::Operator).unwrap();
    let ops = Arc::new(ops);
    let local = LocalMeshId::new(mesh);
    let store = Arc::new(CheckoutGrantStore::open(dir.path(), ops.clone(), local.clone(), system_clock()).unwrap());
    let approvals = Arc::new(ApprovalStore::open(dir.path(), ops, local).unwrap());
    let revoked = Arc::new(RevocationList::new(dir.path().join("revoked.json")));
    store.attach_revocations(revoked.clone());
    let rec = BindingRecord {
        v: 2,
        device_id: "seed-1".into(),
        device_pubkey: hex(&k(20)),
        mesh_id: mesh.to_hex(),
        grant_pubkey: hex(&k(2)),
        steward_node_id: "node-steward".into(),
        steward_pubkey: hex(&k(21)),
        state: BindState::Bound,
        seq: 1,
        bound_at: now,
    };
    let posture = AdmissionPosture { enforce: true, verdict_source_bound: true, open_membership: false };
    store.accept_binding(&sign_binding(&rec, &k(1)).unwrap(), posture, &NoExtraChecks).unwrap();
    let (sha, b3) = (sha256_hex(SCRIPT.as_bytes()), hex_encode(blake3::hash(SCRIPT.as_bytes()).as_bytes()));
    let g = CheckoutGrant {
        v: 1,
        grant_id: String::new(),
        mesh_id: mesh.to_hex(),
        seed_device_id: "seed-1".into(),
        grant_key_id: String::new(),
        source: "cognitum".into(),
        registry: "registry.example".into(),
        cog_id: "fall-detect".into(),
        version: "0.1.0".into(),
        artifacts: vec![GrantArtifact { arch: arch().into(), size: SCRIPT.len() as u64, sha256: sha.clone(), blake3: b3.clone() }],
        manifest_sha256: sha256_hex(b"manifest"),
        licence: LicenceRef { ref_sha256: sha256_hex(b"licence"), expires: now + 30 * 86_400 },
        seq: 1,
        issued_at: now,
        expires_at: now + 72 * 3600,
    };
    store.accept_grant(&sign_grant(&g, &k(2)).unwrap()).unwrap();

    let real = StoreRunGate { grants: store.clone(), approvals: Some(approvals.clone()) };
    let tmp = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[30; 32]);
    let host = host_node(32, board_caps("pi5"), true, &key);
    assert!(host.svc.set_licence_gate(Arc::new(real)));
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("member", host.svc.clone());
    let chain = Arc::new(ChainManager::new(0, 1000));
    let cfg = ExchangeConfig { redistribution: Arc::new(ServeAll), ..ExchangeConfig::default() };
    let id = node_id_from_pubkey(&key.verifying_key().to_bytes());
    let ex = ArtifactExchange::new(&id, Arc::new(ArtifactStore::new_memory()), cfg).unwrap();
    let plane = PlacementControlPlane::new(key, gate_for(&chain), chain, Arc::new(ex), anchors(), conn);
    plane.add_target(&addr, TrustTier::Paired).await.unwrap();
    let pkg = package_from(tmp.path(), "fall-detect", SCRIPT, &[arch()], Some(COGNITUM_URL));

    // Grant only: refused.
    let rep = plane.place(&order(&pkg)).await.unwrap();
    assert!(rep.placed.is_none());
    assert!(rep.attempts[0].reason.as_deref().unwrap_or("").contains("[no_approval]"), "{}", rep.explain);

    // The operator approval arrives: the member runs it.
    let a = Approval { v: 1, mesh_id: mesh.to_hex(), cog_id: "fall-detect".into(), version: "0.1.0".into(), sha256: vec![sha], approved_at: now };
    approvals.accept(&sign_approval(&a, &k(1)).unwrap()).unwrap();
    let rep = plane.place(&order(&pkg)).await.unwrap();
    let placed = rep.placed.clone().unwrap_or_else(|| panic!("{}", rep.explain));
    let permit = &events(&host.chain, EVENT_KIND_RUN_PERMIT)[0].1;
    assert_eq!(permit["approval_id"], a.content_key().as_str());

    // Revoking the artifact hash refuses the restart.
    plane.instance(method::STOP, &placed.instance_id).await.unwrap();
    revoked.revoke_subject(RevocationKind::ArtifactHash, &b3, "withdrawn").unwrap();
    let e = plane.instance(method::START, &placed.instance_id).await.unwrap_err();
    assert!(e.to_string().contains("[hash_revoked]"), "{e}");
}
