//! `mesh` reference adapter — promotes the tray's Mesh chip from
//! "service named `mesh` is registered" to "cluster has peers and
//! cluster.status is fresh."
//!
//! Polls the existing `cluster.status` and `cluster.nodes` RPC verbs.
//! No new kernel surface — these verbs were added during K2 and have
//! been available on the UDS socket ever since.
//!
//! ## Topics
//!
//! | Topic | Shape | Refresh |
//! |-------|-------|---------|
//! | `substrate/mesh/status` | `{total_nodes, healthy_nodes, consensus_enabled, total_shards, active_shards}` | 3s |
//! | `substrate/mesh/nodes` | `[{node_id, name, platform, state, address?, last_seen?}]` | 3s |
//!
//! Public; no install-time permission required (cluster topology is
//! system metadata, not user content).

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use clawft_rpc::DaemonClient;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::adapter::{
    AdapterError, BufferPolicy, OntologyAdapter, PermissionReq, RefreshHint, Sensitivity, SubId,
    Subscription, TopicDecl,
};
use crate::delta::StateDelta;

const CHAN: usize = 4;

/// Declared topics.
pub const TOPICS: &[TopicDecl] = &[
    TopicDecl {
        path: "substrate/mesh/status",
        shape: "ontology://mesh-status",
        refresh_hint: RefreshHint::Periodic { ms: 3000 },
        sensitivity: Sensitivity::Public,
        buffer_policy: BufferPolicy::Refuse,
        max_len: None,
    },
    TopicDecl {
        path: "substrate/mesh/nodes",
        shape: "ontology://mesh-nodes",
        refresh_hint: RefreshHint::Periodic { ms: 3000 },
        sensitivity: Sensitivity::Public,
        buffer_policy: BufferPolicy::BlockCapped,
        max_len: None,
    },
];

/// Permissions — none. Cluster topology is public metadata.
pub const PERMISSIONS: &[PermissionReq] = &[];

type CancelTx = oneshot::Sender<()>;

struct Registry {
    next_id: u64,
    live: HashMap<SubId, CancelTx>,
}

impl Registry {
    fn new() -> Self {
        Self {
            next_id: 1,
            live: HashMap::new(),
        }
    }

    fn allocate(&mut self) -> SubId {
        let id = SubId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

/// Mesh adapter — calls the daemon's cluster.* RPC verbs.
pub struct MeshAdapter {
    reg: Mutex<Registry>,
}

impl Default for MeshAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl MeshAdapter {
    /// Build a new adapter. Connects to the daemon on first poll.
    pub fn new() -> Self {
        Self {
            reg: Mutex::new(Registry::new()),
        }
    }
}

#[async_trait]
impl OntologyAdapter for MeshAdapter {
    fn id(&self) -> &'static str {
        "mesh"
    }

    fn topics(&self) -> &'static [TopicDecl] {
        TOPICS
    }

    fn permissions(&self) -> &'static [PermissionReq] {
        PERMISSIONS
    }

    async fn open(&self, topic: &str, _args: Value) -> Result<Subscription, AdapterError> {
        let rpc_method: &'static str = match topic {
            "substrate/mesh/status" => "cluster.status",
            "substrate/mesh/nodes" => "cluster.nodes",
            other => return Err(AdapterError::UnknownTopic(other.into())),
        };
        let id = {
            let mut reg = self.reg.lock();
            reg.allocate()
        };
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (tx, rx) = mpsc::channel::<StateDelta>(CHAN);
        self.reg.lock().live.insert(id, cancel_tx);

        let topic_path = topic.to_string();
        tokio::spawn(async move {
            poll_rpc(topic_path, rpc_method, tx, cancel_rx).await;
        });
        Ok(Subscription { id, rx })
    }

    async fn close(&self, sub_id: SubId) -> Result<(), AdapterError> {
        let _ = self.reg.lock().live.remove(&sub_id);
        Ok(())
    }
}

async fn poll_rpc(
    topic: String,
    rpc_method: &'static str,
    tx: mpsc::Sender<StateDelta>,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    let mut client: Option<DaemonClient> = None;
    let mut ticker = tokio::time::interval(Duration::from_secs(3));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = &mut cancel_rx => return,
            _ = ticker.tick() => {
                if client.is_none() {
                    client = DaemonClient::connect().await;
                }
                let Some(c) = client.as_mut() else {
                    // Daemon unreachable — emit an explicit "unreachable"
                    // so the tray can distinguish from "no peers."
                    let delta = StateDelta::Replace {
                        path: topic.clone(),
                        value: json!({ "available": false, "reason": "daemon-unreachable" }),
                    };
                    if tx.send(delta).await.is_err() {
                        return;
                    }
                    continue;
                };
                match c.simple_call(rpc_method).await {
                    Ok(resp) if resp.ok => {
                        let delta = StateDelta::Replace {
                            path: topic.clone(),
                            value: resp.result.unwrap_or(Value::Null),
                        };
                        if tx.send(delta).await.is_err() {
                            return;
                        }
                    }
                    Ok(resp) => {
                        let err = resp.error.unwrap_or_else(|| "unknown".into());
                        let delta = StateDelta::Replace {
                            path: topic.clone(),
                            value: json!({ "available": false, "reason": err }),
                        };
                        if tx.send(delta).await.is_err() {
                            return;
                        }
                    }
                    Err(_e) => {
                        client = None; // force reconnect next tick
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn adapter_open_unknown_topic_errors() {
        let a = MeshAdapter::new();
        let r = a.open("substrate/mesh/bogus", Value::Null).await;
        assert!(matches!(r, Err(AdapterError::UnknownTopic(_))));
    }

    #[test]
    fn declares_two_topics() {
        let paths: Vec<&str> = TOPICS.iter().map(|t| t.path).collect();
        assert_eq!(paths, vec!["substrate/mesh/status", "substrate/mesh/nodes"]);
    }
}
