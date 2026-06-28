//! Named pipes for persistent IPC channels.
//!
//! [`NamedPipeRegistry`] manages persistent named communication channels
//! that survive agent restarts. Pipes support multiple senders (fan-in)
//! delivering to a single receiver endpoint.

use std::sync::RwLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::error::{KernelError, KernelResult};
use crate::ipc::KernelMessage;

/// Default pipe channel capacity.
const DEFAULT_PIPE_CAPACITY: usize = 256;

/// Default TTL after all references are dropped (seconds).
const DEFAULT_TTL_SECS: u64 = 60;

/// Metadata about a named pipe (serializable snapshot).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipeInfo {
    /// Pipe name.
    pub name: String,
    /// Channel capacity.
    pub capacity: usize,
    /// When the pipe was created.
    pub created_at: DateTime<Utc>,
    /// Number of connected receivers.
    pub receiver_count: u32,
    /// Number of connected senders.
    pub sender_count: u32,
    /// TTL after all references are dropped.
    pub ttl_after_empty_secs: u64,
}

/// A named pipe with channel-based message delivery.
pub struct NamedPipe {
    /// Pipe name.
    pub name: String,
    /// Channel capacity.
    pub capacity: usize,
    /// When the pipe was created.
    pub created_at: DateTime<Utc>,
    /// Sender half of the pipe channel (cloneable for multiple senders).
    sender: mpsc::Sender<KernelMessage>,
    /// Number of connected receivers.
    pub receiver_count: AtomicU32,
    /// Number of connected senders.
    pub sender_count: AtomicU32,
    /// TTL after all references are dropped.
    pub ttl_after_empty: Duration,
    /// Last time the pipe was actively used.
    pub last_active: RwLock<Instant>,
}

impl NamedPipe {
    /// Get a serializable info snapshot.
    pub fn info(&self) -> PipeInfo {
        PipeInfo {
            name: self.name.clone(),
            capacity: self.capacity,
            created_at: self.created_at,
            receiver_count: self.receiver_count.load(Ordering::Relaxed),
            sender_count: self.sender_count.load(Ordering::Relaxed),
            ttl_after_empty_secs: self.ttl_after_empty.as_secs(),
        }
    }

    /// Touch the pipe to mark it as active.
    fn touch(&self) {
        if let Ok(mut last) = self.last_active.write() {
            *last = Instant::now();
        }
    }

    /// Check if the pipe has expired (no references and TTL elapsed).
    pub fn is_expired(&self) -> bool {
        let senders = self.sender_count.load(Ordering::Relaxed);
        let receivers = self.receiver_count.load(Ordering::Relaxed);
        if senders > 0 || receivers > 0 {
            return false;
        }
        if let Ok(last) = self.last_active.read() {
            last.elapsed() >= self.ttl_after_empty
        } else {
            false
        }
    }
}

/// Registry of named pipes for persistent IPC channels.
pub struct NamedPipeRegistry {
    pipes: DashMap<String, NamedPipe>,
}

impl NamedPipeRegistry {
    /// Create a new empty pipe registry.
    pub fn new() -> Self {
        Self {
            pipes: DashMap::new(),
        }
    }

    /// Create a named pipe with default capacity and TTL.
    ///
    /// Returns the receiver half of the pipe channel.
    /// If a pipe with this name already exists, returns an error.
    pub fn create(&self, name: impl Into<String>) -> KernelResult<mpsc::Receiver<KernelMessage>> {
        self.create_with_options(name, DEFAULT_PIPE_CAPACITY, DEFAULT_TTL_SECS)
    }

    /// Create a named pipe with specific capacity and TTL.
    pub fn create_with_options(
        &self,
        name: impl Into<String>,
        capacity: usize,
        ttl_secs: u64,
    ) -> KernelResult<mpsc::Receiver<KernelMessage>> {
        let name = name.into();
        if self.pipes.contains_key(&name) {
            return Err(KernelError::Ipc(format!(
                "named pipe '{name}' already exists"
            )));
        }

        let (tx, rx) = mpsc::channel(capacity);
        let pipe = NamedPipe {
            name: name.clone(),
            capacity,
            created_at: Utc::now(),
            sender: tx,
            receiver_count: AtomicU32::new(1),
            sender_count: AtomicU32::new(0),
            ttl_after_empty: Duration::from_secs(ttl_secs),
            last_active: RwLock::new(Instant::now()),
        };

        self.pipes.insert(name, pipe);
        Ok(rx)
    }

