//! Cross-node IPC message serialization for mesh transport (K6.3).
//!
//! Handles serializing `KernelMessage` (via [`MeshIpcEnvelope`]) to wire
//! format and deserializing incoming mesh messages.
//!
//! ## Encoding (ADR-031 / WEFT-683)
//!
//! **Shipped default:** JSON (`serde_json`) — [`MeshIpcEncoding::Json`] /
//! [`DEFAULT_MESH_IPC_ENCODING`].
//!
//! **Deferred:** RVF wire-segment encoding for production performance is
//! reserved behind the Cargo feature `mesh-rvf` as [`MeshIpcEncoding::Rvf`].
//! That path currently returns [`MeshIpcError::UnsupportedEncoding`]; enable
//! the feature only to exercise the API surface until Option B is built.
//!
//! Frame *kind* (`mesh_framing::FrameType`, e.g. `IpcMessage = 0x02`) is
//! orthogonal to IPC *payload encoding*. See ADR-031.

use serde::{Deserialize, Serialize};

use crate::ipc::{KernelMessage, MessageTarget};

/// Maximum IPC message size over mesh (16 MiB).
const MAX_IPC_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// Maximum number of relay hops before a message is dropped.
const MAX_HOPS: u8 = 8;

/// Wire encoding for mesh IPC envelopes (ADR-031 / WEFT-683).
///
/// Orthogonal to [`crate::mesh_framing::FrameType`]: frame type selects the
/// message kind (`IpcMessage`, `ChainSync`, …); this enum selects how the
/// bytes *inside* an IPC frame are serialized.
///
/// Discriminant values match the conceptual encoding table in ADR-031
/// (`0x01` JSON, `0x02` RVF). They are **not** written on the wire today —
/// shipped traffic is bare JSON with no encoding prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum MeshIpcEncoding {
    /// JSON (`serde_json`). Production and development default as of WEFT-683.
    #[default]
    Json = 0x01,
    /// RVF wire segment (ADR-031 production target). Only compiled with
    /// `--features mesh-rvf`; encode/decode return
    /// [`MeshIpcError::UnsupportedEncoding`] until Option B lands.
    #[cfg(feature = "mesh-rvf")]
    Rvf = 0x02,
}

/// Production IPC encoding for all default builds (JSON).
pub const DEFAULT_MESH_IPC_ENCODING: MeshIpcEncoding = MeshIpcEncoding::Json;

impl MeshIpcEncoding {
    /// Stable name for logs / errors.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            #[cfg(feature = "mesh-rvf")]
            Self::Rvf => "rvf",
        }
    }

    /// Parse a conceptual encoding byte (`0x01` JSON, `0x02` RVF when gated).
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::Json),
            #[cfg(feature = "mesh-rvf")]
            0x02 => Some(Self::Rvf),
            _ => None,
        }
    }
}

/// Tenant scope carried on a mesh envelope (P3 plan section 1.1).
///
/// `user_id` is 32 hex; `project_id` is a 26-char ULID when present. The
/// machine mesh service stamps `src_scope` from the sending registration;
/// the kernel only carries the fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    /// Owning user id.
    pub user_id: String,
    /// Project within the user, when the address is project-level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// A mesh IPC envelope wrapping a KernelMessage with routing metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshIpcEnvelope {
    /// Source node ID.
    pub source_node: String,
    /// Destination node ID.
    pub dest_node: String,
    /// The kernel message being transported.
    pub message: KernelMessage,
    /// Hop count (incremented at each relay, max 8).
    pub hop_count: u8,
    /// Unique envelope ID for deduplication.
    pub envelope_id: String,
    /// Destination tenant scope. Absent on the wire when `None`, so
    /// envelopes without scopes stay byte-identical to the old format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest_scope: Option<Scope>,
    /// Source tenant scope (stamped by the mesh service, never trusted
    /// from an unregistered peer). Absent on the wire when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src_scope: Option<Scope>,
}

impl MeshIpcEnvelope {
    /// Create a new envelope for a message being sent to a remote node.
    pub fn new(source_node: String, dest_node: String, message: KernelMessage) -> Self {
        Self {
            source_node,
            dest_node,
            message,
            hop_count: 0,
            envelope_id: uuid::Uuid::new_v4().to_string(),
            dest_scope: None,
            src_scope: None,
        }
    }

