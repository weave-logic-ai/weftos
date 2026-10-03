//! Forwarding validated batches to the store owner (the owner's side is in
//! [`super::owner`]).
//!
//! The owner is this node ([`LocalForwarder`]) or another node
//! ([`MeshForwarder`] to a [`StoreOwnerService`]). Remote forwarding is a
//! [`ForwardRequest`] signed by the bridge node's Ed25519 key, carried as a
//! [`MeshIpcEnvelope`] addressed to `ServiceMethod { cog-store, store.ingest }`
//! and answered by a response signed by the owner and bound to the request
//! nonce. The owner checks, in order: size, signature, structure, key
//! binding, addressee, time window, forwarder policy (key and project),
//! nonce freshness. The nonce is recorded last, so a forged request cannot
//! burn a legitimate one.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use super::registry::InstanceBinding;
use super::store::{IngestOutcome, Provenance, StoreDirectory, StoreError};
use super::types::{IngestBatch, IngestError, IngestVector};
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh::MeshStream;
use crate::mesh_ipc::{MeshIpcEnvelope, MeshRequest};
use crate::node_registry::node_id_from_pubkey;
use crate::workload_ctl::msg::{SignedCtl, fresh_nonce, open, sign};
use crate::workload_ctl::transport::CtlConnector;

/// Service the store owner answers as.
pub const STORE_SERVICE: &str = "cog-store";
/// Method of the service.
pub const STORE_INGEST_METHOD: &str = "store.ingest";
/// Wire version.
pub const FORWARD_VERSION: u32 = 1;
/// Domain tag signed before a request payload.
pub const FORWARD_DOMAIN: &[u8] = b"weftos.cog_ingest.forward.v1\0";
/// Domain tag signed before a response payload.
pub const FORWARD_RESPONSE_DOMAIN: &[u8] = b"weftos.cog_ingest.forward_response.v1\0";
/// Address scheme for in-process owners.
pub const MEM_SCHEME: &str = "mem://";

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Takes a batch to the node that owns the store.
#[async_trait]
pub trait Forwarder: Send + Sync {
    /// Forward `batch` for the instance `b`.
    async fn forward(
        &self,
        b: &InstanceBinding,
        batch: &IngestBatch,
    ) -> Result<IngestOutcome, IngestError>;
}

fn store_err(e: StoreError) -> IngestError {
    IngestError::Unavailable(e.to_string())
}

/// The store owner is this node.
pub struct LocalForwarder {
    node_id: String,
    directory: Arc<dyn StoreDirectory>,
}

impl LocalForwarder {
    /// Forwarder into `directory` on node `node_id`.
    pub fn new(node_id: impl Into<String>, directory: Arc<dyn StoreDirectory>) -> Self {
        Self {
            node_id: node_id.into(),
            directory,
        }
    }
}

#[async_trait]
impl Forwarder for LocalForwarder {
    async fn forward(
        &self,
        b: &InstanceBinding,
        batch: &IngestBatch,
    ) -> Result<IngestOutcome, IngestError> {
        let store = self
            .directory
            .store_for(b.project_id.as_deref())
            .ok_or_else(|| IngestError::Unavailable("no store for this placement".into()))?;
        let from = Provenance {
            instance_id: b.instance_id.clone(),
            source_node: self.node_id.clone(),
        };
        store
            .ingest(&from, &batch.vectors, batch.dedup)
            .map_err(store_err)
    }
}

// ── Wire messages ────────────────────────────────────────────────────────

/// A forwarded batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardRequest {
    /// [`FORWARD_VERSION`].
    pub version: u32,
    /// Forwarding node id (derived from the signing key).
    pub requester: String,
    /// Store-owner node id.
    pub target: String,
    /// 32 hex characters, fresh per request.
    pub nonce: String,
    /// Issue time, ms since the Unix epoch.
    pub issued_at_ms: u64,
    /// Expiry, ms since the Unix epoch.
    pub expires_at_ms: u64,
    /// Project whose store takes the batch (`None`: the controller's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Instance that posted it (provenance).
    pub instance_id: String,
    /// Placing controller (provenance).
    pub controller_node: String,
    /// Dedup flag, honoured by the store.
    pub dedup: bool,
    /// The vectors.
    pub vectors: Vec<IngestVector>,
}

/// Why an owner refused. Stable names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardRefusal {
    /// Signature, key or structure check failed.
    Signature,
    /// Outside its time window.
    Expired,
    /// Nonce already seen.
    Replay,
    /// Addressed to another node.
    NotForMe,
    /// Key (or key and project) not authorised to forward here.
    Unauthorized,
    /// Malformed batch.
    Invalid,
    /// No store for the project here.
    NoStore,
    /// Store at capacity.
    StoreFull,
    /// Store backend failure.
    Backend,
}

/// What the owner did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ForwardOutcome {
    /// Written.
    Ok {
        /// Counts.
        result: IngestOutcome,
    },
    /// Refused.
    Refused {
        /// Code.
        code: ForwardRefusal,
        /// Reason.
        reason: String,
    },
}

/// Signed answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardResponse {
    /// [`FORWARD_VERSION`].
    pub version: u32,
    /// Responding node id.
    pub responder: String,
    /// Nonce of the request answered.
    pub request_nonce: String,
    /// Result.
    pub outcome: ForwardOutcome,
}

