//! Mesh wire format of a forwarded inference request (frame types
//! `InferRequest` 0x10 and `InferResponse` 0x11).
//!
//! ```text
//! InferRequest   [ver u8][head_len u32 BE][head JSON][body bytes]
//! InferResponse  [kind u8][data]     kind 0 head (JSON {status, content_type})
//!                                    kind 1 chunk (raw bytes)
//!                                    kind 2 end
//!                                    kind 3 error (UTF-8 text)
//! ```
//!
//! The request carries no `Authorization`: the credentials of the client on
//! one node are not forwarded to another. Everything decoded from the wire
//! is revalidated as if it came from a client, because it came from a peer.

use serde::{Deserialize, Serialize};

use super::http::{safe_header_value, validate_path};
use super::types::{Method, ProxyError, ProxyLimits, ProxyRequest};
use crate::workload_pkg::manifest::valid_token;

/// Wire version of `InferRequest`.
pub const WIRE_VERSION: u8 = 1;
/// Largest body chunk put in one response frame.
pub const CHUNK_BYTES: usize = 64 * 1024;
/// Longest role name.
pub const MAX_ROLE: usize = 64;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestHead {
    role: String,
    method: String,
    path: String,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    accept: Option<String>,
}

/// Encode a request for the mesh (without its `Authorization`).
pub fn encode_request(req: &ProxyRequest) -> Result<Vec<u8>, ProxyError> {
    let head = serde_json::to_vec(&RequestHead {
        role: req.role.clone(),
        method: req.method.as_str().to_string(),
        path: req.path.clone(),
        content_type: req.content_type.clone(),
        accept: req.accept.clone(),
    })
    .map_err(|e| ProxyError::Mesh(e.to_string()))?;
    let mut out = Vec::with_capacity(5 + head.len() + req.body.len());
    out.push(WIRE_VERSION);
    out.extend_from_slice(&(head.len() as u32).to_be_bytes());
    out.extend_from_slice(&head);
    out.extend_from_slice(&req.body);
    Ok(out)
}

/// Decode and revalidate a request received from a peer.
pub fn decode_request(payload: &[u8], limits: &ProxyLimits) -> Result<ProxyRequest, ProxyError> {
    let bad = |m: &str| ProxyError::BadRequest(m.into());
    if payload.len() < 5 || payload[0] != WIRE_VERSION {
        return Err(bad("unsupported infer request version"));
    }
    let head_len = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]) as usize;
    if head_len > limits.max_head_bytes || payload.len() < 5 + head_len {
        return Err(bad("bad infer request head length"));
    }
    let head: RequestHead =
        serde_json::from_slice(&payload[5..5 + head_len]).map_err(|_| bad("bad infer request head"))?;
    let body = &payload[5 + head_len..];
    if body.len() > limits.max_request_body {
        return Err(ProxyError::TooLarge("request body".into()));
    }
    if !valid_token(&head.role, MAX_ROLE) {
        return Err(bad("bad role"));
    }
    let method = match head.method.as_str() {
        "GET" => Method::Get,
        "POST" => Method::Post,
        _ => return Err(ProxyError::MethodNotAllowed),
    };
    validate_path(&head.path)?;
    let clean = |v: Option<String>| -> Result<Option<String>, ProxyError> {
        match v {
            None => Ok(None),
            Some(s) if safe_header_value(&s).is_some() => Ok(Some(s)),
            Some(_) => Err(bad("bad header value")),
        }
    };
    Ok(ProxyRequest {
        role: head.role,
        method,
        path: head.path,
        content_type: clean(head.content_type)?,
        accept: clean(head.accept)?,
        authorization: None,
        body: body.to_vec(),
    })
}

/// One response frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resp {
    /// Status and content type.
    Head {
        /// HTTP status.
        status: u16,
        /// Content type.
        content_type: Option<String>,
    },
    /// Body bytes.
    Chunk(Vec<u8>),
    /// Complete.
    End,
    /// Failed; no more frames.
    Error(String),
}

#[derive(Serialize, Deserialize)]
struct HeadJson {
    status: u16,
    #[serde(default)]
    content_type: Option<String>,
}

/// Encode a response frame payload.
pub fn encode_resp(r: &Resp) -> Vec<u8> {
    match r {
        Resp::Head {
            status,
            content_type,
        } => {
            let mut v = vec![0];
            v.extend(
                serde_json::to_vec(&HeadJson {
                    status: *status,
                    content_type: content_type.clone(),
                })
                .unwrap_or_default(),
            );
            v
        }
        Resp::Chunk(d) => {
            let mut v = Vec::with_capacity(1 + d.len());
            v.push(1);
            v.extend_from_slice(d);
            v
        }
        Resp::End => vec![2],
        Resp::Error(m) => {
            let mut v = vec![3];
            v.extend_from_slice(m.as_bytes());
            v
        }
    }
}

/// Decode a response frame payload received from a peer.
pub fn decode_resp(p: &[u8]) -> Result<Resp, ProxyError> {
    let bad = |m: &str| ProxyError::Mesh(m.into());
    let (kind, data) = p.split_first().ok_or_else(|| bad("empty infer response"))?;
    match kind {
        0 => {
            let h: HeadJson = serde_json::from_slice(data).map_err(|_| bad("bad response head"))?;
            if !(200..=599).contains(&h.status) {
                return Err(bad("bad response status"));
            }
            Ok(Resp::Head {
                status: h.status,
                content_type: h
                    .content_type
                    .filter(|c| safe_header_value(c).is_some()),
            })
        }
        1 => Ok(Resp::Chunk(data.to_vec())),
        2 => Ok(Resp::End),
        3 => Ok(Resp::Error(
            String::from_utf8_lossy(&data[..data.len().min(512)]).into_owned(),
        )),
        _ => Err(bad("unknown infer response kind")),
    }
}