    /// Serialize the envelope using [`DEFAULT_MESH_IPC_ENCODING`] (JSON).
    pub fn to_bytes(&self) -> Result<Vec<u8>, MeshIpcError> {
        self.to_bytes_with_encoding(DEFAULT_MESH_IPC_ENCODING)
    }

    /// Serialize with an explicit encoding.
    ///
    /// JSON is always available. RVF requires `feature = "mesh-rvf"` and
    /// currently returns [`MeshIpcError::UnsupportedEncoding`] (WEFT-683
    /// Option A — deferred until revisit triggers in ADR-031 fire).
    pub fn to_bytes_with_encoding(
        &self,
        encoding: MeshIpcEncoding,
    ) -> Result<Vec<u8>, MeshIpcError> {
        match encoding {
            MeshIpcEncoding::Json => {
                let bytes = serde_json::to_vec(self)
                    .map_err(|e| MeshIpcError::Serialization(e.to_string()))?;
                if bytes.len() > MAX_IPC_MESSAGE_SIZE {
                    return Err(MeshIpcError::MessageTooLarge {
                        size: bytes.len(),
                        max: MAX_IPC_MESSAGE_SIZE,
                    });
                }
                Ok(bytes)
            }
            #[cfg(feature = "mesh-rvf")]
            MeshIpcEncoding::Rvf => Err(MeshIpcError::UnsupportedEncoding {
                encoding: encoding.as_str(),
            }),
        }
    }

    /// Deserialize an envelope from JSON bytes (default encoding).
    pub fn from_bytes(data: &[u8]) -> Result<Self, MeshIpcError> {
        Self::from_bytes_with_encoding(data, DEFAULT_MESH_IPC_ENCODING)
    }

    /// Deserialize with an explicit encoding.
    pub fn from_bytes_with_encoding(
        data: &[u8],
        encoding: MeshIpcEncoding,
    ) -> Result<Self, MeshIpcError> {
        if data.len() > MAX_IPC_MESSAGE_SIZE {
            return Err(MeshIpcError::MessageTooLarge {
                size: data.len(),
                max: MAX_IPC_MESSAGE_SIZE,
            });
        }
        match encoding {
            MeshIpcEncoding::Json => {
                let envelope: Self = serde_json::from_slice(data)
                    .map_err(|e| MeshIpcError::Deserialization(e.to_string()))?;
                Self::validate_boundary(&envelope)?;
                Ok(envelope)
            }
            #[cfg(feature = "mesh-rvf")]
            MeshIpcEncoding::Rvf => Err(MeshIpcError::UnsupportedEncoding {
                encoding: encoding.as_str(),
            }),
        }
    }

    /// Validate required fields at the mesh boundary.
    fn validate_boundary(envelope: &Self) -> Result<(), MeshIpcError> {
        if envelope.envelope_id.is_empty() {
            return Err(MeshIpcError::Deserialization(
                "envelope_id must not be empty".into(),
            ));
        }
        if envelope.source_node.is_empty() {
            return Err(MeshIpcError::Deserialization(
                "source_node must not be empty".into(),
            ));
        }
        Ok(())
    }

    /// Increment hop count and check if max hops exceeded.
    pub fn increment_hop(&mut self) -> Result<(), MeshIpcError> {
        self.hop_count += 1;
        if self.hop_count > MAX_HOPS {
            return Err(MeshIpcError::MaxHopsExceeded {
                hops: self.hop_count,
            });
        }
        Ok(())
    }

    /// Extract the inner target (unwrap RemoteNode if present).
    pub fn inner_target(&self) -> &MessageTarget {
        match &self.message.target {
            MessageTarget::RemoteNode { target, .. } => target.as_ref(),
            other => other,
        }
    }
}

