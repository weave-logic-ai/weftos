//! The store owner's side of forwarding: the `cog-store` service that
//! verifies signed [`ForwardRequest`]s and writes them to the owning store,
//! its policy, and how it is served (in-process or mesh TCP).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use ed25519_dalek::SigningKey;

use super::forward::{
    FORWARD_DOMAIN, FORWARD_RESPONSE_DOMAIN, FORWARD_VERSION, ForwardOutcome, ForwardRefusal,
    ForwardRequest, ForwardResponse, MEM_SCHEME, STORE_INGEST_METHOD, STORE_SERVICE, now_ms,
    owner_target,
};
use super::store::{Provenance, StoreDirectory, StoreError};
use super::types::MAX_VECTORS_PER_BATCH;
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh::{MeshError, MeshStream, TransportListener};
use crate::mesh_ipc::MeshIpcEnvelope;
use crate::node_registry::node_id_from_pubkey;
use crate::workload_ctl::msg::{
    MAX_CTL_BYTES, MAX_SKEW_MS, MAX_TTL_MS, NonceGuard, SignedCtl, open, sign,
};
use crate::workload_ctl::session::NoiseStream;
use crate::workload_ctl::transport::CtlConnector;
use crate::workload_pkg::codec::is_lower_hex;


/// Which bridge keys may forward, and for which projects.
pub trait ForwardPolicy: Send + Sync {
    /// True if a batch for `project_id` signed by `key` may be taken.
    fn allows(&self, key: &[u8; 32], project_id: Option<&str>) -> bool;
}

/// Fixed key table. A key is allowed for every project, or for a listed set
/// (a restricted key cannot forward project-less batches).
#[derive(Default)]
pub struct KeyPolicy {
    keys: HashMap<[u8; 32], Option<HashSet<String>>>,
}

impl KeyPolicy {
    /// Empty policy (nothing allowed).
    pub fn new() -> Self {
        Self::default()
    }

    /// Allow `key` for any project and for project-less batches.
    pub fn allow_any(mut self, key: [u8; 32]) -> Self {
        self.keys.insert(key, None);
        self
    }

    /// Allow `key` for the listed projects only.
    pub fn allow_projects(mut self, key: [u8; 32], projects: &[&str]) -> Self {
        self.keys
            .insert(key, Some(projects.iter().map(|p| p.to_string()).collect()));
        self
    }
}

impl ForwardPolicy for KeyPolicy {
    fn allows(&self, key: &[u8; 32], project_id: Option<&str>) -> bool {
        match self.keys.get(key) {
            None => false,
            Some(None) => true,
            Some(Some(set)) => project_id.is_some_and(|p| set.contains(p)),
        }
    }
}

/// A node's `cog-store` service: verifies forwarded batches and writes them
/// to the owning store.
pub struct StoreOwnerService {
    node_id: String,
    key: SigningKey,
    policy: Arc<dyn ForwardPolicy>,
    directory: Arc<dyn StoreDirectory>,
    nonces: NonceGuard,
}

impl StoreOwnerService {
    /// Service for the node whose identity is `key`.
    pub fn new(
        key: SigningKey,
        policy: Arc<dyn ForwardPolicy>,
        directory: Arc<dyn StoreDirectory>,
    ) -> Self {
        Self {
            node_id: node_id_from_pubkey(&key.verifying_key().to_bytes()),
            key,
            policy,
            directory,
            nonces: NonceGuard::new(),
        }
    }

