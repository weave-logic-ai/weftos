//! A stub `weft-licence` and an in-process three-node mesh for the relay
//! tests. The stub implements the request and grant contract of ADR-106
//! sections 4 and 7: signed requests only (identity excepted), the steward's
//! rate budget charged after its signature verifies, a small separate pool
//! for unsigned and refused callers, durable `seq`, and signed grants.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use super::client::*;
use super::request::*;
use super::tests_common::*;
use super::*;
use crate::artifact_store::ArtifactStore;
use crate::error::{KernelError, KernelResult};
use crate::gate::{GateBackend, GateDecision};
use crate::ipc::KernelMessage;
use crate::mesh_admit::PeerClass;
use crate::mesh_artifact::{ArtifactExchange, ExchangeConfig};
use crate::mesh_artifact_tunnel::PeerSender;
use crate::mesh_cog::CogMesh;
use crate::mesh_delivery::PeerCtx;

pub(super) const STEWARD_NODE: &str = "node-steward";
/// The request clock the tests use, past the COG-011 floor.
pub(super) const REQUEST_NOW_MS: u64 = super::tests_request::NOW_MS;
pub(super) const STEWARD_BUDGET: u32 = 20;
pub(super) const UNSIGNED_POOL: u32 = 30;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Good,
    WrongSigner,
    WrongBytes,
    SeedAhead,
    /// The grant commits to a registry sha256 the bytes do not have.
    WrongSha,
}

struct State {
    replay: ReplayGuard,
    steward_left: u32,
    unsigned_left: u32,
    seq: u64,
    arches: BTreeSet<String>,
}

pub(super) struct StubLicence {
    pub mode: Mutex<Mode>,
    pub checkouts: AtomicU32,
    pub byte_transfers: AtomicU32,
    pub clock: Arc<AtomicU64>,
    pub delay: Duration,
    st: Mutex<State>,
}

pub(super) fn bytes_of(arch: &str) -> Vec<u8> {
    format!("bin-{arch}").into_bytes()
}

