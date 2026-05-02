//! Inter-agent communication bus.
//!
//! [`AgentBus`] provides per-agent inboxes for agent-to-agent messaging
//! with TTL enforcement and agent-scoped delivery (agents can only
//! read from their own inbox).
//!
//! # Security
//!
//! - Messages are tagged with agent IDs.
//! - Agents cannot read other agents' messages (inbox scoping).
//! - Bounded inbox size prevents memory exhaustion.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, mpsc};
use tracing::{debug, warn};

use clawft_types::agent_bus::{AgentBusError, InterAgentMessage};

/// Default inbox capacity per agent.
const DEFAULT_INBOX_CAPACITY: usize = 256;

/// Handle for an agent's inbox receiver.
///
/// This is returned from [`AgentBus::register_agent`] and provides
/// the only way to read messages from the agent's inbox.
pub struct AgentInbox {
    agent_id: String,
    rx: mpsc::Receiver<InterAgentMessage>,
}

impl AgentInbox {
    /// The agent ID that owns this inbox.
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Receive the next message from this inbox.
    ///
    /// Returns `None` if the bus has been dropped or the agent
    /// has been unregistered.
    pub async fn recv(&mut self) -> Option<InterAgentMessage> {
        loop {
            match self.rx.recv().await {
                Some(msg) => {
                    if msg.is_expired() {
                        warn!(
                            msg_id = %msg.id,
                            from = %msg.from_agent,
                            to = %msg.to_agent,
                            "dropping expired inter-agent message"
                        );
                        continue;
                    }
                    return Some(msg);
                }
                None => return None,
            }
        }
    }

    /// Try to receive a message without blocking.
    ///
    /// Returns `None` if no message is available.
    pub fn try_recv(&mut self) -> Option<InterAgentMessage> {
        loop {
            match self.rx.try_recv() {
                Ok(msg) => {
                    if msg.is_expired() {
                        warn!(
                            msg_id = %msg.id,
                            from = %msg.from_agent,
                            to = %msg.to_agent,
                            "dropping expired inter-agent message"
                        );
                        continue;
                    }
                    return Some(msg);
                }
                Err(_) => return None,
            }
        }
    }
}

/// Inter-agent communication bus with per-agent inboxes.
///
/// Agents register on the bus to get an inbox. Messages are delivered
/// to the recipient's inbox. Agents can only read their own inbox
/// (enforced by the [`AgentInbox`] handle).
///
/// # Thread safety
///
/// The bus uses `Arc<Mutex<..>>` internally for concurrent access.
/// Send operations use `try_send` to avoid blocking callers.
pub struct AgentBus {
    /// Per-agent inbox senders: agent_id -> sender.
    inboxes: Arc<Mutex<HashMap<String, mpsc::Sender<InterAgentMessage>>>>,
    /// Inbox capacity for new registrations.
    inbox_capacity: usize,
}

impl AgentBus {
    /// Create a new agent bus with default inbox capacity (256).
    pub fn new() -> Self {
        Self {
            inboxes: Arc::new(Mutex::new(HashMap::new())),
            inbox_capacity: DEFAULT_INBOX_CAPACITY,
        }
    }