    /// This node's id.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Handle one signed request at `now_ms`. The flag is false when the
    /// sender could not be authenticated (the connection is then dropped).
    pub fn handle_at(&self, signed: &SignedCtl, now_ms: u64) -> (SignedCtl, bool) {
        let (nonce, outcome, authed) = match self.check(signed, now_ms) {
            Ok(req) => {
                let nonce = req.nonce.clone();
                (nonce, self.apply(req), true)
            }
            Err((nonce, code, reason, authed)) => {
                (nonce, ForwardOutcome::Refused { code, reason }, authed)
            }
        };
        let resp = ForwardResponse {
            version: FORWARD_VERSION,
            responder: self.node_id.clone(),
            request_nonce: nonce,
            outcome,
        };
        let payload = serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into());
        (sign(FORWARD_RESPONSE_DOMAIN, payload, &self.key), authed)
    }

    #[allow(clippy::type_complexity)]
    fn check(
        &self,
        s: &SignedCtl,
        now: u64,
    ) -> Result<ForwardRequest, (String, ForwardRefusal, String, bool)> {
        use ForwardRefusal as R;
        let fail = |nonce: &str, c: R, m: &str, authed: bool| {
            (nonce.to_string(), c, m.to_string(), authed)
        };
        let pk = open(FORWARD_DOMAIN, s).map_err(|r| fail("", R::Signature, &r.reason, false))?;
        let req: ForwardRequest = serde_json::from_str(&s.payload)
            .map_err(|_| fail("", R::Signature, "malformed request", false))?;
        let n = req.nonce.clone();
        if req.version != FORWARD_VERSION {
            return Err(fail(&n, R::Invalid, "unsupported version", false));
        }
        if req.requester != node_id_from_pubkey(&pk) {
            return Err(fail(&n, R::Signature, "requester is not the signing key's node", false));
        }
        if req.target != self.node_id {
            return Err(fail(&n, R::NotForMe, "addressed to another node", false));
        }
        if req.expires_at_ms <= now {
            return Err(fail(&n, R::Expired, "request expired", false));
        }
        if req.issued_at_ms > now.saturating_add(MAX_SKEW_MS)
            || req.expires_at_ms < req.issued_at_ms
            || req.expires_at_ms - req.issued_at_ms > MAX_TTL_MS
        {
            return Err(fail(&n, R::Expired, "request lifetime out of bounds", false));
        }
        if !is_lower_hex(&req.nonce, 32) {
            return Err(fail(&n, R::Invalid, "nonce must be 32 hex", false));
        }
        if !self.policy.allows(&pk, req.project_id.as_deref()) {
            return Err(fail(&n, R::Unauthorized, "key may not forward for this project", false));
        }
        if req.vectors.is_empty() || req.vectors.len() > MAX_VECTORS_PER_BATCH {
            return Err(fail(&n, R::Invalid, "vector count out of bounds", true));
        }
        if req.vectors.iter().any(|v| v.values.iter().any(|f| !f.is_finite())) {
            return Err(fail(&n, R::Invalid, "non-finite value", true));
        }
        if !self.nonces.admit(&req.nonce, req.expires_at_ms, now) {
            return Err(fail(&n, R::Replay, "nonce already used", true));
        }
        Ok(req)
    }

    fn apply(&self, req: ForwardRequest) -> ForwardOutcome {
        let refuse = |code, reason: &str| ForwardOutcome::Refused {
            code,
            reason: reason.into(),
        };
        let Some(store) = self.directory.store_for(req.project_id.as_deref()) else {
            return refuse(ForwardRefusal::NoStore, "no store for this project here");
        };
        let from = Provenance {
            instance_id: req.instance_id,
            source_node: req.requester,
        };
        match store.ingest(&from, &req.vectors, req.dedup) {
            Ok(result) => ForwardOutcome::Ok { result },
            Err(StoreError::Full(_)) => refuse(ForwardRefusal::StoreFull, "store full"),
            Err(StoreError::Backend(_)) => refuse(ForwardRefusal::Backend, "store backend error"),
        }
    }
}

/// Serve one connection as the owner's `cog-store` until the peer closes it.
pub async fn serve_connection(
    mut stream: Box<dyn MeshStream>,
    svc: Arc<StoreOwnerService>,
) -> Result<(), MeshError> {
    loop {
        let raw = match stream.recv().await {
            Ok(r) => r,
            Err(MeshError::ConnectionClosed) => return Ok(()),
            Err(e) => return Err(e),
        };
        if raw.len() > MAX_CTL_BYTES + 4096 {
            let _ = stream.close().await;
            return Err(MeshError::Transport("oversize ingest frame".into()));
        }
        let env = MeshIpcEnvelope::from_bytes(&raw)
            .map_err(|e| MeshError::Transport(format!("bad envelope: {e}")))?;
        let signed = match (&env.message.target, &env.message.payload) {
            (MessageTarget::ServiceMethod { service, method }, MessagePayload::Json(v))
                if service == STORE_SERVICE && method == STORE_INGEST_METHOD =>
            {
                serde_json::from_value::<SignedCtl>(v.clone())
                    .map_err(|e| MeshError::Transport(format!("bad signed payload: {e}")))?
            }
            _ => {
                let _ = stream.close().await;
                return Err(MeshError::Transport("not a cog-store request".into()));
            }
        };
        let (resp, authed) = svc.handle_at(&signed, now_ms());
        let mut msg = KernelMessage::new(
            0,
            owner_target(),
            MessagePayload::Json(serde_json::to_value(&resp).unwrap_or_default()),
        );
        msg.correlation_id = env.message.correlation_id.clone();
        let out = MeshIpcEnvelope::new(svc.node_id().to_string(), env.source_node, msg);
        let bytes = out
            .to_bytes()
            .map_err(|e| MeshError::Transport(e.to_string()))?;
        stream.send(&bytes).await?;
        if !authed {
            let _ = stream.close().await;
            return Ok(());
        }
    }
}

