//! The relay and renewer in service mode (ADR-106 phase 3 integration): the
//! client signs as the mesh node id the binding names, acts only while this
//! daemon holds the licence role, and the flood and the run gate find an
//! exchange that started after them.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use clawft_kernel::artifact_store::ArtifactStore;
use clawft_kernel::licence::*;
use clawft_kernel::mesh_artifact::{ArtifactExchange, ExchangeConfig};
use clawft_kernel::workload_pkg::codec::hex_encode;
use clawft_kernel::workload_pkg::{KeyOrigin, TrustAnchors};
use ed25519_dalek::SigningKey;

use super::*;

fn sk(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

fn pk(k: &SigningKey) -> String {
    hex_encode(&k.verifying_key().to_bytes())
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

fn mesh() -> MeshId {
    MeshId::derive(&[9; 32], &[7; 32])
}

fn anchors() -> Arc<TrustAnchors> {
    let mut a = TrustAnchors::default();
    a.push_signer("operator-1", &pk(&sk(1)), KeyOrigin::Operator).unwrap();
    Arc::new(a)
}

fn machine_id() -> String {
    clawft_kernel::node_id_from_pubkey(&sk(9).verifying_key().to_bytes())
}

/// A grant store bound to a Seed whose binding names the machine as steward
/// and the control key (`sk(21)`) as its request key.
fn bound_store(dir: &std::path::Path) -> Arc<CheckoutGrantStore> {
    let store = Arc::new(CheckoutGrantStore::open(dir, anchors(), LocalMeshId::new(mesh()), system_clock()).unwrap());
    let rec = BindingRecord {
        v: 2,
        device_id: "seed-x".into(),
        device_pubkey: pk(&sk(20)),
        mesh_id: mesh().to_hex(),
        grant_pubkey: pk(&sk(2)),
        steward_node_id: machine_id(),
        steward_pubkey: pk(&sk(21)),
        state: BindState::Bound,
        seq: 1,
        bound_at: now(),
    };
    let posture = AdmissionPosture { enforce: true, verdict_source_bound: true, open_membership: false };
    store.accept_binding(&sign_binding(&rec, &sk(1)).unwrap(), posture, &NoExtraChecks).unwrap();
    store
}

/// Answers every call with an empty grants page; records the paths and the
/// node each request was signed as.
#[derive(Default)]
struct Stub(Mutex<Vec<(String, String)>>);

#[async_trait]
impl LicenceTransport for Stub {
    async fn call(&self, req: LicenceRequest) -> Result<LicenceResponse, LicenceClientError> {
        let node = req.auth.as_ref().map(|a| a.node.clone()).unwrap_or_default();
        self.0.lock().unwrap().push((req.path.clone(), node));
        Ok(LicenceResponse { status: 200, body: br#"{"grants":[],"next":0}"#.to_vec() })
    }
}

fn exchange() -> Arc<ArtifactExchange> {
    Arc::new(ArtifactExchange::new("n", Arc::new(ArtifactStore::new_memory()), ExchangeConfig::default()).unwrap())
}

fn renewer(store: &Arc<CheckoutGrantStore>, client: Arc<dyn LicenceClient>) -> Arc<Renewer> {
    Renewer::new(store.clone(), exchange(), client, Arc::new(NoFlood), None, RenewalConfig::default())
}

#[tokio::test]
async fn a_binding_naming_the_machine_and_the_control_key_is_served_and_renewed() {
    let tmp = tempfile::tempdir().unwrap();
    let store = bound_store(tmp.path());
    let stub = Arc::new(Stub::default());
    // Service mode: sign as the machine's node id with the control key.
    let client = steward_client(&store, &sk(21), &machine_id(), stub.clone(), Arc::new(|| true));
    let report = renewer(&store, client).run_once().await.unwrap();
    assert!(!report.skipped, "the client accepts the binding and the pass runs");
    let calls = stub.0.lock().unwrap().clone();
    assert!(calls.iter().any(|(p, _)| p == RENEW_PATH), "{calls:?}");
    assert!(calls.iter().all(|(_, n)| *n == machine_id()), "x-licence-node is the machine id: {calls:?}");

    // The integration defect: signing as the control key's own id is never the steward.
    let wrong = clawft_kernel::node_id_from_pubkey(&sk(21).verifying_key().to_bytes());
    let stub2 = Arc::new(Stub::default());
    let client = steward_client(&store, &sk(21), &wrong, stub2.clone(), Arc::new(|| true));
    assert!(renewer(&store, client).run_once().await.unwrap().skipped);
    assert!(stub2.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_daemon_without_the_licence_role_runs_no_relay_call_or_renewal_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let store = bound_store(tmp.path());
    let stub = Arc::new(Stub::default());
    let holder = Arc::new(AtomicBool::new(false));
    let h = holder.clone();
    let client = steward_client(&store, &sk(21), &machine_id(), stub.clone(), Arc::new(move || h.load(Ordering::SeqCst)));
    let r = renewer(&store, client.clone());
    assert!(r.run_once().await.unwrap().skipped, "a non-holder's pass is skipped");
    let req = CheckoutWire { request_id: "r-1".into(), cog_id: "fall-detect".into(), version: "1.2.0".into(), arch: "aarch64".into() };
    match client.checkout(&req).await {
        Err(LicenceClientError::Refused { code, .. }) => assert_eq!(code, "not_holder"),
        other => panic!("expected not_holder, got {other:?}"),
    }
    assert!(stub.0.lock().unwrap().is_empty(), "nothing is sent to the Seed");
    // The role arrives after the client was built: the next pass runs.
    holder.store(true, Ordering::SeqCst);
    assert!(!r.run_once().await.unwrap().skipped);
    assert!(!stub.0.lock().unwrap().is_empty());
}

#[derive(Default)]
struct Recorded(AtomicUsize);

#[async_trait]
impl GrantFlood for Recorded {
    async fn flood(&self, _: &SignedGrant) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn a_grant() -> SignedGrant {
    let b = b"cog".to_vec();
    let g = CheckoutGrant {
        v: 1,
        grant_id: String::new(),
        mesh_id: mesh().to_hex(),
        seed_device_id: "seed-x".into(),
        grant_key_id: String::new(),
        source: "cognitum".into(),
        registry: "registry.example".into(),
        cog_id: "fall-detect".into(),
        version: "1.2.0".into(),
        artifacts: vec![GrantArtifact {
            arch: "aarch64".into(),
            size: b.len() as u64,
            sha256: sha256_hex(&b),
            blake3: hex_encode(blake3::hash(&b).as_bytes()),
        }],
        manifest_sha256: sha256_hex(b"m"),
        licence: LicenceRef { ref_sha256: sha256_hex(b"l"), expires: now() + 86_400 },
        seq: 1,
        issued_at: now(),
        expires_at: now() + 3600,
    };
    sign_grant(&g, &sk(2)).unwrap()
}

#[tokio::test]
async fn a_flood_built_before_the_exchange_reaches_it_once_it_starts() {
    let cell: Arc<OnceLock<Arc<dyn GrantFlood>>> = Arc::default();
    let c = cell.clone();
    let late = crate::cog_swarm::LateFlood::new(Arc::new(move || c.get().cloned()));
    late.flood(&a_grant()).await; // no exchange yet: nothing, no panic
    let rec = Arc::new(Recorded::default());
    let _ = cell.set(rec.clone());
    late.flood(&a_grant()).await;
    assert_eq!(rec.0.load(Ordering::SeqCst), 1, "the exchange that started later gets the flood");
}

#[test]
fn the_run_gate_sees_approvals_from_an_exchange_that_started_after_it() {
    let tmp = tempfile::tempdir().unwrap();
    let grants = Arc::new(
        CheckoutGrantStore::open(tmp.path(), anchors(), LocalMeshId::new(mesh()), system_clock()).unwrap(),
    );
    let cell: Arc<OnceLock<Arc<ApprovalStore>>> = Arc::default();
    let c = cell.clone();
    let gate = LateRunGate { grants, approvals: Arc::new(move || c.get().cloned()) };
    let sha = sha256_hex(b"cog");
    let b3 = hex_encode(blake3::hash(b"cog").as_bytes());
    let req = RunRequest { cog_id: "fall-detect", version: "1.2.0", sha256: &sha, blake3: &b3 };
    assert_eq!(gate.check(&req), Ok(RunVerdict::NotSeedBound), "no approvals seen yet");
    // The exchange starts later with an approval: the same gate sees it.
    let dir2 = tempfile::tempdir().unwrap();
    let approvals = Arc::new(ApprovalStore::open(dir2.path(), anchors(), LocalMeshId::new(mesh())).unwrap());
    let a = Approval {
        v: 1,
        mesh_id: mesh().to_hex(),
        cog_id: "fall-detect".into(),
        version: "1.2.0".into(),
        sha256: vec![sha.clone()],
        approved_at: now(),
    };
    approvals.accept(&sign_approval(&a, &sk(1)).unwrap()).unwrap();
    let _ = cell.set(approvals);
    assert_ne!(gate.check(&req), Ok(RunVerdict::NotSeedBound), "the approvals now count (this node is Seed-bound)");
}
