//! Shared fixtures: a Seed state dir with a key and a binding, stub licence
//! and registry, and a signing steward client. No network, no real keys.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;
use tempfile::TempDir;
use weft_licence::bind;
use weft_licence::config::Config;
use weft_licence::keys;
use weft_licence::providers::*;
use weft_licence::request::{Request, sign_request};
use weft_licence::state::OperatorKeys;
use weft_licence::{Response, Service};
use weft_licence_wire::{BindState, BindingRecord, MeshId, hex_encode, sha256_hex, sign_binding};

pub const T0: u64 = 1_791_000_000;
pub const NODE: &str = "node-steward";

pub fn sk(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}
pub fn pk_hex(k: &SigningKey) -> String {
    hex_encode(&k.verifying_key().to_bytes())
}
pub fn operator() -> SigningKey {
    sk(1)
}
pub fn steward() -> SigningKey {
    sk(21)
}
pub fn mesh() -> MeshId {
    MeshId::derive(&[9; 32], &[7; 32])
}
pub fn other_mesh() -> MeshId {
    MeshId::derive(&[9; 32], &[8; 32])
}

/// A stub licence: cog id to outcome; `"*"` is the fallback.
#[derive(Default, Clone)]
pub struct StubLicence(pub Arc<Mutex<BTreeMap<String, Result<Entitlement, LicenceCheckError>>>>);

impl StubLicence {
    pub fn all(expires: Option<u64>) -> Self {
        let s = Self::default();
        s.set("*", Ok(Entitlement { ref_sha256: sha256_hex(b"acct"), expires }));
        s
    }
    pub fn set(&self, cog: &str, r: Result<Entitlement, LicenceCheckError>) {
        self.0.lock().unwrap().insert(cog.into(), r);
    }
}

impl LicenceProvider for StubLicence {
    fn entitlement(&self, cog: &str, _mesh: &str, _now: u64) -> Result<Entitlement, LicenceCheckError> {
        let m = self.0.lock().unwrap();
        m.get(cog).or_else(|| m.get("*")).cloned().unwrap_or(Err(LicenceCheckError::Unlicensed))
    }
}

/// A stub registry: (cog, arch) to bytes, one version per cog.
#[derive(Clone, Default)]
pub struct StubFetcher {
    pub cogs: Arc<Mutex<BTreeMap<(String, String), Vec<u8>>>>,
    pub version: Arc<Mutex<String>>,
    pub fetches: Arc<AtomicU64>,
    /// When set, `fetch` waits for this channel before returning.
    pub gate: Arc<Mutex<Option<std::sync::mpsc::Receiver<()>>>>,
}

impl StubFetcher {
    pub fn with(cogs: &[(&str, &str, &[u8])]) -> Self {
        let f = Self::default();
        *f.version.lock().unwrap() = "1.0.0".into();
        for (c, a, b) in cogs {
            f.cogs.lock().unwrap().insert((c.to_string(), a.to_string()), b.to_vec());
        }
        f
    }
    pub fn fetch_count(&self) -> u64 {
        self.fetches.load(Ordering::SeqCst)
    }
}

impl CogFetcher for StubFetcher {
    fn resolve(&self, cog: &str, version: &str) -> Result<CogEntry, FetchError> {
        let v = self.version.lock().unwrap().clone();
        let cogs = self.cogs.lock().unwrap();
        let arts: Vec<EntryArtifact> = cogs
            .iter()
            .filter(|((c, _), _)| c == cog)
            .map(|((_, a), b)| EntryArtifact { arch: a.clone(), size: b.len() as u64, sha256: sha256_hex(b) })
            .collect();
        if arts.is_empty() {
            return Err(FetchError::NotFound);
        }
        if version != "latest" && version != v {
            return Err(FetchError::VersionUnavailable(v));
        }
        Ok(CogEntry {
            cog_id: cog.into(),
            version: v,
            registry: "https://registry.example/app-registry.json".into(),
            manifest_sha256: sha256_hex(format!("entry-{cog}").as_bytes()),
            artifacts: arts,
        })
    }
    fn fetch(&self, entry: &CogEntry, arch: &str) -> Result<Vec<u8>, FetchError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        if let Some(rx) = self.gate.lock().unwrap().take() {
            let _ = rx.recv();
        }
        self.cogs
            .lock()
            .unwrap()
            .get(&(entry.cog_id.clone(), arch.to_string()))
            .cloned()
            .ok_or_else(|| FetchError::ArchUnavailable(arch.into()))
    }
}

pub struct Harness {
    pub dir: TempDir,
    pub svc: Arc<Service>,
    pub clock: Arc<AtomicU64>,
    pub licence: StubLicence,
    pub fetcher: StubFetcher,
    pub cfg: Config,
}

