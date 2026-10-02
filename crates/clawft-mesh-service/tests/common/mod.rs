//! Shared harness: a real service on tempdirs (state, socket, loopback mesh
//! port), run as the current user. A second uid is injected into the peer
//! source; the real peer credential is used when no uid is injected.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clawft_mesh_local::client::{ClientConfig, ClientError, MeshLocalClient, RegisterParams};
use clawft_mesh_local::framing::{write_frame, FrameReader};
use clawft_mesh_local::peer::{own_uid, InjectedPeer, PeerIdentity, UnixPeer};
use clawft_mesh_local::proto::{ErrorKind, Frame, Message, Role, ServiceRecord};
use clawft_mesh_service::admin_client::{AdminClient, AdminError, ConnectConfig};
use clawft_mesh_service::limits::LimitConfig;
use clawft_mesh_service::local_server::PeerSource;
use clawft_mesh_service::{start_with, MeshServiceConfig, RunningService, StartError};
use ed25519_dalek::SigningKey;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

/// Sentinel: use the connection's real peer credential.
pub const REAL: u32 = u32::MAX;

pub struct Harness {
    pub dir: tempfile::TempDir,
    pub svc: Option<RunningService>,
    pub next_uid: Arc<AtomicU32>,
    pub cfg: MeshServiceConfig,
    pub limits: LimitConfig,
    pub euid: u32,
}

pub fn peer_source(next: Arc<AtomicU32>) -> PeerSource {
    Arc::new(move |s: &UnixStream| {
        let n = next.load(Ordering::SeqCst);
        if n == REAL {
            UnixPeer::from_stream(s).map(|p| Arc::new(p) as Arc<dyn PeerIdentity>)
        } else {
            Ok(Arc::new(InjectedPeer::uid(n)) as Arc<dyn PeerIdentity>)
        }
    })
}

/// A config rooted in `root` (short paths: unix socket paths are limited).
pub fn config_in(root: &Path, euid: u32) -> MeshServiceConfig {
    MeshServiceConfig {
        state_dir: root.join("st"),
        socket: root.join("r").join("s"),
        listen: "127.0.0.1:0".into(),
        health_listen: Some("127.0.0.1:0".into()),
        probe_facts: false,
        admin_uids: vec![euid],
        build_sha: "test-sha".into(),
        ..MeshServiceConfig::default()
    }
}

impl Harness {
    pub async fn start() -> Self {
        Self::with(|_, _| {}).await
    }

    pub async fn with(f: impl FnOnce(&mut MeshServiceConfig, &mut LimitConfig)) -> Self {
        let euid = own_uid().await.expect("own uid");
        let dir = tempfile::Builder::new().prefix("m").tempdir().expect("tempdir");
        let mut cfg = config_in(dir.path(), euid);
        let mut limits = LimitConfig::default();
        f(&mut cfg, &mut limits);
        let mut h = Self {
            dir,
            svc: None,
            next_uid: Arc::new(AtomicU32::new(REAL)),
            cfg,
            limits,
            euid,
        };
        h.begin().await.expect("service starts");
        h
    }

    pub async fn begin(&mut self) -> Result<(), StartError> {
        let svc = start_with(self.cfg.clone(), peer_source(self.next_uid.clone()), self.limits).await?;
        self.svc = Some(svc);
        Ok(())
    }

    pub async fn stop(&mut self) {
        if let Some(s) = self.svc.take() {
            s.shutdown().await;
        }
    }

    pub async fn restart(&mut self) {
        self.stop().await;
        self.begin().await.expect("service restarts");
    }

    pub fn svc(&self) -> &RunningService {
        self.svc.as_ref().expect("running")
    }

    pub fn socket(&self) -> PathBuf {
        self.cfg.socket.clone()
    }

    pub fn state_dir(&self) -> PathBuf {
        self.cfg.state_dir.clone()
    }

    pub fn record(&self) -> ServiceRecord {
        ServiceRecord::load(&self.cfg.socket.parent().unwrap().join("service.json")).expect("service.json")
    }

