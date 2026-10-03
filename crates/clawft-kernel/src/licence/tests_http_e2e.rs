//! ADR-106 phase 3, end to end on one machine: the real `weft-licence`
//! service on a loopback listener, the steward's HTTP transport and relay,
//! and three in-process mesh nodes (A - B - C, A is the steward).
//!
//! A checks out through `weft-licence` over HTTP, the grant floods to B and
//! C, and the run gate on B refuses until an operator approval (issued from
//! C, a non-steward node) arrives. Then B may run the binary, whose hashes
//! are computed from the bytes A received. Revoking the artifact hash
//! refuses it again. Only loopback is used, with real clocks on both sides.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use weft_licence::providers::*;
use weft_licence::state::OperatorKeys;
use weft_licence::{Config, Service};

use super::tests_common::*;
use super::tests_exchange::{AdmitAll, TNode, link, quiet_cfg, wait_for};
use super::*;
use crate::artifact_store::ArtifactStore;
use crate::gate::{GateBackend, GateDecision};
use crate::mesh_artifact::{ArtifactExchange, ExchangeConfig};
use crate::mesh_artifact_types::ArtifactKey;
use crate::mesh_runtime::MeshRuntime;
use crate::revocation::{RevocationKind, RevocationList};
use crate::workload_runtime::seed_tls::SeedTls;

pub(super) const BIN: &[u8] = b"\x7fELF fall-detect for aarch64";

/// The declared licence; the test can withdraw coverage.
#[derive(Clone, Default)]
pub(super) struct Lic(pub Arc<std::sync::atomic::AtomicBool>);
impl LicenceProvider for Lic {
    fn entitlement(&self, _: &str, _: &str, _: u64) -> Result<Entitlement, LicenceCheckError> {
        if self.0.load(Ordering::SeqCst) {
            return Err(LicenceCheckError::Unlicensed);
        }
        Ok(Entitlement { ref_sha256: sha256_hex(b"licence"), expires: None })
    }
}

pub(super) struct Reg(pub Arc<AtomicU64>);
impl CogFetcher for Reg {
    fn resolve(&self, cog: &str, _: &str) -> Result<CogEntry, FetchError> {
        if cog != "fall-detect" {
            return Err(FetchError::NotFound);
        }
        Ok(CogEntry {
            cog_id: cog.into(),
            version: "1.2.0".into(),
            registry: "registry.example".into(),
            manifest_sha256: sha256_hex(b"manifest"),
            artifacts: vec![EntryArtifact { arch: "aarch64".into(), size: BIN.len() as u64, sha256: sha256_hex(BIN) }],
        })
    }
    fn fetch(&self, _: &CogEntry, arch: &str) -> Result<Vec<u8>, FetchError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        (arch == "aarch64").then(|| BIN.to_vec()).ok_or(FetchError::ArchUnavailable(arch.into()))
    }
}