static NONCE: AtomicU64 = AtomicU64::new(1);

pub fn base_config(dir: &Path) -> Config {
    Config {
        state_dir: dir.join("state"),
        device_id: "seed-test".into(),
        operator_pubkeys: vec![pk_hex(&operator())],
        listen: vec!["127.0.0.1:0".parse().unwrap()],
        ..Config::default()
    }
}

pub fn binding(seq: u64, state: BindState, grant_pk: &str, m: &MeshId) -> weft_licence_wire::SignedBinding {
    sign_binding(
        &BindingRecord {
            v: 2,
            device_id: "seed-test".into(),
            device_pubkey: pk_hex(&sk(20)),
            mesh_id: m.to_hex(),
            grant_pubkey: grant_pk.into(),
            steward_node_id: NODE.into(),
            steward_pubkey: pk_hex(&steward()),
            state,
            seq,
            bound_at: T0,
        },
        &operator(),
    )
    .unwrap()
}

impl Harness {
    pub fn new(cogs: &[(&str, &str, &[u8])]) -> Self {
        Self::with(cogs, |_| {})
    }

    pub fn with(cogs: &[(&str, &str, &[u8])], tweak: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = base_config(dir.path());
        tweak(&mut cfg);
        let init = keys::init(&cfg.state_dir).unwrap();
        let ops = OperatorKeys::load(&cfg.state_dir, &cfg.operator_pubkeys).unwrap();
        bind::apply(&cfg.state_dir, &cfg.device_id, &ops, &binding(1, BindState::Bound, &init.grant_pubkey, &mesh()), None)
            .unwrap();
        let h = Self::open_dir(dir, cfg, StubLicence::all(None), StubFetcher::with(cogs), T0);
        h
    }

    pub fn open_dir(dir: TempDir, cfg: Config, licence: StubLicence, fetcher: StubFetcher, now: u64) -> Self {
        let clock = Arc::new(AtomicU64::new(now));
        let c = clock.clone();
        let svc = Service::open(
            cfg.clone(),
            Arc::new(move || c.load(Ordering::SeqCst)),
            Box::new(licence.clone()),
            Box::new(fetcher.clone()),
            Box::new(StubDeviceSigner),
        )
        .unwrap();
        Self { dir, svc: Arc::new(svc), clock, licence, fetcher, cfg }
    }

    /// Reopen the same state dir with a fresh service (a restart or crash).
    pub fn reopen(self) -> Self {
        let Harness { dir, svc, clock, licence, fetcher, cfg, .. } = self;
        drop(svc);
        let now = clock.load(Ordering::SeqCst);
        Self::open_dir(dir, cfg, licence, fetcher, now)
    }

    pub fn now(&self) -> u64 {
        self.clock.load(Ordering::SeqCst)
    }
    pub fn advance(&self, secs: u64) {
        self.clock.fetch_add(secs, Ordering::SeqCst);
    }

    /// A request signed by the steward with a fresh nonce.
    pub fn signed(&self, method: &str, target: &str, body: &[u8]) -> Request {
        self.signed_with(&steward(), method, target, body, self.now() * 1000)
    }

    pub fn signed_with(&self, key: &SigningKey, method: &str, target: &str, body: &[u8], ts: u64) -> Request {
        let n = NONCE.fetch_add(1, Ordering::SeqCst);
        let nonce = format!("{:032x}", n);
        let headers = sign_request(key, NODE, method, target, body, ts, &nonce);
        Request { method: method.into(), target: target.into(), headers, body: body.to_vec() }
    }

    pub fn call(&self, method: &str, target: &str, body: &[u8]) -> Response {
        self.svc.handle(&self.signed(method, target, body))
    }

    pub fn checkout(&self, cog: &str, arch: &str) -> Response {
        let b = serde_json::json!({"request_id": "r1", "cog_id": cog, "version": "latest", "arch": arch});
        self.call("POST", "/licence/v1/checkout", b.to_string().as_bytes())
    }

    pub fn unsigned(&self, method: &str, target: &str) -> Response {
        self.svc.handle(&Request { method: method.into(), target: target.into(), ..Request::default() })
    }

    pub fn grant_key(&self) -> [u8; 32] {
        keys::load(&self.cfg.state_dir).unwrap().verifying_key().to_bytes()
    }
}

pub fn status(r: &Response) -> u16 {
    r.status
}

pub fn code(r: &Response) -> String {
    r.json_body().and_then(|v| v["error"].as_str().map(String::from)).unwrap_or_default()
}

pub fn grant_of(r: &Response) -> weft_licence_wire::SignedGrant {
    serde_json::from_value(r.json_body().expect("json")["grant"].clone()).expect("grant")
}