impl ForwardRequest {
    /// Build and sign a request for `batch` from `key`'s node.
    pub fn signed(
        key: &SigningKey,
        target: &str,
        b: &InstanceBinding,
        batch: &IngestBatch,
        ttl_ms: u64,
    ) -> Result<(Self, SignedCtl), IngestError> {
        let now = now_ms();
        let req = Self {
            version: FORWARD_VERSION,
            requester: node_id_from_pubkey(&key.verifying_key().to_bytes()),
            target: target.to_string(),
            nonce: fresh_nonce(),
            issued_at_ms: now,
            expires_at_ms: now.saturating_add(ttl_ms),
            project_id: b.project_id.clone(),
            instance_id: b.instance_id.clone(),
            controller_node: b.controller_node.clone(),
            dedup: batch.dedup,
            vectors: batch.vectors.clone(),
        };
        let payload = serde_json::to_string(&req)
            .map_err(|e| IngestError::Unavailable(format!("encode: {e}")))?;
        let signed = sign(FORWARD_DOMAIN, payload, key);
        Ok((req, signed))
    }
}

pub(super) fn owner_target() -> MessageTarget {
    MessageTarget::ServiceMethod {
        service: STORE_SERVICE.to_string(),
        method: STORE_INGEST_METHOD.to_string(),
    }
}

// ── Bridge side ──────────────────────────────────────────────────────────

/// Forwards to a remote owner over a kept connection (reconnects once on a
/// transport failure).
pub struct MeshForwarder {
    key: SigningKey,
    local_node: String,
    owner_node: String,
    owner_key: [u8; 32],
    addr: String,
    connector: Arc<dyn CtlConnector>,
    timeout: Duration,
    conn: tokio::sync::Mutex<Option<Box<dyn MeshStream>>>,
}

impl MeshForwarder {
    /// Forwarder signing as `key`'s node to the owner `owner_node` (whose
    /// key must be `owner_key`) at `addr`.
    pub fn new(
        key: SigningKey,
        owner_node: &str,
        owner_key: [u8; 32],
        addr: &str,
        connector: Arc<dyn CtlConnector>,
    ) -> Self {
        Self {
            local_node: node_id_from_pubkey(&key.verifying_key().to_bytes()),
            key,
            owner_node: owner_node.to_string(),
            owner_key,
            addr: addr.to_string(),
            connector,
            timeout: Duration::from_secs(5),
            conn: tokio::sync::Mutex::new(None),
        }
    }

    async fn round_trip(
        &self,
        guard: &mut Option<Box<dyn MeshStream>>,
        req: &MeshRequest,
        bytes: &[u8],
    ) -> Result<MeshIpcEnvelope, String> {
        if guard.is_none() {
            *guard = Some(
                self.connector
                    .connect(&self.addr)
                    .await
                    .map_err(|e| e.to_string())?,
            );
        }
        let s = guard.as_mut().ok_or("no connection")?;
        s.send(bytes).await.map_err(|e| e.to_string())?;
        let raw = tokio::time::timeout(self.timeout, s.recv())
            .await
            .map_err(|_| "timed out".to_string())?
            .map_err(|e| e.to_string())?;
        let env = MeshIpcEnvelope::from_bytes(&raw).map_err(|e| e.to_string())?;
        if !req.matches_response(&env) {
            return Err("uncorrelated response".into());
        }
        Ok(env)
    }
}

#[async_trait]
impl Forwarder for MeshForwarder {
    async fn forward(
        &self,
        b: &InstanceBinding,
        batch: &IngestBatch,
    ) -> Result<IngestOutcome, IngestError> {
        let unavailable = |m: String| IngestError::Unavailable(m);
        let (sent, signed) = ForwardRequest::signed(&self.key, &self.owner_node, b, batch, 30_000)?;
        let msg = KernelMessage::new(
            0,
            owner_target(),
            MessagePayload::Json(serde_json::to_value(&signed).unwrap_or_default()),
        );
        let req = MeshRequest::new(
            MeshIpcEnvelope::new(self.local_node.clone(), self.owner_node.clone(), msg),
            self.timeout,
        );
        let bytes = req.request.to_bytes().map_err(|e| unavailable(e.to_string()))?;
        let mut guard = self.conn.lock().await;
        let env = match self.round_trip(&mut guard, &req, &bytes).await {
            Ok(e) => e,
            Err(first) => {
                // A kept connection may have gone away. The same signed
                // request is resent once; if the owner had already taken it
                // the answer is a replay refusal, never a double write.
                *guard = None;
                self.round_trip(&mut guard, &req, &bytes)
                    .await
                    .map_err(|e| unavailable(format!("{first}; retry: {e}")))?
            }
        };
        drop(guard);
        let MessagePayload::Json(v) = env.message.payload else {
            return Err(unavailable("response payload is not JSON".into()));
        };
        let signed: SignedCtl = serde_json::from_value(v).map_err(|e| unavailable(e.to_string()))?;
        let pk = open(FORWARD_RESPONSE_DOMAIN, &signed).map_err(|r| unavailable(r.reason))?;
        if pk != self.owner_key {
            return Err(unavailable("response signed by another key".into()));
        }
        let resp: ForwardResponse =
            serde_json::from_str(&signed.payload).map_err(|e| unavailable(e.to_string()))?;
        if resp.responder != self.owner_node || resp.request_nonce != sent.nonce {
            return Err(unavailable("response answers another request".into()));
        }
        match resp.outcome {
            ForwardOutcome::Ok { result } => Ok(result),
            ForwardOutcome::Refused { code, reason } => {
                Err(unavailable(format!("owner refused ({code:?}): {reason}")))
            }
        }
    }
}
