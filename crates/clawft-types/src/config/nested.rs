//! ADR-103 D10: master-owned nested user-instance configuration.
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// Instance authority; independent of a project's `weave.master` flag.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeaveConfig {
    #[serde(default)]
    pub master: bool,
    /// Present only in a supervisor-generated config; requires signed boot material.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nested: Option<NestedInstance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NestedInstance {
    pub id: String,
    pub parent: String,
    pub depth: u32,
}

/// Outer mesh membership is never implicit or machine-service based.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "level", rename_all = "snake_case", deny_unknown_fields)]
pub enum NestedRegistration {
    #[default]
    Isolated,
    Collapsed {
        inner_key_id: String,
        listen: SocketAddr,
        genesis_hash: String,
        /// Each entry must be `socket-address#node-id`; no discovery/TOFU.
        peers: Vec<String>,
    },
}

impl NestedRegistration {
    pub fn validate(&self, key_id: &str) -> Result<(), String> {
        if let Self::Collapsed {
            inner_key_id,
            listen,
            genesis_hash,
            peers,
        } = self
        {
            if inner_key_id != key_id || listen.port() == 0 || !listen.ip().is_loopback() {
                return Err(
                    "mesh grant must bind the inner key and a nonzero loopback port".into(),
                );
            }
            let hex = |s: &str, n| {
                s.len() == n
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            };
            if !hex(genesis_hash, 64) || peers.is_empty() {
                return Err("mesh grant requires genesis and pinned peers".into());
            }
            for peer in peers {
                let (addr, id) = peer.rsplit_once('#').ok_or("peer must pin node-id")?;
                let addr: SocketAddr = addr
                    .parse()
                    .map_err(|_| "peer must use a literal socket address")?;
                if addr.port() == 0 || !hex(id, 32) {
                    return Err("invalid pinned peer".into());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grants_require_identity_ports_and_pinned_peers() {
        let mut grant = NestedRegistration::Collapsed {
            inner_key_id: "a".repeat(32),
            listen: "127.0.0.1:9490".parse().unwrap(),
            genesis_hash: "b".repeat(64),
            peers: vec![format!("127.0.0.1:9489#{}", "c".repeat(32))],
        };
        assert!(grant.validate(&"a".repeat(32)).is_ok());
        assert!(grant.validate(&"d".repeat(32)).is_err());
        if let NestedRegistration::Collapsed { peers, .. } = &mut grant {
            *peers = vec!["127.0.0.1:9489".into()];
        }
        assert!(grant.validate(&"a".repeat(32)).is_err());
        assert!(serde_json::from_str::<NestedRegistration>(r#"{"level":"service"}"#).is_err());
    }
}

impl WeaveConfig {
    pub fn is_default(&self) -> bool {
        !self.master && self.nested.is_none()
    }
}
