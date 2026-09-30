//! `workload.ctl` on the wire: [`MeshIpcEnvelope`]s carrying a
//! [`KernelMessage`] addressed to `ServiceMethod { workload-host, <method> }`,
//! correlated with [`MeshRequest`], plus the artifact piece protocol
//! multiplexed on the same stream for fetch-before-load.
//!
//! One connection, controller to target:
//!
//! ```text
//! controller                                  target (workload-host)
//!   envelope(SignedCtl request)       ->
//!                                     <-      artifact frames (target fetches
//!   artifact frames (controller serves) ->     the package from the controller)
//!                                     <-      envelope(SignedCtl response)
//! ```
//!
//! Envelopes are JSON (first byte `{`); artifact frames start with a
//! big-endian length whose first byte is 0 (frames are far below 16 MiB).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh::{MeshError, MeshStream};
use crate::mesh_artifact::ArtifactExchange;
use crate::mesh_artifact_transfer::{PeerLink, PeerSet, ServeStats};
use crate::mesh_ipc::{MeshIpcEnvelope, MeshRequest};
use crate::mesh_noise::EncryptedChannel;

use super::host_service::WorkloadHostService;
use super::msg::{SignedCtl, WORKLOAD_HOST_SERVICE};

/// True if a raw frame is an IPC envelope (JSON) rather than an artifact frame.
pub fn is_envelope(raw: &[u8]) -> bool {
    raw.first() == Some(&b'{')
}

/// Wrap a signed request for `dest` as a correlated [`MeshRequest`].
pub fn request_envelope(
    source: &str,
    dest: &str,
    method: &str,
    signed: &SignedCtl,
    timeout: Duration,
) -> MeshRequest {
    let msg = KernelMessage::new(
        0,
        MessageTarget::ServiceMethod {
            service: WORKLOAD_HOST_SERVICE.to_string(),
            method: method.to_string(),
        },
        MessagePayload::Json(serde_json::to_value(signed).unwrap_or_default()),
    );
    MeshRequest::new(
        MeshIpcEnvelope::new(source.into(), dest.into(), msg),
        timeout,
    )
}

/// The signed control payload of an envelope addressed to `workload-host`.
pub fn ctl_payload(env: &MeshIpcEnvelope) -> Result<(String, SignedCtl), String> {
    let method = match env.inner_target() {
        MessageTarget::ServiceMethod { service, method } if service == WORKLOAD_HOST_SERVICE => {
            method.clone()
        }
        other => {
            return Err(format!(
                "not addressed to {WORKLOAD_HOST_SERVICE}: {other:?}"
            ));
        }
    };
    let MessagePayload::Json(v) = &env.message.payload else {
        return Err("workload.ctl payload must be JSON".into());
    };
    let signed: SignedCtl =
        serde_json::from_value(v.clone()).map_err(|e| format!("bad signed payload: {e}"))?;
    Ok((method, signed))
}

/// Largest plaintext per Noise message (a Noise message is at most 65535
/// bytes including the 16-byte tag; one byte carries the fragment flag).
const NOISE_CHUNK: usize = 65_000;
/// Largest reassembled message (the IPC and artifact frame caps are lower).
const MAX_NOISE_MESSAGE: usize = 17 * 1024 * 1024;

/// A Noise channel as a [`MeshStream`]. Messages larger than one Noise
/// message (artifact pieces) are split into fragments, each prefixed with
/// a flag byte (1 = last), and reassembled on receipt. Partial fragments
/// are kept on `self`, so `recv` stays cancel-safe.
pub struct NoiseStream {
    ch: Box<dyn EncryptedChannel>,
    partial: Vec<u8>,
}

impl NoiseStream {
    /// Wrap an established channel.
    pub fn new(ch: Box<dyn EncryptedChannel>) -> Self {
        Self {
            ch,
            partial: Vec::new(),
        }
    }
}

#[async_trait]
impl MeshStream for NoiseStream {
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
        let mut chunks = data.chunks(NOISE_CHUNK).peekable();
        if chunks.peek().is_none() {
            return self.ch.send_encrypted(&[1]).await;
        }
        while let Some(c) = chunks.next() {
            let mut m = Vec::with_capacity(c.len() + 1);
            m.push(u8::from(chunks.peek().is_none()));
            m.extend_from_slice(c);
            self.ch.send_encrypted(&m).await?;
        }
        Ok(())
    }
    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        loop {
            let m = self.ch.recv_encrypted().await?;
            let (flag, body) = m
                .split_first()
                .ok_or_else(|| MeshError::Transport("empty noise fragment".into()))?;
            if self.partial.len() + body.len() > MAX_NOISE_MESSAGE {
                self.partial.clear();
                return Err(MeshError::MessageTooLarge {
                    size: MAX_NOISE_MESSAGE + 1,
                    max: MAX_NOISE_MESSAGE,
                });
            }
            self.partial.extend_from_slice(body);
            if *flag == 1 {
                return Ok(std::mem::take(&mut self.partial));
            }
        }
    }
    async fn close(&mut self) -> Result<(), MeshError> {
        self.ch.close().await
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        None
    }
}

