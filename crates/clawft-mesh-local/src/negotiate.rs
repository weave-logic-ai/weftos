//! Version negotiation and the service's public record.

use serde::{Deserialize, Serialize};

use crate::hexser::hex32;
use crate::proto::{PROTO_MAX, PROTO_MIN};

/// An inclusive supported-version range, optionally with a build sha.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRange {
    pub min: u32,
    pub max: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("no common mesh-local protocol version")]
pub struct ProtoMismatch {
    pub service: VersionRange,
    pub client: VersionRange,
}

impl ProtoMismatch {
    /// The one-line fix, depending on which side is behind.
    pub fn remedy(&self) -> &'static str {
        if self.client.min > self.service.max {
            "restart the service after `weaver update`"
        } else {
            "update the user daemon"
        }
    }
}

/// `proto = min(client.max, service.max)`, required to be at least
/// `max(client.min, service.min)`. Ranges are `(min, max)`.
pub fn negotiate(service: (u32, u32), client: (u32, u32)) -> Result<u32, ProtoMismatch> {
    let proto = client.1.min(service.1);
    if client.0 <= client.1 && service.0 <= service.1 && proto >= client.0.max(service.0) {
        Ok(proto)
    } else {
        Err(ProtoMismatch {
            service: VersionRange { min: service.0, max: service.1, sha: None },
            client: VersionRange { min: client.0, max: client.1, sha: None },
        })
    }
}

/// Negotiate against this build's own range.
pub fn negotiate_service(client_min: u32, client_max: u32) -> Result<u32, ProtoMismatch> {
    negotiate((PROTO_MIN, PROTO_MAX), (client_min, client_max))
}

/// Intersection of two feature lists, in `a` order, without duplicates.
/// Unknown features are ignored, never an error.
pub fn negotiate_features(a: &[String], b: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in a {
        if b.contains(f) && !out.contains(f) {
            out.push(f.clone());
        }
    }
    out
}

/// The public record the service writes (`service.json`) that clients pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceRecord {
    pub node_id: String,
    #[serde(with = "hex32")]
    pub machine_pubkey: [u8; 32],
    pub service_uid: u32,
    pub proto: VersionRange,
    #[serde(default)]
    pub build_sha: String,
    #[serde(default)]
    pub started_at: u64,
}

impl ServiceRecord {
    pub fn load(path: &std::path::Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}
