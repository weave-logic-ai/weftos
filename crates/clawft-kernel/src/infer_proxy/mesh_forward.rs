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

use async_trait::async_trait;

use super::types::{MeshDialer, ProxyAudit, ProxyError, ProxyLimits, ProxyRequest, ResponseSink};
use super::upstream::Upstream;
use super::wire::{self, Resp};
use crate::mesh::MeshStream;
use crate::mesh::MAX_MESSAGE_SIZE;
use crate::mesh_framing::{FrameType, MeshFrame};

/// Send a frame the way the artifact protocol does: the transport carries
/// `[len u32][type][payload]` as one message.
pub async fn write_frame(s: &mut dyn MeshStream, f: &MeshFrame) -> Result<(), ProxyError> {
    let wire = f.encode().map_err(mesh)?;
    s.send(&wire).await.map_err(mesh)
}

/// Receive one frame sent by [`write_frame`]; the declared length must
/// match the message exactly and stay under the mesh cap.
pub async fn read_frame(s: &mut dyn MeshStream) -> Result<MeshFrame, ProxyError> {
    let raw = s.recv().await.map_err(mesh)?;
    if raw.len() < 5 {
        return Err(mesh("short frame"));
    }
    let declared = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
    if declared > MAX_MESSAGE_SIZE || declared != raw.len() - 4 {
        return Err(mesh("bad frame length"));
    }
    MeshFrame::decode(&raw[4..]).map_err(mesh)
}

fn mesh(e: impl std::fmt::Display) -> ProxyError {
    ProxyError::Mesh(e.to_string())
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
    /// True only when the id was verified against the authenticated
    /// connection (a claimed id is not enough).
    pub verified: bool,
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

/// Serve one forwarded request from `peer` on `stream`. `local_for_mesh`
/// maps a role to the base URL of the loopback instance this node exposes
/// to the mesh for it (or `None`).
pub async fn serve_infer(
    stream: &mut dyn MeshStream,
    peer: &InferPeer,
    local_for_mesh: &(dyn Fn(&str) -> Option<String> + Sync),
    upstream: &Upstream,
    audit: Option<&dyn ProxyAudit>,
) -> Result<Served, ProxyError> {
    let limits = upstream.limits().clone();
    let refuse = |e: &ProxyError| {
        let why = e.to_string();
        if let (Some(a), ProxyError::Refused(_) | ProxyError::NoInstance(_)) = (audit, e) {
            a.record(
                "infer.mesh.refused",
                serde_json::json!({"peer": peer.node_id, "verified": peer.verified, "why": why}),
            );
        }
        why
    };
    let mut sink = FrameSink { stream };

    let frame = match recv_timeout(sink.stream, &limits).await {
        Ok(f) => f,
        Err(e) => {
            return Ok(Served::Failed(refuse(&e)));
        }
    };
    let outcome: Result<(), ProxyError> = async {
        if !peer.verified {
            return Err(ProxyError::Refused("peer identity is not verified".into()));
        }
        if frame.frame_type != FrameType::InferRequest {
            return Err(ProxyError::BadRequest("expected an infer request".into()));
        }
        let req = wire::decode_request(&frame.payload, &limits)?;
        let base = local_for_mesh(&req.role).ok_or_else(|| {
            ProxyError::NoInstance(format!("{} (not served to the mesh here)", req.role))
        })?;
        let fwd = upstream.forward(&base, &req, false, &mut sink);
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