/// Mesh IPC errors.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum MeshIpcError {
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("deserialization error: {0}")]
    Deserialization(String),
    #[error("message too large: {size} bytes (max {max})")]
    MessageTooLarge { size: usize, max: usize },
    #[error("max hops exceeded: {hops}")]
    MaxHopsExceeded { hops: u8 },
    #[error("duplicate message: {envelope_id}")]
    DuplicateMessage { envelope_id: String },
    /// Encoding requested but not implemented (or not enabled).
    ///
    /// Used for [`MeshIpcEncoding::Rvf`] under `feature = "mesh-rvf"` until
    /// ADR-031 Option B lands (WEFT-683).
    #[error("unsupported mesh IPC encoding: {encoding} (ADR-031 / WEFT-683 — deferred)")]
    UnsupportedEncoding { encoding: &'static str },
}

/// A correlated mesh request awaiting response.
#[derive(Debug)]
pub struct MeshRequest {
    /// Request envelope.
    pub request: MeshIpcEnvelope,
    /// Correlation ID for matching response.
    pub correlation_id: String,
    /// When the request was sent.
    pub sent_at: std::time::Instant,
    /// Timeout duration.
    pub timeout: std::time::Duration,
}

impl MeshRequest {
    /// Create a new mesh request with an auto-generated correlation ID.
    pub fn new(mut envelope: MeshIpcEnvelope, timeout: std::time::Duration) -> Self {
        let correlation_id = uuid::Uuid::new_v4().to_string();
        envelope.message.correlation_id = Some(correlation_id.clone());
        Self {
            request: envelope,
            correlation_id,
            sent_at: std::time::Instant::now(),
            timeout,
        }
    }

    /// Check if the request has timed out.
    pub fn is_timed_out(&self) -> bool {
        // `>=` so a zero-duration timeout (and the exact-deadline instant)
        // counts as timed out — `>` left a sub-tick race where a freshly
        // created zero-timeout request read as not-yet-expired under load.
        self.sent_at.elapsed() >= self.timeout
    }

    /// Check if a response envelope matches this request.
    pub fn matches_response(&self, response: &MeshIpcEnvelope) -> bool {
        response.message.correlation_id.as_deref() == Some(&self.correlation_id)
    }
}

/// Pending request tracker for correlated request-response.
pub struct PendingRequests {
    requests: std::collections::HashMap<String, MeshRequest>,
    /// Optional chain manager for exochain audit logging.
    #[cfg(feature = "exochain")]
    chain_manager: Option<std::sync::Arc<crate::chain::ChainManager>>,
}

impl PendingRequests {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self {
            requests: std::collections::HashMap::new(),
            #[cfg(feature = "exochain")]
            chain_manager: None,
        }
    }

    /// Attach a chain manager for exochain audit logging.
    #[cfg(feature = "exochain")]
    pub fn set_chain_manager(&mut self, cm: std::sync::Arc<crate::chain::ChainManager>) {
        self.chain_manager = Some(cm);
    }

    /// Register a pending request.
    pub fn register(&mut self, request: MeshRequest) {
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "mesh_ipc",
                crate::chain::EVENT_KIND_MESH_IPC_SEND,
                Some(serde_json::json!({
                    "correlation_id": &request.correlation_id,
                    "source_node": &request.request.source_node,
                    "dest_node": &request.request.dest_node,
                    "envelope_id": &request.request.envelope_id,
                })),
            );
        }
        self.requests
            .insert(request.correlation_id.clone(), request);
    }

    /// Try to match a response to a pending request, removing it on match.
    pub fn try_complete(&mut self, response: &MeshIpcEnvelope) -> Option<MeshRequest> {
        if let Some(corr_id) = &response.message.correlation_id {
            self.requests.remove(corr_id)
        } else {
            None
        }
    }

    /// Remove timed-out requests. Returns their correlation IDs.
    pub fn evict_timed_out(&mut self) -> Vec<String> {
        let timed_out: Vec<String> = self
            .requests
            .iter()
            .filter(|(_, r)| r.is_timed_out())
            .map(|(id, _)| id.clone())
            .collect();
        for id in &timed_out {
            self.requests.remove(id);
        }
        timed_out
    }

    /// Number of pending requests.
    pub fn len(&self) -> usize {
        self.requests.len()
    }

    /// Whether there are no pending requests.
    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }
}