impl StubLicence {
    pub fn new(clock: Arc<AtomicU64>, delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            mode: Mutex::new(Mode::Good),
            checkouts: AtomicU32::new(0),
            byte_transfers: AtomicU32::new(0),
            clock,
            delay,
            st: Mutex::new(State {
                replay: ReplayGuard::default(),
                steward_left: STEWARD_BUDGET,
                unsigned_left: UNSIGNED_POOL,
                seq: 0,
                arches: BTreeSet::new(),
            }),
        })
    }

    pub fn set_mode(&self, m: Mode) {
        *self.mode.lock().unwrap() = m;
    }

    pub fn steward_budget_left(&self) -> u32 {
        self.st.lock().unwrap().steward_left
    }

    pub fn unsigned_left(&self) -> u32 {
        self.st.lock().unwrap().unsigned_left
    }

    fn reply(status: u16, v: serde_json::Value) -> Result<LicenceResponse, LicenceClientError> {
        Ok(LicenceResponse { status, body: serde_json::to_vec(&v).unwrap() })
    }

    fn mint(&self, wire: &CheckoutWire) -> Result<LicenceResponse, LicenceClientError> {
        let now = self.clock.load(Ordering::SeqCst);
        let mode = *self.mode.lock().unwrap();
        let (seq, arches) = {
            let mut st = self.st.lock().unwrap();
            st.seq += 1;
            st.arches.insert(wire.arch.clone());
            (st.seq, st.arches.iter().cloned().collect::<Vec<_>>())
        };
        let issued = if mode == Mode::SeedAhead { now + 3600 } else { now };
        let names: Vec<&str> = arches.iter().map(String::as_str).collect();
        let mut rec = grant_rec(seq, issued, 72 * 3600, &names);
        for a in &mut rec.artifacts {
            a.size = bytes_of(&a.arch).len() as u64;
            if mode == Mode::WrongSha {
                a.sha256 = sha256_hex(b"not these bytes");
            }
        }
        let key = if mode == Mode::WrongSigner { sk(77) } else { grant_key() };
        let signed = sign_grant(&rec, &key).unwrap();
        Self::reply(200, json!({ "grant": signed }))
    }

    pub fn handle(&self, req: LicenceRequest) -> Result<LicenceResponse, LicenceClientError> {
        if req.path == "/licence/v1/identity" {
            return Self::reply(200, json!({ "device_id": "seed-test" }));
        }
        let steward = sk(21).verifying_key().to_bytes();
        {
            let mut st = self.st.lock().unwrap();
            if let Err(why) = verify_request(&req, &steward, STEWARD_NODE, "seed-test", REQUEST_NOW_MS, &mut st.replay) {
                // Refused callers share the small pool; the steward's budget
                // is untouched.
                if st.unsigned_left == 0 {
                    return Self::reply(429, json!({ "error": "rate_limited" }));
                }
                st.unsigned_left -= 1;
                let code = match why {
                    RequestRefused::Unsigned => "unsigned",
                    _ => "bad_request_signature",
                };
                return Self::reply(401, json!({ "error": code }));
            }
            if st.steward_left == 0 {
                return Self::reply(429, json!({ "error": "rate_limited" }));
            }
            st.steward_left -= 1;
        }
        match (req.method.as_str(), req.path.as_str()) {
            ("POST", CHECKOUT_PATH) => {
                let wire: CheckoutWire = serde_json::from_slice(&req.body).unwrap();
                if wire.cog_id != "fall-detect" {
                    return Self::reply(403, json!({ "error": "cog_unlicensed" }));
                }
                self.checkouts.fetch_add(1, Ordering::SeqCst);
                self.mint(&wire)
            }
            ("GET", p) if p.starts_with(ARTIFACT_PATH) => {
                let id = &p[ARTIFACT_PATH.len()..];
                let arch = ["aarch64", "x86_64", "armv7"].into_iter().find(|a| b3_of(a) == id);
                let Some(arch) = arch else {
                    return Self::reply(404, json!({ "error": "unknown_artifact" }));
                };
                self.byte_transfers.fetch_add(1, Ordering::SeqCst);
                let mut b = bytes_of(arch);
                if *self.mode.lock().unwrap() == Mode::WrongBytes {
                    b[0] ^= 0xff;
                }
                Ok(LicenceResponse { status: 200, body: b })
            }
            ("GET", p) if p.starts_with(GRANTS_PATH) => Self::reply(200, json!({ "grants": [] })),
            _ => Self::reply(404, json!({ "error": "not_found" })),
        }
    }
}

pub(super) struct StubLink(pub Arc<StubLicence>);

#[async_trait]
impl LicenceTransport for StubLink {
    async fn call(&self, req: LicenceRequest) -> Result<LicenceResponse, LicenceClientError> {
        if !self.0.delay.is_zero() {
            tokio::time::sleep(self.0.delay).await;
        }
        self.0.handle(req)
    }
}

pub(super) fn steward_client(stub: &Arc<StubLicence>, _clock: &Arc<AtomicU64>) -> Arc<dyn LicenceClient> {
    SignedLicenceClient::new(sk(21), STEWARD_NODE, "seed-test", StubLink(stub.clone()), Arc::new(|| REQUEST_NOW_MS))
}

/// What a test gate answers.
pub(super) struct TestGate {
    pub permit: bool,
    pub asked: Mutex<Vec<(String, String)>>,
}

impl TestGate {
    pub fn new(permit: bool) -> Arc<Self> {
        Arc::new(Self { permit, asked: Mutex::default() })
    }
}

impl GateBackend for TestGate {
    fn check(&self, agent_id: &str, action: &str, _: &serde_json::Value) -> GateDecision {
        self.asked.lock().unwrap().push((agent_id.into(), action.into()));
        if self.permit {
            GateDecision::Permit { token: None }
        } else {
            GateDecision::Deny { reason: "test gate".into(), receipt: None }
        }
    }
}

// ── the in-process mesh ──────────────────────────────────────────

pub(super) struct Member {
    pub fx: Fx,
    pub ex: Arc<ArtifactExchange>,
    pub mesh: Arc<CogMesh>,
}