    /// Connect to an existing named pipe as a sender.
    ///
    /// Returns a sender that can push messages into the pipe.
    pub fn connect_sender(&self, name: &str) -> KernelResult<mpsc::Sender<KernelMessage>> {
        let pipe = self
            .pipes
            .get(name)
            .ok_or_else(|| KernelError::Ipc(format!("named pipe '{name}' not found")))?;

        pipe.sender_count.fetch_add(1, Ordering::Relaxed);
        pipe.touch();
        Ok(pipe.sender.clone())
    }

    /// Send a message to a named pipe.
    ///
    /// This is a convenience method that looks up the pipe and sends
    /// directly, without requiring the caller to hold a sender.
    pub async fn send(&self, name: &str, msg: KernelMessage) -> KernelResult<()> {
        let pipe = self
            .pipes
            .get(name)
            .ok_or_else(|| KernelError::Ipc(format!("named pipe '{name}' not found")))?;

        pipe.touch();
        pipe.sender.try_send(msg).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => {
                KernelError::Ipc(format!("named pipe '{name}' is full"))
            }
            mpsc::error::TrySendError::Closed(_) => {
                KernelError::Ipc(format!("named pipe '{name}' receiver dropped"))
            }
        })
    }

    /// Disconnect a sender from a named pipe.
    pub fn disconnect_sender(&self, name: &str) {
        if let Some(pipe) = self.pipes.get(name) {
            let prev = pipe.sender_count.fetch_sub(1, Ordering::Relaxed);
            if prev == 0 {
                // Underflow protection
                pipe.sender_count.store(0, Ordering::Relaxed);
            }
            pipe.touch();
        }
    }

    /// Disconnect a receiver from a named pipe.
    pub fn disconnect_receiver(&self, name: &str) {
        if let Some(pipe) = self.pipes.get(name) {
            let prev = pipe.receiver_count.fetch_sub(1, Ordering::Relaxed);
            if prev == 0 {
                pipe.receiver_count.store(0, Ordering::Relaxed);
            }
            pipe.touch();
        }
    }

    /// Remove a named pipe.
    pub fn remove(&self, name: &str) -> bool {
        self.pipes.remove(name).is_some()
    }

    /// Check if a named pipe exists.
    pub fn exists(&self, name: &str) -> bool {
        self.pipes.contains_key(name)
    }

    /// List all pipe names.
    pub fn list(&self) -> Vec<String> {
        self.pipes.iter().map(|e| e.key().clone()).collect()
    }

    /// Get info about a specific pipe.
    pub fn info(&self, name: &str) -> Option<PipeInfo> {
        self.pipes.get(name).map(|p| p.info())
    }

    /// Get info about all pipes.
    pub fn list_info(&self) -> Vec<PipeInfo> {
        self.pipes.iter().map(|e| e.value().info()).collect()
    }

    /// Number of registered pipes.
    pub fn len(&self) -> usize {
        self.pipes.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.pipes.is_empty()
    }

    /// Remove pipes that have expired (no references and TTL elapsed).
    ///
    /// Returns the names of removed pipes.
    pub fn cleanup_expired(&self) -> Vec<String> {
        let expired: Vec<String> = self
            .pipes
            .iter()
            .filter(|e| e.value().is_expired())
            .map(|e| e.key().clone())
            .collect();

        for name in &expired {
            self.pipes.remove(name);
        }

        expired
    }
}

impl Default for NamedPipeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Registry trait implementation ────────────────────────────────────

impl clawft_types::Registry for NamedPipeRegistry {
    type Value = PipeInfo;

    fn get(&self, key: &str) -> Option<Self::Value> {
        self.info(key)
    }

    fn list_keys(&self) -> Vec<String> {
        self.list()
    }

    fn contains(&self, key: &str) -> bool {
        self.exists(key)
    }

