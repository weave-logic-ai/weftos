//! Harness for the Phase 3 end-to-end test: the real machine mesh service
//! (package S) on tempdirs, and user daemons linked to it through the real
//! daemon glue (package U). Everything runs as the current user; a second uid
//! is injected into the service's peer source and into the client's view of
//! itself, exactly as S's own tests do. No root, no /var, /etc, launchd or
//! systemd; nothing reads the real `$HOME`.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use clawft_kernel::error::KernelResult;
use clawft_kernel::ipc::KernelMessage;
use clawft_kernel::mesh_delivery::{LocalDelivery, PeerCtx};
use clawft_kernel::mesh_ipc::Scope;
use clawft_mesh_local::client::RegisterParams;
use clawft_mesh_local::peer::{InjectedPeer, PeerIdentity, UnixPeer, own_uid};
use clawft_mesh_local::proto::{Message, Role, ServiceRecord};
use clawft_mesh_local::{ClientConfig, node_id_from_pubkey};
use clawft_mesh_service::admin_client::{AdminClient, ConnectConfig};
use clawft_mesh_service::limits::LimitConfig;
use clawft_mesh_service::local_server::PeerSource;
use clawft_mesh_service::{MeshServiceConfig, RunningService, start_with};
use clawft_types::config::{MeshConfig, MeshServicePolicy};
use clawft_weave::mesh_local_chain::{ChainQueue, ChainSink};
use clawft_weave::mesh_local_glue::{
    LinkDeps, LinkHandle, Resolved, ServiceEndpoint, Timings, build_endpoint, resolve, spawn,
};
use clawft_weave::mesh_state::MeshStateCell;
use ed25519_dalek::SigningKey;
use serde_json::Value;

/// Sentinel: the service reads the connection's real peer credential.
pub const REAL: u32 = u32::MAX;
/// The injected second account.
pub const OTHER_UID: u32 = 9001;
/// Cluster genesis label, so `observe` admission runs the crypto gate.
pub const GENESIS: [u8; 32] = [0x47; 32];

/// The real service on tempdirs.
pub struct Svc {
    pub dir: tempfile::TempDir,
    pub cfg: MeshServiceConfig,
    pub limits: LimitConfig,
    pub svc: Option<RunningService>,
    pub next_uid: Arc<AtomicU32>,
    pub euid: u32,
}

fn peer_source(next: Arc<AtomicU32>) -> PeerSource {
    Arc::new(move |s: &tokio::net::UnixStream| {
        let n = next.load(Ordering::SeqCst);
        if n == REAL {
            UnixPeer::from_stream(s).map(|p| Arc::new(p) as Arc<dyn PeerIdentity>)
        } else {
            Ok(Arc::new(InjectedPeer::uid(n)) as Arc<dyn PeerIdentity>)
        }
    })
}

impl Svc {
    pub async fn start() -> Self {
        Self::with_limits(LimitConfig::default()).await
    }

    pub async fn with_limits(limits: LimitConfig) -> Self {
        let euid = own_uid().await.expect("own uid");
        // Short root: unix socket paths are length-limited.
        let dir = tempfile::Builder::new().prefix("x").tempdir().expect("tempdir");
        let cfg = MeshServiceConfig {
            state_dir: dir.path().join("st"),
            socket: dir.path().join("r").join("s"),
            listen: "127.0.0.1:0".into(),
            health_listen: None,
            probe_facts: false,
            admin_uids: vec![euid],
            genesis_hash: Some(GENESIS),
            build_sha: "e2e-service".into(),
            ..MeshServiceConfig::default()
        };
        let mut s = Self { dir, cfg, limits, svc: None, next_uid: Arc::new(AtomicU32::new(REAL)), euid };
        s.begin().await;
        s
    }

    pub async fn begin(&mut self) {
        let svc = start_with(self.cfg.clone(), peer_source(self.next_uid.clone()), self.limits)
            .await
            .expect("service starts");
        self.svc = Some(svc);
    }

    pub async fn stop(&mut self) {
        if let Some(s) = self.svc.take() {
            s.shutdown().await;
        }
    }

    pub fn running(&self) -> &RunningService {
        self.svc.as_ref().expect("service running")
    }

    pub fn node_id(&self) -> String {
        self.running().node_id.clone()
    }

    pub fn socket(&self) -> PathBuf {
        self.cfg.socket.clone()
    }

    pub fn record(&self) -> ServiceRecord {
        ServiceRecord::load(&self.cfg.socket.parent().unwrap().join("service.json")).expect("service.json")
    }

    /// The daemon's mesh config pointing at this service.
    pub fn mesh_cfg(&self, policy: MeshServicePolicy) -> MeshConfig {
        MeshConfig {
            enabled: true,
            service: policy,
            service_socket: Some(self.socket().display().to_string()),
            ..MeshConfig::default()
        }
    }

    /// Admin request that must succeed; the reply data (`Null` for an ack).
    pub async fn admin(&self, m: Message) -> Value {
        self.next_uid.store(REAL, Ordering::SeqCst);
        let mut a = AdminClient::connect(&ConnectConfig::new(self.socket(), self.record(), Role::Admin))
            .await
            .expect("admin connects");
        match a.request(m).await.expect("admin request") {
            Message::Reply { data } => data,
            Message::Ack {} => Value::Null,
            other => panic!("unexpected admin reply {other:?}"),
        }
    }