pub(super) struct PermitAll;
impl GateBackend for PermitAll {
    fn check(&self, _: &str, _: &str, _: &serde_json::Value) -> GateDecision {
        GateDecision::Permit { token: None }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

/// A mesh node whose stores run on the real clock.
fn node(id: &str) -> TNode {
    node_clocked(id, system_clock())
}

/// A mesh node whose stores run on `clock`.
pub(super) fn node_clocked(id: &str, clock: Clock) -> TNode {
    let dir = tempfile::tempdir().unwrap();
    let local = LocalMeshId::new(mesh());
    let sink = Arc::new(RecordingSink::default());
    let store = Arc::new(CheckoutGrantStore::open(dir.path(), anchors(), local.clone(), clock).unwrap());
    store.set_sink(sink.clone());
    let approvals = Arc::new(ApprovalStore::open(dir.path(), anchors(), local.clone()).unwrap());
    let fx = Fx { dir, clock: Arc::new(AtomicU64::new(0)), local, sink, store, approvals };
    let rt = Arc::new(MeshRuntime::new(id.to_string()));
    let ex = LicenceExchange::start(LicenceExchangeParts {
        store: fx.store.clone(),
        approvals: fx.approvals.clone(),
        anchors: anchors(),
        runtime: rt.clone(),
        posture: Arc::new(posture),
        admission: Arc::new(AdmitAll),
        sink: fx.sink.clone(),
        config: quiet_cfg(),
    });
    TNode { id: id.to_string(), fx, rt, ex }
}

/// The Seed: key, operator-signed binding naming the steward, a loopback listener.
fn seed(fetches: Arc<AtomicU64>) -> (tempfile::TempDir, SignedBinding, weft_licence::http::Server) {
    seed_with(fetches, Arc::new(now_secs), 72 * 3600, Lic::default())
}

/// [`seed`] on `clock`, with a grant TTL and a licence the test controls.
pub(super) fn seed_with(
    fetches: Arc<AtomicU64>,
    clock: Clock,
    ttl: u64,
    lic: Lic,
) -> (tempfile::TempDir, SignedBinding, weft_licence::http::Server) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        state_dir: dir.path().join("state"),
        device_id: "seed-test".into(),
        operator_pubkeys: vec![pk_hex(&op())],
        listen: vec!["127.0.0.1:0".parse().unwrap()],
        grant_ttl_secs: ttl,
        ..Config::default()
    };
    let init = weft_licence::keys::init(&cfg.state_dir).unwrap();
    let mut rec = binding_rec(1, BindState::Bound, &grant_key(), &mesh());
    rec.grant_pubkey = init.grant_pubkey.clone();
    rec.bound_at = clock();
    let signed = sign_binding(&rec, &op()).unwrap();
    let ops = OperatorKeys::load(&cfg.state_dir, &cfg.operator_pubkeys).unwrap();
    weft_licence::bind::apply(&cfg.state_dir, "seed-test", &ops, &signed, None).unwrap();
    let svc = Service::open(
        cfg,
        clock,
        Box::new(lic),
        Box::new(Reg(fetches)),
        Box::new(StubDeviceSigner),
    )
    .unwrap();
    let server = weft_licence::http::serve(Arc::new(svc), &["127.0.0.1:0".parse().unwrap()]).unwrap();
    (dir, signed, server)
}

