//! Forwarding over the mesh: the consumer side ([`forward_remote`]) and the
//! serving side ([`serve_infer`]).
//!
//! Security shape:
//! - The consumer dials only a node id the table resolved, and only through
//!   a [`MeshDialer`] that vouches for the peer being admitted and verified.
//!   The client's request never names a host.
//! - The server serves only a verified peer, only a role this node exposed
//!   to the mesh, and only a *local* instance: it never forwards onward.
//! - Both sides bound frame counts by bytes and time, and the server
//!   revalidates the request as it would a client's.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::mesh_policy::{mesh_path_allowed, pin_body};
use super::table::MeshLocal;
use super::types::qualifies;
use super::types::{MeshDialer, ProxyAudit, ProxyError, ProxyLimits, ProxyRequest, ResponseSink};
use super::upstream::Upstream;
use super::wire::{self, Resp};
use crate::mesh::MeshStream;
use crate::mesh_admit::Grant;
use crate::mesh_framing::{FrameType, MeshFrame, read_frame as rf, write_frame as wf};

fn mesh(e: impl std::fmt::Display) -> ProxyError {
    ProxyError::Mesh(e.to_string())
}

async fn write_frame(s: &mut dyn MeshStream, f: &MeshFrame) -> Result<(), ProxyError> {
    wf(s, f).await.map_err(mesh)
}

async fn read_frame(s: &mut dyn MeshStream) -> Result<MeshFrame, ProxyError> {
    rf(s).await.map_err(mesh)
}

async fn recv_timeout(
    s: &mut dyn MeshStream,
    limits: &ProxyLimits,
) -> Result<MeshFrame, ProxyError> {
    tokio::time::timeout(limits.stall_timeout, read_frame(s))
        .await
        .map_err(|_| ProxyError::Timeout("mesh peer stalled".into()))?
}

/// Forward `req` to the instance of its role on `node_id` and relay the
/// response into `sink`.
pub async fn forward_remote(
    dialer: &dyn MeshDialer,
    node_id: &str,
    req: &ProxyRequest,
    sink: &mut dyn ResponseSink,
    limits: &ProxyLimits,
) -> Result<(), ProxyError> {
    if !dialer.is_admitted(node_id) {
        return Err(ProxyError::Refused(format!(
            "peer {node_id} is not an admitted peer"
        )));
    }
    let mut stream = dialer.dial(node_id).await?;
    let result = async {
        let payload = wire::encode_request(req)?;
        write_frame(
            stream.as_mut(),
            &MeshFrame {
                frame_type: FrameType::InferRequest,
                payload,
            },
        )
        .await?;
        let mut total: u64 = 0;
        let mut got_head = false;
        loop {
            let f = recv_timeout(stream.as_mut(), limits).await?;
            if f.frame_type != FrameType::InferResponse {
                return Err(mesh("unexpected frame from peer"));
            }
            match wire::decode_resp(&f.payload)? {
                Resp::Head {
                    status,
                    content_type,
                } => {
                    if got_head {
                        return Err(mesh("second response head"));
                    }
                    got_head = true;
                    sink.head(status, content_type.as_deref()).await?;
                }
                Resp::Chunk(d) => {
                    if !got_head {
                        return Err(mesh("body before head"));
                    }
                    total += d.len() as u64;
                    if total > limits.max_response_body {
                        return Err(ProxyError::TooLarge("response body".into()));
                    }
                    sink.chunk(&d).await?;
                }
                Resp::End if got_head => return Ok(()),
                Resp::End => return Err(mesh("end before head")),
                Resp::Error(m) => return Err(ProxyError::Upstream(format!("peer: {m}"))),
            }
        }
    }
    .await;
    let _ = stream.close().await;
    result
}

/// The peer on the other end of a stream, as established by admission.
#[derive(Debug, Clone)]
pub struct InferPeer {
    /// Node id.
    pub node_id: String,
    /// The peer's admission grant, from the connection (`None`: the id
    /// was not verified against it). Serving requires [`qualifies`].
    pub grant: Option<Grant>,
}

/// What a served exchange did, for the caller's accounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Served {
    /// Request answered.
    Ok,
    /// Request refused or failed; the reason was sent to the peer.
    Failed(String),
}

struct FrameSink<'a> {
    stream: &'a mut dyn MeshStream,
}

impl FrameSink<'_> {
    async fn send(&mut self, r: Resp) -> Result<(), ProxyError> {
        write_frame(
            self.stream,
            &MeshFrame {
                frame_type: FrameType::InferResponse,
                payload: wire::encode_resp(&r),
            },
        )
        .await
    }
}

#[async_trait]
impl ResponseSink for FrameSink<'_> {
    async fn head(&mut self, status: u16, content_type: Option<&str>) -> Result<(), ProxyError> {
        self.send(Resp::Head {
            status,
            content_type: content_type.map(str::to_string),
        })
        .await
    }

    async fn chunk(&mut self, data: &[u8]) -> Result<(), ProxyError> {
        for piece in data.chunks(wire::CHUNK_BYTES) {
            self.send(Resp::Chunk(piece.to_vec())).await?;
        }
        Ok(())
    }
}

