//! `workload.cog.checkout.release | renew | list` against the real
//! `weft-licence` on a loopback listener: the boot-owned stores, the steward
//! client over HTTP, a checkout through the relay, then the verbs.

use std::path::Path;
use std::sync::Arc;

use clawft_kernel::artifact_store::ArtifactStore;
use clawft_kernel::chain::ChainManager;
use clawft_kernel::gate::{GateBackend, GateDecision};
use clawft_kernel::licence::*;
use clawft_kernel::mesh_artifact::{ArtifactExchange, ExchangeConfig};
use clawft_kernel::mesh_runtime::MeshRuntime;
use clawft_kernel::revocation::RevocationList;
use clawft_kernel::workload_pkg::codec::hex_encode;
use clawft_kernel::workload_pkg::{KeyOrigin, TrustAnchors};
use clawft_kernel::workload_runtime::seed_tls::SeedTls;
use clawft_types::config::{MeshAdmissionMode, MeshConfig};
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};
use weft_licence::providers::*;

use crate::licence_boot::{InitArgs, LicenceRuntime, build};
use crate::licence_checkout_rpc::{Ctx, route};

const PIN: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
const NONCE: &str = "bb00000000000000000000000000000000000000000000000000000000000002";
const BIN: &[u8] = b"\x7fELF fall-detect";

fn sk(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}
fn pk(k: &SigningKey) -> String {
    hex_encode(&k.verifying_key().to_bytes())
}
fn now() -> u64 {
    chrono::Utc::now().timestamp() as u64
}

struct Lic;
impl LicenceProvider for Lic {
    fn entitlement(&self, _: &str, _: &str, _: u64) -> Result<Entitlement, LicenceCheckError> {
        Ok(Entitlement { ref_sha256: sha256_hex(b"licence"), expires: None })
    }
}

struct Reg;
impl CogFetcher for Reg {
    fn resolve(&self, cog: &str, _: &str) -> Result<CogEntry, FetchError> {
        Ok(CogEntry {
            cog_id: cog.into(),
            version: "1.2.0".into(),
            registry: "registry.example".into(),
            manifest_sha256: sha256_hex(b"manifest"),
            artifacts: vec![EntryArtifact { arch: "aarch64".into(), size: BIN.len() as u64, sha256: sha256_hex(BIN) }],
        })
    }
    fn fetch(&self, _: &CogEntry, _: &str) -> Result<Vec<u8>, FetchError> {
        Ok(BIN.to_vec())
    }
}

struct PermitAll;
impl GateBackend for PermitAll {
    fn check(&self, _: &str, _: &str, _: &Value) -> GateDecision {
        GateDecision::Permit { token: None }
    }
}

fn anchors() -> TrustAnchors {
    let mut a = TrustAnchors::default();
    a.push_signer("op", &pk(&sk(1)), KeyOrigin::Operator).unwrap();
    a
}