impl Default for PendingRequests {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};

    fn make_test_message() -> KernelMessage {
        KernelMessage::text(1, MessageTarget::Process(2), "hello mesh")
    }

    #[test]
    fn envelope_new_sets_defaults() {
        let msg = make_test_message();
        let env = MeshIpcEnvelope::new("node-a".into(), "node-b".into(), msg);
        assert_eq!(env.source_node, "node-a");
        assert_eq!(env.dest_node, "node-b");
        assert_eq!(env.hop_count, 0);
        assert!(!env.envelope_id.is_empty());
    }

    #[test]
    fn to_bytes_from_bytes_roundtrip() {
        let msg = make_test_message();
        let env = MeshIpcEnvelope::new("src".into(), "dst".into(), msg);
        let bytes = env.to_bytes().unwrap();
        let restored = MeshIpcEnvelope::from_bytes(&bytes).unwrap();
        assert_eq!(restored.source_node, "src");
        assert_eq!(restored.dest_node, "dst");
        assert_eq!(restored.envelope_id, env.envelope_id);
        assert_eq!(restored.message.id, env.message.id);
    }

    #[test]
    fn from_bytes_rejects_oversized() {
        let big = vec![0u8; MAX_IPC_MESSAGE_SIZE + 1];
        let err = MeshIpcEnvelope::from_bytes(&big).unwrap_err();
        assert!(matches!(err, MeshIpcError::MessageTooLarge { .. }));
    }

    #[test]
    fn from_bytes_rejects_invalid_json() {
        let err = MeshIpcEnvelope::from_bytes(b"not json").unwrap_err();
        assert!(matches!(err, MeshIpcError::Deserialization(_)));
    }

    #[test]
    fn increment_hop_within_limit() {
        let msg = make_test_message();
        let mut env = MeshIpcEnvelope::new("a".into(), "b".into(), msg);
        for _ in 0..MAX_HOPS {
            env.increment_hop().unwrap();
        }
        assert_eq!(env.hop_count, MAX_HOPS);
    }

    #[test]
    fn increment_hop_exceeds_max() {
        let msg = make_test_message();
        let mut env = MeshIpcEnvelope::new("a".into(), "b".into(), msg);
        env.hop_count = MAX_HOPS;
        let err = env.increment_hop().unwrap_err();
        assert!(matches!(err, MeshIpcError::MaxHopsExceeded { .. }));
    }

    #[test]
    fn inner_target_unwraps_remote_node() {
        let inner = MessageTarget::Process(7);
        let remote_target = MessageTarget::RemoteNode {
            node_id: "node-42".into(),
            target: Box::new(inner),
        };
        let msg = KernelMessage::new(1, remote_target, MessagePayload::Text("hi".into()));
        let env = MeshIpcEnvelope::new("a".into(), "node-42".into(), msg);
        assert!(matches!(env.inner_target(), MessageTarget::Process(7)));
    }

    #[test]
    fn inner_target_returns_non_remote_as_is() {
        let msg = KernelMessage::text(1, MessageTarget::Broadcast, "hi");
        let env = MeshIpcEnvelope::new("a".into(), "b".into(), msg);
        assert!(matches!(env.inner_target(), MessageTarget::Broadcast));
    }

    #[test]
    fn from_bytes_rejects_empty_source_node() {
        let msg = make_test_message();
        let mut env = MeshIpcEnvelope::new("".into(), "b".into(), msg);
        env.source_node = "".into();
        let bytes = serde_json::to_vec(&env).unwrap();
        let err = MeshIpcEnvelope::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, MeshIpcError::Deserialization(_)));
    }

    #[test]
    fn from_bytes_rejects_empty_envelope_id() {
        let msg = make_test_message();
        let mut env = MeshIpcEnvelope::new("a".into(), "b".into(), msg);
        env.envelope_id = "".into();
        let bytes = serde_json::to_vec(&env).unwrap();
        let err = MeshIpcEnvelope::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, MeshIpcError::Deserialization(_)));
    }

    #[test]
    fn k6_cross_node_ipc_via_did_addressing() {
        // GlobalPid.node_id is derived from Ed25519 pubkey hash,
        // functioning as a decentralized identifier (DID).
        // This test proves messages can be addressed and routed via DID-based node IDs.
        use crate::ipc::GlobalPid;

        // Simulated DIDs: hex(SHA-256(pubkey)[0..16]) per node identity
        let node_a_did = "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8";
        let node_b_did = "b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8a1";

        // Address a process using DID-based GlobalPid
        let target_pid = GlobalPid::local(42, node_b_did);
        assert_eq!(target_pid.node_id, node_b_did);
        assert!(!target_pid.is_local(node_a_did));
        assert!(target_pid.is_local(node_b_did));

        // Wrap in RemoteNode target for cross-node delivery
        let msg = KernelMessage::text(
            1,
            MessageTarget::RemoteNode {
                node_id: node_b_did.to_string(),
                target: Box::new(MessageTarget::Process(42)),
            },
            "hello via DID",
        );

        // Create envelope with DID-based source and dest
        let envelope = MeshIpcEnvelope::new(node_a_did.to_string(), node_b_did.to_string(), msg);

        // Roundtrip serialization preserves DID addressing
        let bytes = envelope.to_bytes().unwrap();
        let restored = MeshIpcEnvelope::from_bytes(&bytes).unwrap();
        assert_eq!(restored.dest_node, node_b_did);
        assert_eq!(restored.source_node, node_a_did);

        // Inner target unwraps to the addressed process
        assert!(matches!(
            restored.inner_target(),
            MessageTarget::Process(42)
        ));
    }

    #[test]
    fn mesh_request_correlation() {
        let msg = make_test_message();
        let env = MeshIpcEnvelope::new("a".into(), "b".into(), msg);
        let req = MeshRequest::new(env, std::time::Duration::from_secs(5));
        assert!(!req.correlation_id.is_empty());
        assert_eq!(
            req.request.message.correlation_id.as_deref(),
            Some(req.correlation_id.as_str())
        );
    }

    #[test]
    fn mesh_request_timeout() {
        let msg = make_test_message();
        let env = MeshIpcEnvelope::new("a".into(), "b".into(), msg);
        // Zero-duration timeout: immediately timed out
        let req = MeshRequest::new(env, std::time::Duration::from_secs(0));
        assert!(req.is_timed_out());
    }

    #[test]
    fn pending_requests_complete() {
        let msg = make_test_message();
        let env = MeshIpcEnvelope::new("a".into(), "b".into(), msg);
        let req = MeshRequest::new(env, std::time::Duration::from_secs(30));
        let corr_id = req.correlation_id.clone();

        let mut pending = PendingRequests::new();
        pending.register(req);
        assert_eq!(pending.len(), 1);

        // Build a response with matching correlation_id
        let resp_msg = KernelMessage::with_correlation(
            2,
            MessageTarget::Process(1),
            MessagePayload::Text("ok".into()),
            corr_id.clone(),
        );
        let resp_env = MeshIpcEnvelope::new("b".into(), "a".into(), resp_msg);
        let completed = pending.try_complete(&resp_env);
        assert!(completed.is_some());
        assert_eq!(completed.unwrap().correlation_id, corr_id);
        assert!(pending.is_empty());
    }

    #[test]
    fn pending_requests_evict_timed_out() {
        let msg = make_test_message();
        let env = MeshIpcEnvelope::new("a".into(), "b".into(), msg);
        let req = MeshRequest::new(env, std::time::Duration::from_secs(0));
        let corr_id = req.correlation_id.clone();

        let mut pending = PendingRequests::new();
        pending.register(req);
        assert_eq!(pending.len(), 1);

        let evicted = pending.evict_timed_out();
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0], corr_id);
        assert!(pending.is_empty());
    }

    #[test]
    fn envelope_serde_with_correlation_id() {
        let msg = KernelMessage::with_correlation(
            1,
            MessageTarget::Process(2),
            MessagePayload::Text("req".into()),
            "corr-99".into(),
        );
        let env = MeshIpcEnvelope::new("n1".into(), "n2".into(), msg);
        let bytes = env.to_bytes().unwrap();
        let restored = MeshIpcEnvelope::from_bytes(&bytes).unwrap();
        assert_eq!(restored.message.correlation_id, Some("corr-99".into()));
    }

    #[test]
    fn default_encoding_is_json() {
        assert_eq!(DEFAULT_MESH_IPC_ENCODING, MeshIpcEncoding::Json);
        assert_eq!(MeshIpcEncoding::default(), MeshIpcEncoding::Json);
        assert_eq!(MeshIpcEncoding::Json.as_str(), "json");
        assert_eq!(
            MeshIpcEncoding::from_byte(0x01),
            Some(MeshIpcEncoding::Json)
        );
        // RVF byte is unknown unless mesh-rvf is enabled.
        #[cfg(not(feature = "mesh-rvf"))]
        assert_eq!(MeshIpcEncoding::from_byte(0x02), None);
        #[cfg(feature = "mesh-rvf")]
        assert_eq!(MeshIpcEncoding::from_byte(0x02), Some(MeshIpcEncoding::Rvf));
    }

    #[test]
    fn to_bytes_with_encoding_json_matches_to_bytes() {
        let msg = make_test_message();
        let env = MeshIpcEnvelope::new("src".into(), "dst".into(), msg);
        let a = env.to_bytes().unwrap();
        let b = env.to_bytes_with_encoding(MeshIpcEncoding::Json).unwrap();
        assert_eq!(a, b);
        let restored =
            MeshIpcEnvelope::from_bytes_with_encoding(&a, MeshIpcEncoding::Json).unwrap();
        assert_eq!(restored.envelope_id, env.envelope_id);
    }

    #[cfg(feature = "mesh-rvf")]
    #[test]
    fn rvf_encoding_returns_unsupported() {
        let msg = make_test_message();
        let env = MeshIpcEnvelope::new("src".into(), "dst".into(), msg);
        let err = env
            .to_bytes_with_encoding(MeshIpcEncoding::Rvf)
            .unwrap_err();
        assert!(matches!(
            err,
            MeshIpcError::UnsupportedEncoding { encoding: "rvf" }
        ));
        let err =
            MeshIpcEnvelope::from_bytes_with_encoding(b"{}", MeshIpcEncoding::Rvf).unwrap_err();
        assert!(matches!(
            err,
            MeshIpcError::UnsupportedEncoding { encoding: "rvf" }
        ));
    }

    // ── P3-K0: scope fields are wire-compatible ──────────────────

    /// Frozen wire bytes of an envelope as written before scopes existed.
    /// Do not regenerate: if this fails, the wire format changed.
    const FROZEN_PRE_SCOPE_ENVELOPE: &str = concat!(
        r#"{"source_node":"a","dest_node":"b","message":{"id":"m1","from":1,"#,
        r#""target":{"Topic":"t"},"payload":{"Text":"x"},"#,
        r#""timestamp":"2026-01-02T03:04:05Z"},"hop_count":0,"envelope_id":"fixed-id"}"#
    );

    #[test]
    fn envelope_without_scopes_serializes_byte_identical_to_old_format() {
        let env = MeshIpcEnvelope::from_bytes(FROZEN_PRE_SCOPE_ENVELOPE.as_bytes()).unwrap();
        assert!(env.dest_scope.is_none() && env.src_scope.is_none());
        assert_eq!(serde_json::to_string(&env).unwrap(), FROZEN_PRE_SCOPE_ENVELOPE);
        assert!(!FROZEN_PRE_SCOPE_ENVELOPE.contains("scope"));
    }

    #[test]
    fn old_json_without_scopes_deserializes() {
        let msg = KernelMessage::text(1, MessageTarget::Topic("t".into()), "x");
        let env = MeshIpcEnvelope::new("a".into(), "b".into(), msg);
        let bytes = env.to_bytes().unwrap();
        let back = MeshIpcEnvelope::from_bytes(&bytes).unwrap();
        assert!(back.dest_scope.is_none() && back.src_scope.is_none());
    }

    #[test]
    fn scoped_envelope_round_trips() {
        let msg = KernelMessage::text(1, MessageTarget::Topic("t".into()), "x");
        let mut env = MeshIpcEnvelope::new("a".into(), "b".into(), msg);
        env.dest_scope = Some(Scope {
            user_id: "u".repeat(32),
            project_id: Some("P".repeat(26)),
        });
        env.src_scope = Some(Scope {
            user_id: "v".repeat(32),
            project_id: None,
        });
        let back = MeshIpcEnvelope::from_bytes(&env.to_bytes().unwrap()).unwrap();
        assert_eq!(back.dest_scope, env.dest_scope);
        assert_eq!(back.src_scope, env.src_scope);
    }
}