/// Concurrency limits on the serving side: a peer cannot occupy the model
/// server with parallel requests, and the node as a whole stays bounded.
pub struct ServeGate {
    total: Arc<Semaphore>,
    per_peer_max: usize,
    per_peer: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl ServeGate {
    /// At most `per_peer` concurrent requests from one peer and `total` in
    /// all.
    pub fn new(per_peer: usize, total: usize) -> Self {
        Self {
            total: Arc::new(Semaphore::new(total)),
            per_peer_max: per_peer,
            per_peer: Mutex::new(HashMap::new()),
        }
    }

    fn acquire(&self, peer: &str) -> Option<(OwnedSemaphorePermit, OwnedSemaphorePermit)> {
        let peer_sem = {
            let mut m = self.per_peer.lock().unwrap();
            // Entries with no request in flight are dropped so the table
            // cannot grow with the peers seen.
            m.retain(|_, s| s.available_permits() < self.per_peer_max);
            m.entry(peer.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(self.per_peer_max)))
                .clone()
        };
        let a = peer_sem.try_acquire_owned().ok()?;
        let b = self.total.clone().try_acquire_owned().ok()?;
        Some((a, b))
    }
}

#[cfg(test)]
impl ServeGate {
    /// Take a slot and leak it (the test never releases it).
    pub(super) fn acquire_for_test(&self, peer: &str) -> bool {
        match self.acquire(peer) {
            Some((a, b)) => {
                std::mem::forget(a);
                std::mem::forget(b);
                true
            }
            None => false,
        }
    }
}

impl Default for ServeGate {
    fn default() -> Self {
        Self::new(2, 8)
    }
}

/// What a peer is told. Local detail (ports, paths, server messages) goes to
/// the audit trail, never over the wire.
fn peer_reason(e: &ProxyError) -> &'static str {
    match e {
        ProxyError::Refused(_) => "refused",
        ProxyError::NoInstance(_) => "role not served here",
        ProxyError::BadRequest(_)
        | ProxyError::Forbidden(_)
        | ProxyError::MethodNotAllowed
        | ProxyError::TooLarge(_) => "request not accepted",
        ProxyError::Timeout(_) => "timeout",
        _ => "upstream error",
    }
}

/// Serve one forwarded request from `peer` on `stream`. `local_for_mesh`
/// maps `(role, peer node id)` to the loopback instance this node exposes
/// to that peer for the role (or `None`: not exposed, or the peer is not on
/// the role's serve allowlist). The request is held to the mesh path allowlist and its
/// body is pinned to that instance's model.
pub async fn serve_infer(
    stream: &mut dyn MeshStream,
    peer: &InferPeer,
    local_for_peer: &(dyn Fn(&str, &str) -> Option<MeshLocal> + Sync),
    upstream: &Upstream,
    gate: &ServeGate,
    audit: Option<&dyn ProxyAudit>,
) -> Result<Served, ProxyError> {
    let limits = upstream.limits().clone();
    let refuse = |e: &ProxyError| {
        if let Some(a) = audit {
            a.record(
                "infer.mesh.failed",
                serde_json::json!({"peer": peer.node_id, "standing": peer.grant.as_ref().map(qualifies), "why": e.to_string()}),
            );
        }
        peer_reason(e).to_string()
    };
    let mut sink = FrameSink { stream };

    // Take the slots before reading anything: a peer that is over its
    // limit must not be able to make this node buffer a frame per stream.
    let Some(_permits) = gate.acquire(&peer.node_id) else {
        let e = ProxyError::Refused("too many concurrent requests".into());
        let why = refuse(&e);
        let _ = sink.send(Resp::Error(why.clone())).await;
        return Ok(Served::Failed(why));
    };
    let frame = match recv_timeout(sink.stream, &limits).await {
        Ok(f) => f,
        Err(e) => return Ok(Served::Failed(refuse(&e))),
    };
    let outcome: Result<(), ProxyError> = async {
        if !peer.grant.as_ref().is_some_and(qualifies) {
            return Err(ProxyError::Refused(
                "peer is not an enforced-admission full node".into(),
            ));
        }
        if frame.frame_type != FrameType::InferRequest {
            return Err(ProxyError::BadRequest("expected an infer request".into()));
        }
        let mut req = wire::decode_request(&frame.payload, &limits)?;
        if !mesh_path_allowed(&req.path) {
            return Err(ProxyError::Forbidden("path is not served to peers".into()));
        }
        let local = local_for_peer(&req.role, &peer.node_id).ok_or_else(|| {
            ProxyError::NoInstance(format!("{} (not served to this peer here)", req.role))
        })?;
        req.body = pin_body(&req.path, &req.body, &local)?;
        let fwd = upstream.forward(&local.base, &req, false, &mut sink);
        tokio::time::timeout(limits.request_timeout, fwd)
            .await
            .map_err(|_| ProxyError::Timeout("request".into()))??;
        sink.send(Resp::End).await
    }
    .await;
    match outcome {
        Ok(()) => Ok(Served::Ok),
        Err(e) => {
            let why = refuse(&e);
            let _ = sink.send(Resp::Error(why.clone())).await;
            Ok(Served::Failed(why))
        }
    }
}