fn boot(dir: &Path, chain: &Arc<ChainManager>) -> LicenceRuntime {
    let mesh = MeshConfig {
        enabled: true,
        admission: MeshAdmissionMode::Enforce,
        genesis_hash: Some(PIN.into()),
        mesh_nonce: Some(NONCE.into()),
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

struct Rig {
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    chain: Arc<ChainManager>,
    rt: LicenceRuntime,
    renewer: Arc<Renewer>,
    exchange: Arc<LicenceExchange>,
    _server: weft_licence::http::Server,
}

/// A Seed bound to this node's mesh with `steward` as its steward; this
/// node holds the same binding and one checkout of fall-detect.
async fn rig(steward: &str) -> Rig {
    let (dir, seed_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rt = boot(dir.path(), &chain);
    let cfg = weft_licence::Config {
        state_dir: seed_dir.path().join("state"),
        device_id: "seed-1".into(),
        operator_pubkeys: vec![pk(&sk(1))],
        listen: vec!["127.0.0.1:0".parse().unwrap()],
        ..weft_licence::Config::default()
    };
    let init = weft_licence::keys::init(&cfg.state_dir).unwrap();
    let rec = BindingRecord {
        v: 2,
        device_id: "seed-1".into(),
        device_pubkey: pk(&sk(20)),
        mesh_id: rt.local.get().unwrap().to_hex(),
        grant_pubkey: init.grant_pubkey.clone(),
        steward_node_id: steward.into(),
        steward_pubkey: pk(&sk(21)),
        state: BindState::Bound,
        seq: 1,
        bound_at: now(),
    };
    let signed = sign_binding(&rec, &sk(1)).unwrap();
    let ops = weft_licence::state::OperatorKeys::load(&cfg.state_dir, &cfg.operator_pubkeys).unwrap();
    weft_licence::bind::apply(&cfg.state_dir, "seed-1", &ops, &signed, None).unwrap();
    let svc = weft_licence::Service::open(cfg, Arc::new(now), Box::new(Lic), Box::new(Reg), Box::new(StubDeviceSigner)).unwrap();
    let server = weft_licence::http::serve(Arc::new(svc), &["127.0.0.1:0".parse().unwrap()]).unwrap();
    let posture = AdmissionPosture { enforce: true, verdict_source_bound: true, open_membership: false };
    rt.store().accept_binding(&signed, posture, &NoExtraChecks).unwrap();
    let transport = HttpLicenceTransport::new(LicenceLinkConfig {
        url: format!("http://{}", server.addrs()[0]),
        tls: SeedTls::WebPki,
        allow_unpinned_lab_link: true,
        limits: TransportLimits::default(),
    })
    .unwrap();
    let client: Arc<dyn LicenceClient> =
        StewardLicenceClient::new(rt.store().clone(), sk(21), "node-steward", Arc::new(transport), system_clock_ms());
    let ax = Arc::new(ArtifactExchange::new("node-steward", Arc::new(ArtifactStore::new_memory()), ExchangeConfig::default()).unwrap());
    if steward == "node-steward" {
        let relay = CheckoutRelay::new(rt.store().clone(), ax.clone(), client.clone(), Arc::new(PermitAll), Arc::new(NoFlood), None);
        let wire = CheckoutWire { request_id: "r".into(), cog_id: "fall-detect".into(), version: "latest".into(), arch: "aarch64".into() };
        relay.handle(CheckoutCaller::Kernel, &wire).await.expect("checkout");
    }
    let renewer = Renewer::new(rt.store().clone(), ax, client, Arc::new(NoFlood), Some(chain.clone()), RenewalConfig::default());
    let a = Arc::new(anchors());
    let exchange = LicenceExchange::start(LicenceExchangeParts {
        store: rt.store().clone(),
        approvals: Arc::new(ApprovalStore::open_or_poisoned(&dir.path().join("licence"), a.clone(), rt.local.clone())),
        anchors: a,
        runtime: Arc::new(MeshRuntime::new("node-steward".into())),
        posture: Arc::new(move || posture),
        admission: Arc::new(CtxAdmission),
        sink: Arc::new(NoopSink),
        config: LicenceExchangeConfig { sync_on_connect: false, ..Default::default() },
    });
    Rig { _dirs: (dir, seed_dir), chain, rt, renewer, exchange, _server: server }
}

fn never(_: &str) -> bool {
    false
}

static UNPACED: crate::licence_checkout_verbs::ManualLimit =
    crate::licence_checkout_verbs::ManualLimit::new(std::time::Duration::ZERO);

async fn call(r: &Rig, renewer: bool, m: &str, params: Value) -> Result<Value, String> {
    call_as(r, renewer, m, params, "operator", &UNPACED).await
}

async fn call_as(
    r: &Rig,
    renewer: bool,
    m: &str,
    params: Value,
    principal: &str,
    manual: &crate::licence_checkout_verbs::ManualLimit,
) -> Result<Value, String> {
    let ctx = Ctx {
        rt: &r.rt,
        mesh: None,
        exchange: Some(r.exchange.clone()),
        relay: None,
        reachable: &never,
        arch: Some("aarch64"),
        now: now(),
        principal,
        renewer: renewer.then(|| r.renewer.clone()),
        manual,
    };
    let resp = route(&ctx, m, params).await;
    if resp.ok { Ok(resp.result.unwrap_or_default()) } else { Err(resp.error.unwrap_or_default()) }
}

fn kinds(chain: &ChainManager) -> Vec<String> {
    chain.tail(chain.len()).into_iter().map(|e| e.kind).collect()
}

const ONE: fn() -> Value = || json!({"cog_id": "fall-detect", "version": "1.2.0"});

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn renew_list_and_release_on_the_steward_against_the_real_seed() {
    let r = rig("node-steward").await;
    let l = call(&r, true, "workload.cog.checkout.list", json!({})).await.unwrap();
    assert_eq!((l["grants"][0]["seq"].as_u64(), l["grants"][0]["valid"].as_bool()), (Some(1), Some(true)));
    assert!(l["grants"][0]["expires_in"].as_u64().unwrap() > 70 * 3600);
    assert!(l["grants"][0]["artifacts"][0]["approval_id"].is_null(), "not approved yet");

    let v = call(&r, true, "workload.cog.checkout.renew", ONE()).await.expect("renewed");
    assert_eq!((v["seq_before"].as_u64(), v["seq"].as_u64(), v["renewed"].as_bool()), (Some(1), Some(2), Some(true)));
    let k = kinds(&r.chain);
    assert!(k.contains(&"cog.checkout.renew".into()) && k.contains(&"cog.checkout.renewed".into()), "{k:?}");

    let v = call(&r, true, "workload.cog.checkout.release", ONE()).await.expect("released");
    assert_eq!((v["released"].as_bool(), v["seq"].as_u64()), (Some(true), Some(3)));
    let k = kinds(&r.chain);
    assert!(k.contains(&"cog.checkout.release".into()) && k.contains(&"cog.checkout.lapsed".into()), "{k:?}");
    let l = call(&r, true, "workload.cog.checkout.list", json!({})).await.unwrap();
    assert_eq!((l["grants"][0]["withdrawn"].as_bool(), l["grants"][0]["valid"].as_bool()), (Some(true), Some(false)));

    // A renewal after the release does not revive it.
    let v = call(&r, true, "workload.cog.checkout.renew", ONE()).await.unwrap();
    assert_eq!(v["withdrawn"].as_bool(), Some(true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_and_renew_refuse_off_the_steward_without_a_renewer_and_for_bad_input() {
    let r = rig("node-steward").await;
    let e = call(&r, false, "workload.cog.checkout.release", ONE()).await.unwrap_err();
    assert!(e.starts_with("[no_steward]"), "{e}");
    let e = call(&r, true, "workload.cog.checkout.release", json!({"cog_id": "fall-detect", "version": "latest"})).await.unwrap_err();
    assert!(e.contains("exact version"), "{e}");
    let e = call(&r, true, "workload.cog.checkout.renew", json!({"cog_id": "other", "version": "1.0.0"})).await.unwrap_err();
    assert!(e.contains("no checkout of other@1.0.0"), "{e}");
    assert!(call(&r, true, "workload.cog.checkout.release", json!({"cog_id": "x"})).await.is_err(), "missing version");

    let member = rig("node-other").await;
    let e = call(&member, true, "workload.cog.checkout.release", ONE()).await.unwrap_err();
    assert!(e.starts_with("[not_steward]") && e.contains("node-other"), "{e}");
    let e = call(&member, true, "workload.cog.checkout.renew", ONE()).await.unwrap_err();
    assert!(e.starts_with("[not_steward]"), "{e}");
    // list is read-only and works anywhere.
    assert!(call(&member, false, "workload.cog.checkout.list", json!({})).await.unwrap()["grants"].as_array().unwrap().is_empty());
}


#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_renew_and_release_are_paced_node_wide_and_chain_the_callers_principal() {
    let r = rig("node-steward").await;
    let paced = crate::licence_checkout_verbs::ManualLimit::new(std::time::Duration::from_secs(60));
    call_as(&r, true, "workload.cog.checkout.renew", ONE(), "project:01J000000000000000000000PX", &paced).await.expect("first");
    let e = call_as(&r, true, "workload.cog.checkout.release", ONE(), "operator", &paced).await.unwrap_err();
    assert!(e.starts_with("[rate_limited]") && e.contains("try again in"), "{e}");
    // A refused call is not chained and spends nothing at the Seed.
    let renews: Vec<Value> = r.chain.tail(r.chain.len()).into_iter()
        .filter(|ev| ev.kind == "cog.checkout.renew").filter_map(|ev| ev.payload).collect();
    assert_eq!(renews.len(), 1);
    assert_eq!(renews[0]["principal"], "project:01J000000000000000000000PX", "the event names the caller");
    assert!(!kinds(&r.chain).contains(&"cog.checkout.release".to_string()));
}

#[test]
fn release_and_renew_are_admin_extension_routes_by_exact_name() {
    let routes = crate::rpc_ext::builtin_route_names();
    for m in ["workload.cog.checkout.release", "workload.cog.checkout.renew"] {
        assert!(routes.contains(&(m, crate::capability::Capability::Admin)), "{m}");
    }
    assert!(!routes.iter().any(|(p, _)| *p == "workload.cog.checkout."), "no prefix route");
}