    fn count(&self) -> usize {
        self.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{MessagePayload, MessageTarget};

    fn make_msg(text: &str) -> KernelMessage {
        KernelMessage::new(
            1,
            MessageTarget::Process(2),
            MessagePayload::Text(text.into()),
        )
    }

    #[test]
    fn create_and_exists() {
        let registry = NamedPipeRegistry::new();
        let _rx = registry.create("test-pipe").unwrap();
        assert!(registry.exists("test-pipe"));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn create_duplicate_fails() {
        let registry = NamedPipeRegistry::new();
        let _rx = registry.create("dup").unwrap();
        let err = registry.create("dup").unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn send_and_receive_roundtrip() {
        let registry = NamedPipeRegistry::new();
        let mut rx = registry.create("pipe-1").unwrap();

        let msg = make_msg("hello pipe");
        registry.send("pipe-1", msg).await.unwrap();

        let received = rx.recv().await.unwrap();
        match &received.payload {
            MessagePayload::Text(t) => assert_eq!(t, "hello pipe"),
            other => panic!("expected Text, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn multiple_senders_fan_in() {
        let registry = NamedPipeRegistry::new();
        let mut rx = registry.create("fan-in").unwrap();

        let tx1 = registry.connect_sender("fan-in").unwrap();
        let tx2 = registry.connect_sender("fan-in").unwrap();

        tx1.send(make_msg("from-1")).await.unwrap();
        tx2.send(make_msg("from-2")).await.unwrap();

        let m1 = rx.recv().await.unwrap();
        let m2 = rx.recv().await.unwrap();

        let texts: Vec<String> = [m1, m2]
            .iter()
            .filter_map(|m| match &m.payload {
                MessagePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"from-1".to_string()));
        assert!(texts.contains(&"from-2".to_string()));
    }

    #[tokio::test]
    async fn capacity_limit_returns_error() {
        let registry = NamedPipeRegistry::new();
        let _rx = registry.create_with_options("tiny", 2, 60).unwrap();

        // Fill the pipe
        registry.send("tiny", make_msg("1")).await.unwrap();
        registry.send("tiny", make_msg("2")).await.unwrap();

        // Third should fail
        let err = registry.send("tiny", make_msg("3")).await.unwrap_err();
        assert!(err.to_string().contains("full"));
    }

    #[test]
    fn send_to_nonexistent_pipe_fails() {
        let registry = NamedPipeRegistry::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let err = rt
            .block_on(registry.send("nope", make_msg("x")))
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn connect_sender_to_nonexistent_fails() {
        let registry = NamedPipeRegistry::new();
        let err = registry.connect_sender("nope").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn remove_pipe() {
        let registry = NamedPipeRegistry::new();
        let _rx = registry.create("removable").unwrap();
        assert!(registry.exists("removable"));
        assert!(registry.remove("removable"));
        assert!(!registry.exists("removable"));
    }

    #[test]
    fn list_pipes() {
        let registry = NamedPipeRegistry::new();
        let _r1 = registry.create("alpha").unwrap();
        let _r2 = registry.create("beta").unwrap();

        let names = registry.list();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
    }

    #[test]
    fn pipe_info() {
        let registry = NamedPipeRegistry::new();
        let _rx = registry.create_with_options("info-pipe", 128, 120).unwrap();

        let info = registry.info("info-pipe").unwrap();
        assert_eq!(info.name, "info-pipe");
        assert_eq!(info.capacity, 128);
        assert_eq!(info.receiver_count, 1);
        assert_eq!(info.sender_count, 0);
        assert_eq!(info.ttl_after_empty_secs, 120);
    }

    #[test]
    fn sender_count_tracking() {
        let registry = NamedPipeRegistry::new();
        let _rx = registry.create("counted").unwrap();

        let _tx1 = registry.connect_sender("counted").unwrap();
        let _tx2 = registry.connect_sender("counted").unwrap();

        let info = registry.info("counted").unwrap();
        assert_eq!(info.sender_count, 2);

        registry.disconnect_sender("counted");
        let info = registry.info("counted").unwrap();
        assert_eq!(info.sender_count, 1);
    }

    #[test]
    fn ttl_cleanup() {
        let registry = NamedPipeRegistry::new();
        // Create a pipe with 0-second TTL
        let _rx = registry.create_with_options("ephemeral", 16, 0).unwrap();

        // Disconnect the receiver so ref counts go to zero
        registry.disconnect_receiver("ephemeral");

        // It should be expired now (TTL=0)
        let expired = registry.cleanup_expired();
        assert_eq!(expired, vec!["ephemeral".to_string()]);
        assert!(!registry.exists("ephemeral"));
    }

    #[test]
    fn pipe_not_expired_with_senders() {
        let registry = NamedPipeRegistry::new();
        let _rx = registry.create_with_options("active", 16, 0).unwrap();
        let _tx = registry.connect_sender("active").unwrap();

        // Even with TTL=0, pipe has senders so not expired
        let expired = registry.cleanup_expired();
        assert!(expired.is_empty());
    }

    #[test]
    fn default_registry() {
        let registry = NamedPipeRegistry::default();
        assert!(registry.is_empty());
    }

    #[test]
    fn pipe_info_serde_roundtrip() {
        let info = PipeInfo {
            name: "test".into(),
            capacity: 256,
            created_at: Utc::now(),
            receiver_count: 1,
            sender_count: 2,
            ttl_after_empty_secs: 60,
        };
        let json = serde_json::to_string(&info).unwrap();
        let restored: PipeInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "test");
        assert_eq!(restored.sender_count, 2);
    }
}