    /// Connect and register as `uid` (`None` = the real credential) with the
    /// user key derived from `seed`.
    pub async fn connect(&self, uid: Option<u32>, seed: u8, params: RegisterParams) -> Result<MeshLocalClient, ClientError> {
        let mut cc = ClientConfig::new(self.socket(), self.record());
        cc.build_sha = "client-sha".into();
        match uid {
            None => self.next_uid.store(REAL, Ordering::SeqCst),
            Some(u) => {
                self.next_uid.store(u, Ordering::SeqCst);
                cc.server_peer = Some(Arc::new(InjectedPeer::uid(self.euid)));
                cc.own_uid = Some(u);
            }
        }
        MeshLocalClient::connect_and_register(&cc, &key(seed), &params).await
    }

    /// `connect`, retrying while the previous registration is still closing.
    pub async fn connect_retry(&self, uid: Option<u32>, seed: u8, params: RegisterParams) -> Result<MeshLocalClient, ClientError> {
        let mut last = None;
        for _ in 0..60 {
            match self.connect(uid, seed, params.clone()).await {
                Err(ClientError::Server(e)) if e.kind == ErrorKind::AddressInUse => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    last = Some(ClientError::Server(e));
                }
                other => return other,
            }
        }
        Err(last.unwrap())
    }

    pub async fn admin(&self) -> Result<AdminClient, AdminError> {
        self.next_uid.store(REAL, Ordering::SeqCst);
        AdminClient::connect(&ConnectConfig::new(self.socket(), self.record(), Role::Admin)).await
    }

    /// Admin request that must succeed; returns the reply data (`Null` for an ack).
    pub async fn admin_ok(&self, m: Message) -> serde_json::Value {
        let mut a = self.admin().await.expect("admin connects");
        match a.request(m).await.expect("admin request") {
            Message::Reply { data } => data,
            Message::Ack {} => serde_json::Value::Null,
            other => panic!("unexpected admin reply {other:?}"),
        }
    }

    pub async fn admin_err(&self, m: Message) -> ErrorKind {
        let mut a = self.admin().await.expect("admin connects");
        match a.request(m).await {
            Err(AdminError::Server(e)) => e.kind.clone(),
            other => panic!("expected a server error, got {other:?}"),
        }
    }
}

pub fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub fn pubkey_hex(seed: u8) -> String {
    clawft_mesh_local::hexser::encode(&key(seed).verifying_key().to_bytes())
}

pub fn user_id(seed: u8) -> String {
    clawft_mesh_local::node_id_from_pubkey(&key(seed).verifying_key().to_bytes())
}

pub fn server_kind(r: Result<MeshLocalClient, ClientError>) -> ErrorKind {
    match r {
        Err(ClientError::Server(e)) => e.kind,
        Err(other) => panic!("expected a server error, got {other}"),
        Ok(_) => panic!("expected a server error, but the call succeeded"),
    }
}

/// A raw mesh-local connection (no verification) for protocol-level tests.
pub struct Raw {
    pub rd: FrameReader<OwnedReadHalf>,
    pub wr: OwnedWriteHalf,
}

impl Raw {
    pub async fn connect(path: &Path) -> Raw {
        let (rd, wr) = UnixStream::connect(path).await.expect("raw connect").into_split();
        Raw { rd: FrameReader::new(rd), wr }
    }

    pub async fn send(&mut self, m: Message) {
        write_frame(&mut self.wr, &Frame::new(m)).await.expect("raw send");
    }

    pub async fn recv(&mut self) -> Option<Frame> {
        self.rd.read_frame_within(Duration::from_secs(3)).await.ok().flatten()
    }
}

pub fn hello(role: Role, proto_min: u32, proto_max: u32) -> Message {
    Message::Hello {
        proto_min,
        proto_max,
        features: vec![],
        role,
        build_sha: "raw".into(),
        exe: String::new(),
        pid: std::process::id(),
        client_nonce: [7; 32],
    }
}
