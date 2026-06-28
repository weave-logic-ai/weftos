//! Agent identity registry.
//!
//! Maps opaque `agent_id` strings (UUID v4) to their Ed25519 public
//! keys. Registration requires a proof-of-possession signature so a
//! hostile client cannot register someone else's key.
//!
//! The registry is in-memory: restart-resets by design. Each kernel
//! boot chain-appends `agent.registered` events, so the audit trail
//! survives even though the registry itself does not.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;

/// A registered agent: name + public key + when it registered.
#[derive(Debug, Clone)]
pub struct RegisteredAgent {
    /// Public identifier assigned by the kernel (UUID v4).
    pub agent_id: String,
    /// Human-readable name supplied at registration.
    pub name: String,
    /// Ed25519 public key (32 bytes).
    pub pubkey: [u8; 32],
    /// When the agent registered.
    pub registered_at: DateTime<Utc>,
}

/// Agent identity registry.
///
/// Cheap to clone — the inner map is wrapped in `Arc`/`DashMap`.
#[derive(Debug, Default, Clone)]
pub struct AgentRegistry {
    inner: Arc<DashMap<String, RegisteredAgent>>,
}

impl AgentRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new agent with the given name and public key.
    ///
    /// Returns the freshly-minted `agent_id`. This function does not
    /// verify the proof-of-possession — the caller (daemon RPC
    /// handler) does that against `register_payload` before calling
    /// this method.
    pub fn register(&self, name: String, pubkey: [u8; 32]) -> RegisteredAgent {
        let agent_id = uuid::Uuid::new_v4().to_string();
        let entry = RegisteredAgent {
            agent_id: agent_id.clone(),
            name,
            pubkey,
            registered_at: Utc::now(),
        };
        self.inner.insert(agent_id, entry.clone());
        entry
    }

    /// Look up an agent by id.
    pub fn get(&self, agent_id: &str) -> Option<RegisteredAgent> {
        self.inner.get(agent_id).map(|e| e.clone())
    }

    /// Number of registered agents.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// List all registered agents.
    pub fn list(&self) -> Vec<RegisteredAgent> {
        self.inner.iter().map(|e| e.value().clone()).collect()
    }
}

/// Compose the canonical byte payload an agent must sign as the
/// proof-of-possession during `agent.register`.
///
/// Layout: `b"register\0" || name || b"\0" || pubkey || b"\0" || ts_le`.
///
/// The nonce (`ts`, unix-millis) binds the proof to a specific
/// instant so a replayed signature from an earlier session cannot be
/// re-used to register the same name by a different actor.
pub fn register_payload(name: &str, pubkey: &[u8; 32], ts: u64) -> Vec<u8> {
    let mut buf = Vec::with_capacity(9 + name.len() + 1 + 32 + 1 + 8);
    buf.extend_from_slice(b"register\0");
    buf.extend_from_slice(name.as_bytes());
    buf.push(0);
    buf.extend_from_slice(pubkey);
    buf.push(0);
    buf.extend_from_slice(&ts.to_le_bytes());
    buf
}

/// Compose the canonical byte payload signed for an `ipc.publish`.
///
/// Layout:
/// `b"ipc.publish\0" || topic || b"\0" || message || b"\0" || ts_le || b"\0" || actor_id`.
pub fn publish_payload(topic: &str, message: &str, ts: u64, actor_id: &str) -> Vec<u8> {
    let mut buf =
        Vec::with_capacity(12 + topic.len() + 1 + message.len() + 1 + 8 + 1 + actor_id.len());
    buf.extend_from_slice(b"ipc.publish\0");
    buf.extend_from_slice(topic.as_bytes());
    buf.push(0);
    buf.extend_from_slice(message.as_bytes());
    buf.push(0);
    buf.extend_from_slice(&ts.to_le_bytes());
    buf.push(0);
    buf.extend_from_slice(actor_id.as_bytes());
    buf
}

/// Compose the canonical byte payload signed for an
/// `ipc.subscribe_stream` request.
///
/// Layout: `b"ipc.subscribe\0" || topic || b"\0" || ts_le || b"\0" || actor_id`.
pub fn subscribe_payload(topic: &str, ts: u64, actor_id: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(14 + topic.len() + 1 + 8 + 1 + actor_id.len());
    buf.extend_from_slice(b"ipc.subscribe\0");
    buf.extend_from_slice(topic.as_bytes());
    buf.push(0);
    buf.extend_from_slice(&ts.to_le_bytes());
    buf.push(0);
    buf.extend_from_slice(actor_id.as_bytes());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_lookup() {
        let reg = AgentRegistry::new();
        let entry = reg.register("python-bridge".into(), [7u8; 32]);
        assert_eq!(reg.len(), 1);
        let fetched = reg.get(&entry.agent_id).unwrap();
        assert_eq!(fetched.name, "python-bridge");
        assert_eq!(fetched.pubkey, [7u8; 32]);
    }

    #[test]
    fn register_assigns_unique_ids() {
        let reg = AgentRegistry::new();
        let a = reg.register("a".into(), [1u8; 32]);
        let b = reg.register("b".into(), [2u8; 32]);
        assert_ne!(a.agent_id, b.agent_id);
    }

    #[test]
    fn payload_layouts_are_stable() {
        let pk = [0xAAu8; 32];
        let p = register_payload("foo", &pk, 42);
        // Prefix + name + sep
        assert_eq!(&p[..9], b"register\0");
        assert_eq!(&p[9..12], b"foo");
        assert_eq!(p[12], 0);
        assert_eq!(&p[13..45], &pk);
    }

    #[test]
    fn publish_payload_round() {
        let p1 = publish_payload("t1", "hi", 100, "aid");
        let p2 = publish_payload("t1", "hi", 100, "aid");
        assert_eq!(p1, p2);
        let p3 = publish_payload("t1", "hi", 101, "aid");
        assert_ne!(p1, p3, "ts must affect the payload");
    }
}
