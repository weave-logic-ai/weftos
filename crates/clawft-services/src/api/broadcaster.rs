//! Topic-based message broadcasting for WebSocket clients.
//!
//! [`TopicBroadcaster`] manages a set of named broadcast channels so that
//! WebSocket handlers can subscribe clients to topics (e.g. `"sessions:abc"`,
//! `"agents"`) and the gateway dispatch loop can publish events that are
//! forwarded to all connected subscribers.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, broadcast};

/// Manages topic-based message broadcasting for WebSocket clients.
///
/// Each topic has a [`broadcast::Sender`] with a fixed capacity. Clients
/// subscribe by obtaining a [`broadcast::Receiver`] via [`subscribe`]. The
/// gateway dispatch loop (or any other producer) publishes via [`publish`].
#[derive(Clone)]
pub struct TopicBroadcaster {
    /// Map of topic name to broadcast sender.
    topics: Arc<RwLock<HashMap<String, broadcast::Sender<String>>>>,
}

impl TopicBroadcaster {
    /// Create a new, empty broadcaster.
    pub fn new() -> Self {
        Self {
            topics: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get or create a broadcast channel for a topic.
    ///
    /// If the topic does not yet exist, a new channel with capacity 256 is
    /// created. Returns a clone of the sender.
    pub async fn get_or_create(&self, topic: &str) -> broadcast::Sender<String> {
        let mut topics = self.topics.write().await;
        topics
            .entry(topic.to_string())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(256);
                tx
            })
            .clone()
    }

    /// Publish a message to a topic.
    ///
    /// If no subscribers are currently listening on the topic, the message is
    /// silently dropped.
    pub async fn publish(&self, topic: &str, message: serde_json::Value) {
        let topics = self.topics.read().await;
        if let Some(tx) = topics.get(topic) {
            // Ignore send errors (no active subscribers).
            let _ = tx.send(message.to_string());
        }
    }

    /// Subscribe to a topic, returning a broadcast receiver.
    ///
    /// Creates the topic channel if it does not yet exist.
    pub async fn subscribe(&self, topic: &str) -> broadcast::Receiver<String> {
        let tx = self.get_or_create(topic).await;
        tx.subscribe()
    }

    /// List all topic names that currently have channels.
    #[allow(dead_code)]
    pub async fn topics(&self) -> Vec<String> {
        let topics = self.topics.read().await;
        topics.keys().cloned().collect()
    }
}

impl Default for TopicBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribe_and_receive() {
        let bc = TopicBroadcaster::new();
        let mut rx = bc.subscribe("agents").await;

        let msg = serde_json::json!({"event": "agent_started", "name": "coder"});
        bc.publish("agents", msg.clone()).await;

        let received = rx.recv().await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&received).unwrap();
        assert_eq!(parsed, msg);
    }

    #[tokio::test]
    async fn publish_to_missing_topic_is_noop() {
        let bc = TopicBroadcaster::new();
        // Should not panic or error.
        bc.publish("nonexistent", serde_json::json!({"type": "test"}))
            .await;
    }

    #[tokio::test]
    async fn multiple_subscribers() {
        let bc = TopicBroadcaster::new();
        let mut rx1 = bc.subscribe("sessions").await;
        let mut rx2 = bc.subscribe("sessions").await;

        let msg = serde_json::json!({"type": "message_added"});
        bc.publish("sessions", msg.clone()).await;

        let r1 = rx1.recv().await.unwrap();
        let r2 = rx2.recv().await.unwrap();
        assert_eq!(r1, r2);
    }

    #[tokio::test]
    async fn topics_list() {
        let bc = TopicBroadcaster::new();
        let _rx1 = bc.subscribe("agents").await;
        let _rx2 = bc.subscribe("sessions").await;

        let mut topics = bc.topics().await;
        topics.sort();
        assert_eq!(topics, vec!["agents", "sessions"]);
    }
}