    /// Lines of the machine journal with `kind`.
    pub fn journal(&self, kind: &str) -> Vec<Value> {
        let text = std::fs::read_to_string(self.cfg.state_dir.join("journal.jsonl")).unwrap_or_default();
        text.lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|r| r["kind"] == kind)
            .collect()
    }
}

/// Everything the daemon's router would see of one inbound delivery.
#[derive(Debug, Clone)]
pub struct Got {
    pub peer_id: String,
    pub scope: Option<Scope>,
    pub src_scope: Option<Scope>,
    pub msg: KernelMessage,
}

/// Stands in for the daemon's `A2ARouter`.
#[derive(Default)]
pub struct Inbox(pub Mutex<Vec<Got>>);

#[async_trait]
impl LocalDelivery for Inbox {
    async fn deliver(&self, from: &PeerCtx, scope: Option<&Scope>, msg: KernelMessage) -> KernelResult<()> {
        self.0.lock().unwrap().push(Got {
            peer_id: from.peer_id.clone(),
            scope: scope.cloned(),
            src_scope: from.src_scope.clone(),
            msg,
        });
        Ok(())
    }
}

/// Stands in for the user chain.
#[derive(Clone, Default)]
pub struct Chain(pub Arc<Mutex<Vec<(String, Value)>>>);

impl ChainSink for Chain {
    fn append(&self, kind: &str, payload: Value) -> Result<(), String> {
        self.0.lock().unwrap().push((kind.to_owned(), payload));
        Ok(())
    }
}

impl Chain {
    pub fn of(&self, kind: &str) -> Vec<Value> {
        self.0.lock().unwrap().iter().filter(|(k, _)| k == kind).map(|(_, v)| v.clone()).collect()
    }
}

/// One user daemon's mesh side.
pub struct Daemon {
    pub handle: Option<LinkHandle>,
    pub state: Arc<MeshStateCell>,
    pub inbox: Arc<Inbox>,
    pub chain: Chain,
    pub user_id: String,
    pub node_id: String,
}

impl Daemon {
    pub fn received(&self) -> Vec<Got> {
        self.inbox.0.lock().unwrap().clone()
    }

    pub fn link_state(&self) -> Option<String> {
        self.state.get().and_then(|s| s.state)
    }

    pub async fn shutdown(&mut self) {
        if let Some(h) = self.handle.take() {
            h.shutdown().await;
        }
    }
}

pub fn fast() -> Timings {
    Timings {
        anchor_every: Duration::from_millis(100),
        backoff: (Duration::from_millis(20), Duration::from_millis(100)),
        deadline: Duration::from_secs(2),
    }
}

/// Resolve and link a daemon. The caller has set the service's next uid.
pub async fn link(cfg: &MeshConfig, ep: ServiceEndpoint) -> Daemon {
    let user_id = node_id_from_pubkey(&ep.user_key.verifying_key().to_bytes());
    let link = match resolve(cfg, Ok(Some(ep))).await.expect("resolves") {
        Resolved::Service(l) => l,
        _ => panic!("expected service mode"),
    };
    let node_id = link.node_id().to_owned();
    let state = Arc::new(MeshStateCell::new());
    let inbox = Arc::new(Inbox::default());
    let chain = Chain::default();
    let handle = spawn(
        link,
        LinkDeps {
            delivery: inbox.clone(),
            gate: None,
            chain: Arc::new(ChainQueue::new(chain.clone())),
            state: state.clone(),
            timings: fast(),
        },
    );
    Daemon { handle: Some(handle), state, inbox, chain, user_id, node_id }
}

/// Daemon A: the current user, endpoint built by the production builder from
/// a temp home (user key created there, machine key pinned there).
pub async fn daemon_as_me(svc: &Svc, home: &Path) -> Daemon {
    svc.next_uid.store(REAL, Ordering::SeqCst);
    let cfg = svc.mesh_cfg(MeshServicePolicy::Required);
    let ep = build_endpoint(&cfg, home, "e2e-daemon").expect("endpoint").expect("socket present");
    link(&cfg, ep).await
}

/// An endpoint for the injected second account with `key`.
pub fn other_endpoint(svc: &Svc, key: SigningKey, accept_from: Vec<String>) -> ServiceEndpoint {
    let mut client = ClientConfig::new(svc.socket(), svc.record());
    client.server_peer = Some(Arc::new(InjectedPeer::uid(svc.euid)));
    client.own_uid = Some(OTHER_UID);
    client.deadline = Duration::from_secs(2);
    client.build_sha = "e2e-daemon-b".into();
    let user_id = node_id_from_pubkey(&key.verifying_key().to_bytes());
    ServiceEndpoint {
        client,
        user_key: key,
        register: RegisterParams {
            topic_prefixes: vec![format!("user/{user_id}/")],
            capabilities: vec!["a2a".into()],
            version: "e2e".into(),
            accept_from,
            ..RegisterParams::default()
        },
    }
}

/// Daemon B: the injected second account.
pub async fn daemon_as_other(svc: &Svc, key: SigningKey, accept_from: Vec<String>) -> Daemon {
    svc.next_uid.store(OTHER_UID, Ordering::SeqCst);
    let cfg = svc.mesh_cfg(MeshServicePolicy::Required);
    let d = link(&cfg, other_endpoint(svc, key, accept_from)).await;
    svc.next_uid.store(REAL, Ordering::SeqCst);
    d
}

pub fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for: {what}");
}