pub(super) fn wire() -> CheckoutWire {
    CheckoutWire { request_id: "r1".into(), cog_id: "fall-detect".into(), version: "latest".into(), arch: "aarch64".into() }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn checkout_over_http_floods_then_approval_gates_the_run_on_a_member() {
    let fetches = Arc::new(AtomicU64::new(0));
    let (_seed_dir, binding, server) = seed(fetches.clone());
    let addr = server.addrs()[0];
    let (a, b, c) = (node("node-steward"), node("node-b"), node("node-c"));
    link(&a, &b, true);
    link(&b, &c, true);
    a.ex.issue_binding(binding).await.unwrap();
    wait_for("C bound", || c.fx.store.active_binding().is_some()).await;

    // The steward's relay over the real HTTP transport (lab opt-in: plain loopback).
    let transport = HttpLicenceTransport::new(LicenceLinkConfig {
        url: format!("http://{addr}"),
        tls: SeedTls::WebPki,
        allow_unpinned_lab_link: true,
        limits: TransportLimits::default(),
    })
    .unwrap();
    let client = StewardLicenceClient::new(a.fx.store.clone(), sk(21), "node-steward", Arc::new(transport), system_clock_ms());
    let ax = Arc::new(ArtifactExchange::new("node-steward", Arc::new(ArtifactStore::new_memory()), ExchangeConfig::default()).unwrap());
    let flood: Arc<dyn GrantFlood> = a.ex.clone();
    let relay = CheckoutRelay::new(a.fx.store.clone(), ax.clone(), client, Arc::new(PermitAll), flood, None);

    let signed = tokio::time::timeout(Duration::from_secs(20), relay.handle(CheckoutCaller::Kernel, &wire()))
        .await
        .expect("relay finished")
        .expect("checkout granted");
    let g: CheckoutGrant = serde_json::from_str(&signed.payload).unwrap();
    assert_eq!((g.cog_id.as_str(), g.version.as_str()), ("fall-detect", "1.2.0"));
    assert_eq!(fetches.load(Ordering::SeqCst), 1, "one registry fetch");

    // The grant reached C through B.
    wait_for("C holds the grant", || c.fx.store.held_grant("fall-detect", "1.2.0").is_some()).await;

    // The run request on B carries the hashes of the bytes the steward got.
    let d = ax.resolve(&ArtifactKey::Content(*blake3::hash(BIN).as_bytes())).expect("seeded");
    let bytes = ax.read_all(&d.id()).unwrap();
    let (sha, b3) = (sha256_hex(&bytes), hex_encode(blake3::hash(&bytes).as_bytes()));
    let req = RunRequest { cog_id: "fall-detect", version: "1.2.0", sha256: &sha, blake3: &b3 };
    let gate_b = StoreRunGate { grants: b.fx.store.clone(), approvals: Some(b.fx.approvals.clone()) };
    assert_eq!(gate_b.check(&req), Err(RunRefusal::NoApproval), "a grant alone never runs");

    // The operator approves from C (any node); the approval floods to B.
    let approval = Approval {
        v: 1,
        mesh_id: mesh().to_hex(),
        cog_id: "fall-detect".into(),
        version: "1.2.0".into(),
        sha256: vec![sha.clone()],
        approved_at: now_secs(),
    };
    c.ex.issue_approval(sign_approval(&approval, &op()).unwrap()).await.unwrap();
    wait_for("B holds the approval", || b.fx.approvals.len() == 1).await;
    let Ok(RunVerdict::Permit(p)) = gate_b.check(&req) else { panic!("B may run it now") };
    assert_eq!(p.grant_id, g.grant_id);

    // A second member request is answered from the Seed's existing grant; no new fetch.
    let again = relay.handle(CheckoutCaller::Kernel, &wire()).await.expect("again");
    assert_eq!(serde_json::from_str::<CheckoutGrant>(&again.payload).unwrap().seq, g.seq);
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    // Revoking the artifact refuses the run.
    let list = Arc::new(RevocationList::new(b.fx.dir.path().join("revoked.json")));
    b.fx.store.attach_revocations(list.clone());
    list.revoke_subject(RevocationKind::ArtifactHash, &b3, "withdrawn").unwrap();
    assert_eq!(gate_b.check(&req), Err(RunRefusal::HashRevoked));
    server.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_weft_licence_refusal_and_a_foreign_steward_key_come_back_as_codes() {
    let fetches = Arc::new(AtomicU64::new(0));
    let (_seed_dir, binding, server) = seed(fetches.clone());
    let addr = server.addrs()[0];
    let a = node("node-steward");
    a.ex.issue_binding(binding).await.unwrap();
    let transport = HttpLicenceTransport::new(LicenceLinkConfig {
        url: format!("http://{addr}"),
        tls: SeedTls::WebPki,
        allow_unpinned_lab_link: true,
        limits: TransportLimits::default(),
    })
    .unwrap();
    let client = StewardLicenceClient::new(a.fx.store.clone(), sk(21), "node-steward", Arc::new(transport), system_clock_ms());
    let mut w = wire();
    w.cog_id = "not-a-cog".into();
    let e = client.checkout(&w).await.unwrap_err();
    assert!(matches!(&e, LicenceClientError::Refused { code, .. } if code == "cog_not_found"), "{e:?}");
    // A request signed by a key the Seed does not know as its steward.
    let rogue = StewardLicenceClient::new(a.fx.store.clone(), sk(22), "node-steward", client_transport(addr), system_clock_ms());
    assert!(matches!(rogue.checkout(&wire()).await, Err(LicenceClientError::Refused { code, .. }) if code == "not_steward"));
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
    server.stop();
}

pub(super) fn client_transport(addr: std::net::SocketAddr) -> Arc<dyn LicenceTransport> {
    Arc::new(
        HttpLicenceTransport::new(LicenceLinkConfig {
            url: format!("http://{addr}"),
            tls: SeedTls::WebPki,
            allow_unpinned_lab_link: true,
            limits: TransportLimits::default(),
        })
        .unwrap(),
    )
}