/// Serve one connection as the target's `workload-host` until the peer
/// closes it. Each request may fetch its payload over the same stream.
pub async fn serve_connection(
    stream: Box<dyn MeshStream>,
    svc: Arc<WorkloadHostService>,
) -> Result<(), MeshError> {
    let mut peers = PeerSet::single(PeerLink::new("controller", stream));
    loop {
        let raw = match peers.links_mut()[0].stream.recv().await {
            Ok(r) => r,
            Err(MeshError::ConnectionClosed) => return Ok(()),
            Err(e) => return Err(e),
        };
        if !is_envelope(&raw) {
            // Artifact frames are only valid while this side is fetching.
            let _ = peers.links_mut()[0].stream.close().await;
            return Err(MeshError::Transport("unsolicited artifact frame".into()));
        }
        let env = MeshIpcEnvelope::from_bytes(&raw)
            .map_err(|e| MeshError::Transport(format!("bad envelope: {e}")))?;
        let reply_to = env.source_node.clone();
        let (method, signed) = match ctl_payload(&env) {
            Ok(x) => x,
            Err(e) => return Err(MeshError::Transport(e)),
        };
        peers.links_mut()[0].peer_id = reply_to.clone();
        let resp = svc.handle(&method, &signed, Some(&mut peers)).await;
        let mut msg = KernelMessage::new(
            0,
            MessageTarget::ServiceMethod {
                service: WORKLOAD_HOST_SERVICE.to_string(),
                method: method.clone(),
            },
            MessagePayload::Json(serde_json::to_value(&resp).unwrap_or_default()),
        );
        msg.correlation_id = env.message.correlation_id.clone();
        let out = MeshIpcEnvelope::new(svc.node_id().to_string(), reply_to, msg);
        let bytes = out
            .to_bytes()
            .map_err(|e| MeshError::Transport(e.to_string()))?;
        let link = &mut peers.links_mut()[0];
        if link.is_dead() {
            return Ok(()); // a failed fetch closed the stream
        }
        link.stream.send(&bytes).await?;
    }
}

/// Why a call did not produce a signed response.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    /// Transport failure.
    #[error("transport: {0}")]
    Transport(String),
    /// No response in time.
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    /// The peer spoke something else.
    #[error("protocol: {0}")]
    Protocol(String),
}

impl From<MeshError> for CallError {
    fn from(e: MeshError) -> Self {
        CallError::Transport(e.to_string())
    }
}

/// Controller end of one connection.
pub struct CtlConnection {
    stream: Box<dyn MeshStream>,
    local_node: String,
}

impl CtlConnection {
    /// Wrap an open stream to a target.
    pub fn new(stream: Box<dyn MeshStream>, local_node: impl Into<String>) -> Self {
        Self {
            stream,
            local_node: local_node.into(),
        }
    }

    /// Send one signed request to `dest` and wait for its correlated
    /// response. Artifact frames that arrive meanwhile are served from
    /// `exchange` (the target fetching the payload), if given.
    pub async fn call(
        &mut self,
        dest: &str,
        method: &str,
        signed: &SignedCtl,
        exchange: Option<&ArtifactExchange>,
        timeout: Duration,
    ) -> Result<SignedCtl, CallError> {
        let req = request_envelope(&self.local_node, dest, method, signed, timeout);
        let bytes = req
            .request
            .to_bytes()
            .map_err(|e| CallError::Protocol(e.to_string()))?;
        self.stream.send(&bytes).await?;
        let deadline = tokio::time::Instant::now() + timeout;
        let mut stats = ServeStats::default();
        loop {
            let raw = tokio::time::timeout_at(deadline, self.stream.recv())
                .await
                .map_err(|_| CallError::Timeout(timeout))??;
            if !is_envelope(&raw) {
                let ex = exchange.ok_or_else(|| {
                    CallError::Protocol("artifact frame but nothing to serve".into())
                })?;
                ex.serve_frame(self.stream.as_mut(), dest, &raw, &mut stats)
                    .await
                    .map_err(|e| CallError::Protocol(format!("serving payload: {e}")))?;
                continue;
            }
            let env = MeshIpcEnvelope::from_bytes(&raw)
                .map_err(|e| CallError::Protocol(e.to_string()))?;
            if !req.matches_response(&env) {
                return Err(CallError::Protocol("uncorrelated response".into()));
            }
            let (_, signed) = ctl_payload(&env).map_err(CallError::Protocol)?;
            return Ok(signed);
        }
    }

    /// Close the connection.
    pub async fn close(mut self) {
        let _ = self.stream.close().await;
    }
}