/// Most concurrent sessions an owner listener serves.
pub const MAX_SESSIONS: usize = 32;

/// Accept connections on `listener` and serve each as a `cog-store` session
/// (Noise XX responder when `noise` is set). Runs until the listener fails.
pub async fn serve_listener(
    mut listener: Box<dyn TransportListener>,
    svc: Arc<StoreOwnerService>,
    noise: bool,
) -> Result<(), MeshError> {
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_SESSIONS));
    loop {
        let (mut stream, peer) = listener.accept().await?;
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            let _ = stream.close().await;
            continue;
        };
        let svc = svc.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let stream: Box<dyn MeshStream> = if noise {
                let cfg = crate::mesh_noise::NoiseConfig {
                    pattern: crate::mesh_noise::NoisePattern::XX,
                    local_private_key: rand::random(),
                    remote_static_key: None,
                };
                match crate::mesh_noise::NoiseChannel::respond(stream, &cfg).await {
                    Ok(ch) => Box::new(NoiseStream::new(Box::new(ch))),
                    Err(e) => {
                        tracing::warn!(%peer, error = %e, "cog-store noise handshake failed");
                        return;
                    }
                }
            } else {
                stream
            };
            if let Err(e) = serve_connection(stream, svc).await {
                tracing::debug!(%peer, error = %e, "cog-store session ended");
            }
        });
    }
}

/// Reaches owners: `mem://<name>` to registered in-process services, any
/// other address over mesh TCP (Noise XX when `noise` is set).
#[derive(Default)]
pub struct OwnerConnector {
    noise: bool,
    local: std::sync::RwLock<HashMap<String, Arc<StoreOwnerService>>>,
}

impl OwnerConnector {
    /// Connector; `noise` wraps TCP streams in Noise XX.
    pub fn new(noise: bool) -> Self {
        Self {
            noise,
            local: Default::default(),
        }
    }

    /// Register an in-process owner at `mem://<name>`; returns the address.
    pub fn register_local(&self, name: &str, svc: Arc<StoreOwnerService>) -> String {
        let addr = format!("{MEM_SCHEME}{name}");
        if let Ok(mut m) = self.local.write() {
            m.insert(addr.clone(), svc);
        }
        addr
    }
}

#[async_trait]
impl CtlConnector for OwnerConnector {
    async fn connect(&self, addr: &str) -> Result<Box<dyn MeshStream>, MeshError> {
        use crate::mesh::MeshTransport;
        if addr.starts_with(MEM_SCHEME) {
            let svc = self
                .local
                .read()
                .ok()
                .and_then(|m| m.get(addr).cloned())
                .ok_or_else(|| MeshError::PeerNotConnected(addr.to_string()))?;
            let (client, server) = crate::mesh_test_support::connected_pair().await?;
            tokio::spawn(async move {
                let _ = serve_connection(Box::new(server), svc).await;
            });
            return Ok(Box::new(client));
        }
        let stream = crate::mesh_tcp::TcpTransport.connect(addr).await?;
        if !self.noise {
            return Ok(stream);
        }
        let cfg = crate::mesh_noise::NoiseConfig {
            pattern: crate::mesh_noise::NoisePattern::XX,
            local_private_key: rand::random(),
            remote_static_key: None,
        };
        let ch = crate::mesh_noise::NoiseChannel::initiate(stream, &cfg).await?;
        Ok(Box::new(NoiseStream::new(Box::new(ch))))
    }
}