    /// Create a new agent bus with custom inbox capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inboxes: Arc::new(Mutex::new(HashMap::new())),
            inbox_capacity: capacity,
        }
    }

    /// Register an agent on the bus and return its inbox handle.
    ///
    /// If the agent is already registered, the old inbox is replaced
    /// (messages in the old inbox are lost).
    pub async fn register_agent(&self, agent_id: &str) -> AgentInbox {
        let (tx, rx) = mpsc::channel(self.inbox_capacity);
        let mut inboxes = self.inboxes.lock().await;
        inboxes.insert(agent_id.to_string(), tx);

        debug!(agent_id = %agent_id, "agent registered on bus");

        AgentInbox {
            agent_id: agent_id.to_string(),
            rx,
        }
    }

    /// Unregister an agent from the bus.
    ///
    /// The agent's inbox sender is dropped, causing the receiver to
    /// return `None` on next recv.
    pub async fn unregister_agent(&self, agent_id: &str) {
        let mut inboxes = self.inboxes.lock().await;
        if inboxes.remove(agent_id).is_some() {
            debug!(agent_id = %agent_id, "agent unregistered from bus");
        }
    }

    /// Send a message to the specified agent's inbox.
    ///
    /// # Errors
    ///
    /// Returns [`AgentBusError::AgentNotFound`] if the target agent
    /// is not registered.
    /// Returns [`AgentBusError::InboxFull`] if the inbox has reached
    /// its capacity limit.
    /// Returns [`AgentBusError::MessageExpired`] if the message has
    /// already expired.
    pub async fn send(&self, msg: InterAgentMessage) -> Result<(), AgentBusError> {
        // Check TTL before delivery.
        if msg.is_expired() {
            warn!(
                msg_id = %msg.id,
                from = %msg.from_agent,
                to = %msg.to_agent,
                "dropping expired message before delivery"
            );
            return Err(AgentBusError::MessageExpired { ttl: msg.ttl });
        }

        let inboxes = self.inboxes.lock().await;
        let tx = inboxes
            .get(&msg.to_agent)
            .ok_or_else(|| AgentBusError::AgentNotFound(msg.to_agent.clone()))?;

        tx.try_send(msg).map_err(|e| match e {
            mpsc::error::TrySendError::Full(msg) => {
                AgentBusError::InboxFull(msg.to_agent.clone())
            }
            mpsc::error::TrySendError::Closed(msg) => {
                AgentBusError::AgentNotFound(msg.to_agent.clone())
            }
        })
    }

    /// Check whether an agent is registered on the bus.
    pub async fn is_registered(&self, agent_id: &str) -> bool {
        let inboxes = self.inboxes.lock().await;
        inboxes.contains_key(agent_id)
    }

    /// List all registered agent IDs.
    pub async fn registered_agents(&self) -> Vec<String> {
        let inboxes = self.inboxes.lock().await;
        inboxes.keys().cloned().collect()
    }
}

impl Default for AgentBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Coordinator for dispatching subtasks to worker agents and collecting
/// results via the [`AgentBus`].
///
/// Implements the coordinator pattern: a lead agent dispatches work to
/// worker agents and waits for their replies.
pub struct SwarmCoordinator {
    /// Shared agent bus for message delivery.
    bus: Arc<AgentBus>,
    /// Agent ID of the coordinator.
    coordinator_id: String,
    /// Registered worker agent IDs.
    worker_agents: Vec<String>,
}

impl SwarmCoordinator {
    /// Create a new coordinator.
    pub fn new(
        bus: Arc<AgentBus>,
        coordinator_id: impl Into<String>,
        worker_agents: Vec<String>,
    ) -> Self {
        Self {
            bus,
            coordinator_id: coordinator_id.into(),
            worker_agents,
        }
    }

    /// Dispatch a subtask to a specific worker agent.
    ///
    /// Sends an [`InterAgentMessage`] to the worker and returns the
    /// message ID for later correlation.
    pub async fn dispatch_subtask(
        &self,
        task: &str,
        worker: &str,
        payload: serde_json::Value,
        ttl: Duration,
    ) -> Result<uuid::Uuid, AgentBusError> {
        let msg = InterAgentMessage::new(
            &self.coordinator_id,
            worker,
            task,
            payload,
            ttl,
        );
        let msg_id = msg.id;
        self.bus.send(msg).await?;
        debug!(
            coordinator = %self.coordinator_id,
            worker = %worker,
            task = %task,
            msg_id = %msg_id,
            "dispatched subtask"
        );
        Ok(msg_id)
    }

    /// Dispatch the same task to all registered workers.
    ///
    /// Returns a list of (worker_id, message_id) pairs.
    pub async fn broadcast_task(
        &self,
        task: &str,
        payload: serde_json::Value,
        ttl: Duration,
    ) -> Vec<(String, Result<uuid::Uuid, AgentBusError>)> {
        let mut results = Vec::new();
        for worker in &self.worker_agents {
            let result = self
                .dispatch_subtask(task, worker, payload.clone(), ttl)
                .await;
            results.push((worker.clone(), result));
        }
        results
    }

    /// Coordinator agent ID.
    pub fn coordinator_id(&self) -> &str {
        &self.coordinator_id
    }