/// How the net stamps what a node sends (the service's origin, simulated).
#[derive(Clone, Copy)]
pub(super) enum Stamp {
    Admitted,
    Leaf,
    Unadmitted,
}

#[derive(Default)]
pub(super) struct Net {
    pub members: Mutex<HashMap<String, Arc<Member>>>,
    pub log: Mutex<Vec<(String, String, String)>>,
    pub stamps: Mutex<HashMap<String, Stamp>>,
}

pub(super) struct NetSender {
    pub net: Arc<Net>,
    pub me: String,
}

fn ctx(me: &str, s: Stamp) -> PeerCtx {
    match s {
        Stamp::Admitted => PeerCtx {
            peer_id: me.into(),
            node_verified: true,
            class: PeerClass::Node,
            remote_static: None,
            src_scope: None,
        },
        Stamp::Leaf => PeerCtx {
            peer_id: me.into(),
            node_verified: true,
            class: PeerClass::Leaf,
            remote_static: None,
            src_scope: None,
        },
        Stamp::Unadmitted => PeerCtx::unauthenticated(me),
    }
}

#[async_trait]
impl PeerSender for NetSender {
    async fn send_to_node(&self, node: &str, msg: KernelMessage) -> KernelResult<()> {
        let topic = match &msg.target {
            crate::ipc::MessageTarget::Topic(t) => t.clone(),
            _ => String::new(),
        };
        self.net.log.lock().unwrap().push((self.me.clone(), node.to_owned(), topic));
        let target = self.net.members.lock().unwrap().get(node).cloned();
        let stamp = self.net.stamps.lock().unwrap().get(&self.me).copied().unwrap_or(Stamp::Admitted);
        match target {
            // Inline and awaited: one ordered link per direction.
            Some(t) => {
                t.mesh.on_delivery(&ctx(&self.me, stamp), msg).await;
                Ok(())
            }
            None => Err(KernelError::Mesh(format!("peer {node} is not connected"))),
        }
    }
}

/// Floods a new grant by installing it on every other member directly.
pub(super) struct NetFlood {
    pub net: Arc<Net>,
    pub from: String,
    pub floods: AtomicU32,
}

#[async_trait]
impl GrantFlood for NetFlood {
    async fn flood(&self, grant: &SignedGrant) {
        self.floods.fetch_add(1, Ordering::SeqCst);
        let others: Vec<Arc<Member>> = self
            .net
            .members
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| **id != self.from)
            .map(|(_, m)| m.clone())
            .collect();
        for m in others {
            let _ = install_grant(&m.fx.store, &m.ex, grant);
        }
    }
}

pub(super) fn exchange_for(id: &str, fx: &Fx) -> Arc<ArtifactExchange> {
    let cfg = ExchangeConfig {
        redistribution: Arc::new(MeshCheckoutPolicy::new(fx.store.clone())),
        recv_timeout: Duration::from_millis(400),
        ..ExchangeConfig::default()
    };
    Arc::new(ArtifactExchange::new(id, Arc::new(ArtifactStore::new_memory()), cfg).unwrap())
}

/// Add a bound member `id`. The steward passes a relay builder.
pub(super) fn add_member(
    net: &Arc<Net>,
    id: &str,
    relay: impl FnOnce(&Fx, &Arc<ArtifactExchange>) -> Option<Arc<CheckoutRelay>>,
) -> Arc<Member> {
    let fx = Fx::new();
    fx.bind();
    let ex = exchange_for(id, &fx);
    let sender: Arc<dyn PeerSender> = Arc::new(NetSender { net: net.clone(), me: id.into() });
    let r = relay(&fx, &ex);
    let mesh = CogMesh::new(ex.clone(), fx.store.clone(), sender, r);
    let m = Arc::new(Member { fx, ex, mesh });
    net.members.lock().unwrap().insert(id.into(), m.clone());
    m
}

pub(super) fn wire(arch: &str) -> CheckoutWire {
    CheckoutWire {
        request_id: format!("req-{arch}"),
        cog_id: "fall-detect".into(),
        version: "1.2.0".into(),
        arch: arch.into(),
    }
}