    /// List of registered worker agent IDs.
    pub fn workers(&self) -> &[String] {
        &self.worker_agents
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn register_and_send() {
        let bus = AgentBus::new();
        let mut inbox = bus.register_agent("agent-a").await;

        let msg = InterAgentMessage::new(
            "agent-b",
            "agent-a",
            "do work",
            json!({}),
            Duration::from_secs(60),
        );
        bus.send(msg).await.unwrap();

        let received = inbox.recv().await.unwrap();
        assert_eq!(received.from_agent, "agent-b");
        assert_eq!(received.to_agent, "agent-a");
        assert_eq!(received.task, "do work");
    }

    #[tokio::test]
    async fn send_to_unregistered_agent_fails() {
        let bus = AgentBus::new();
        let msg = InterAgentMessage::new(
            "sender",
            "nonexistent",
            "task",
            json!({}),
            Duration::from_secs(60),
        );
        let result = bus.send(msg).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AgentBusError::AgentNotFound(_)
        ));
    }

    #[tokio::test]
    async fn agent_isolation() {
        let bus = AgentBus::new();
        let mut inbox_a = bus.register_agent("agent-a").await;
        let mut inbox_b = bus.register_agent("agent-b").await;

        // Send to agent-a only.
        let msg = InterAgentMessage::new(
            "sender",
            "agent-a",
            "for a only",
            json!({}),
            Duration::from_secs(60),
        );
        bus.send(msg).await.unwrap();

        // agent-a should receive it.
        let received = inbox_a.try_recv();
        assert!(received.is_some());
        assert_eq!(received.unwrap().task, "for a only");

        // agent-b should not.
        let received_b = inbox_b.try_recv();
        assert!(received_b.is_none());
    }

    #[tokio::test]
    async fn unregister_agent() {
        let bus = AgentBus::new();
        let _inbox = bus.register_agent("agent-a").await;

        assert!(bus.is_registered("agent-a").await);
        bus.unregister_agent("agent-a").await;
        assert!(!bus.is_registered("agent-a").await);
    }

    #[tokio::test]
    async fn inbox_full_backpressure() {
        let bus = AgentBus::with_capacity(2);
        let _inbox = bus.register_agent("agent-a").await;

        // Fill inbox.
        for i in 0..2 {
            let msg = InterAgentMessage::new(
                "sender",
                "agent-a",
                format!("task-{i}"),
                json!({}),
                Duration::from_secs(60),
            );
            bus.send(msg).await.unwrap();
        }

        // Third should fail.
        let msg = InterAgentMessage::new(
            "sender",
            "agent-a",
            "overflow",
            json!({}),
            Duration::from_secs(60),
        );
        let result = bus.send(msg).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AgentBusError::InboxFull(_)));
    }

    #[tokio::test]
    async fn registered_agents_list() {
        let bus = AgentBus::new();
        bus.register_agent("a").await;
        bus.register_agent("b").await;

        let agents = bus.registered_agents().await;
        assert_eq!(agents.len(), 2);
        assert!(agents.contains(&"a".to_string()));
        assert!(agents.contains(&"b".to_string()));
    }

    #[tokio::test]
    async fn coordinator_dispatch() {
        let bus = Arc::new(AgentBus::new());
        let _inbox = bus.register_agent("worker-1").await;

        let coord = SwarmCoordinator::new(
            bus.clone(),
            "coordinator",
            vec!["worker-1".into()],
        );

        let msg_id = coord
            .dispatch_subtask("subtask-1", "worker-1", json!({"data": 42}), Duration::from_secs(60))
            .await
            .unwrap();

        // Message ID should be valid.
        assert!(!msg_id.is_nil());
    }

    #[tokio::test]
    async fn coordinator_broadcast() {
        let bus = Arc::new(AgentBus::new());
        let _inbox1 = bus.register_agent("w1").await;
        let _inbox2 = bus.register_agent("w2").await;

        let coord = SwarmCoordinator::new(
            bus.clone(),
            "coord",
            vec!["w1".into(), "w2".into()],
        );

        let results = coord
            .broadcast_task("broadcast-task", json!({}), Duration::from_secs(60))
            .await;

        assert_eq!(results.len(), 2);
        for (_, result) in &results {
            assert!(result.is_ok());
        }
    }

    #[test]
    fn agent_bus_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AgentBus>();
    }
}
