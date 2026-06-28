//! `chain` reference adapter — promotes the tray's ExoChain chip from
//! "service named `chain`/`exochain` is registered" to "the daemon's
//! `chain.status` returns fresh data."
//!
//! Polls `chain.status`. On daemons built without the `exochain`
//! feature the RPC returns an error and the adapter emits
//! `{available: false}` so the tray can render amber/grey rather than
//! pretend the chain is up.
//!
//! ## Topics
//!
//! | Topic | Shape | Refresh |
//! |-------|-------|---------|
//! | `substrate/chain/status` | success → `{chain_id, sequence, event_count, checkpoint_count, last_hash}`; failure → `{available: false, reason}` | 3s |
//! | `substrate/chain/tail`   | array of `ChainEventInfo { sequence, chain_id, timestamp, source, kind, hash, detail }` (newest last); empty array on chain-disabled / unreachable | 1.5s |
//!
//! Public — chain head hash + sequence is public metadata, not user
//! content. Writes stay gated through governance (ADR-015); these
//! topics are read-only.

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

const CHAN: usize = 1;

/// Declared topics. `tail` polls faster than `status` so the witness-
/// chain panel sees fresh `agent.chat.turn` events soon after they
/// land, without making the bare summary chips refresh more often
/// than they need to.
pub const TOPICS: &[TopicDecl] = &[
    TopicDecl {
        path: "substrate/chain/status",
        shape: "ontology://chain-status",
        refresh_hint: RefreshHint::Periodic { ms: 3000 },
        sensitivity: Sensitivity::Public,
        buffer_policy: BufferPolicy::Refuse,
        max_len: None,
    },
    TopicDecl {
        path: "substrate/chain/tail",
        shape: "ontology://chain-tail",
        refresh_hint: RefreshHint::Periodic { ms: 1500 },
        sensitivity: Sensitivity::Public,
        buffer_policy: BufferPolicy::Refuse,
        max_len: None,
    },
];

/// Default number of trailing chain events the panel polls. Matches
/// the daemon's `chain.tail` default and gives the witness-chain
/// stream-view ~20 rows of context per refresh — enough to see the
/// last few `agent.chat.turn` mirrors plus the surrounding routing /
/// health-tick noise.
const TAIL_COUNT: u32 = 20;

/// Permissions — none.
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

/// Chain adapter — calls the daemon's `chain.status` RPC verb.
pub struct ChainAdapter {
    reg: Mutex<Registry>,
}

impl Default for ChainAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ChainAdapter {
    /// Build a new adapter. Connects to the daemon on first poll.
    pub fn new() -> Self {
        Self {
            reg: Mutex::new(Registry::new()),
        }
    }
}

#[async_trait]
impl OntologyAdapter for ChainAdapter {
    fn id(&self) -> &'static str {
        "chain"
    }

    fn topics(&self) -> &'static [TopicDecl] {
        TOPICS
    }

    fn permissions(&self) -> &'static [PermissionReq] {
        PERMISSIONS
    }

    async fn open(&self, topic: &str, _args: Value) -> Result<Subscription, AdapterError> {
        let id = {
            let mut reg = self.reg.lock();
            reg.allocate()
        };
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (tx, rx) = mpsc::channel::<StateDelta>(CHAN);
        self.reg.lock().live.insert(id, cancel_tx);

        match topic {
            "substrate/chain/status" => {
                tokio::spawn(async move {
                    poll_chain_status(tx, cancel_rx).await;
                });
            }
            "substrate/chain/tail" => {
                tokio::spawn(async move {
                    poll_chain_tail(tx, cancel_rx).await;
                });
            }
            _ => {
                self.reg.lock().live.remove(&id);
                return Err(AdapterError::UnknownTopic(topic.into()));
            }
        }
        Ok(Subscription { id, rx })
    }

    async fn close(&self, sub_id: SubId) -> Result<(), AdapterError> {
        let _ = self.reg.lock().live.remove(&sub_id);
        Ok(())
    }
}

async fn poll_chain_status(tx: mpsc::Sender<StateDelta>, mut cancel_rx: oneshot::Receiver<()>) {
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
                    let delta = StateDelta::Replace {
                        path: "substrate/chain/status".into(),
                        value: json!({ "available": false, "reason": "daemon-unreachable" }),
                    };
                    if tx.send(delta).await.is_err() {
                        return;
                    }
                    continue;
                };
                match c.simple_call("chain.status").await {
                    Ok(resp) if resp.ok => {
                        // Enrich the success response with an
                        // `available: true` flag so the tray binding
                        // can match a single predicate instead of
                        // distinguishing two shapes.
                        let mut value = resp.result.unwrap_or(Value::Null);
                        if let Value::Object(ref mut obj) = value {
                            obj.insert("available".into(), json!(true));
                        }
                        let delta = StateDelta::Replace {
                            path: "substrate/chain/status".into(),
                            value,
                        };
                        if tx.send(delta).await.is_err() {
                            return;
                        }
                    }
                    Ok(resp) => {
                        // chain.status returns `error` when the
                        // daemon was built without the `exochain`
                        // feature. Surface it as `available: false`.
                        let err = resp.error.unwrap_or_else(|| "unknown".into());
                        let delta = StateDelta::Replace {
                            path: "substrate/chain/status".into(),
                            value: json!({
                                "available": false,
                                "reason": err,
                            }),
                        };
                        if tx.send(delta).await.is_err() {
                            return;
                        }
                    }
                    Err(_e) => {
                        client = None;
                    }
                }
            }
        }
    }
}

/// Poll the daemon's `chain.tail` and republish the trailing event
/// list under `substrate/chain/tail`. On daemons without `exochain`
/// the call returns an error and we publish an empty array — same
/// shape, just no rows — so the witness-chain panel renders its
/// empty-state hint instead of "no data."
async fn poll_chain_tail(tx: mpsc::Sender<StateDelta>, mut cancel_rx: oneshot::Receiver<()>) {
    let mut client: Option<DaemonClient> = None;
    let mut ticker = tokio::time::interval(Duration::from_millis(1500));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = &mut cancel_rx => return,
            _ = ticker.tick() => {
                if client.is_none() {
                    client = DaemonClient::connect().await;
                }
                let Some(c) = client.as_mut() else {
                    let delta = StateDelta::Replace {
                        path: "substrate/chain/tail".into(),
                        value: json!([]),
                    };
                    if tx.send(delta).await.is_err() {
                        return;
                    }
                    continue;
                };
                let req = clawft_rpc::Request::with_params(
                    "chain.tail",
                    json!({ "count": TAIL_COUNT }),
                );
                match c.call(req).await {
                    Ok(resp) if resp.ok => {
                        let value = resp.result.unwrap_or(json!([]));
                        let delta = StateDelta::Replace {
                            path: "substrate/chain/tail".into(),
                            value,
                        };
                        if tx.send(delta).await.is_err() {
                            return;
                        }
                    }
                    Ok(_) => {
                        // Daemon-level error (e.g. exochain feature
                        // off) — empty list is the honest signal here.
                        let delta = StateDelta::Replace {
                            path: "substrate/chain/tail".into(),
                            value: json!([]),
                        };
                        if tx.send(delta).await.is_err() {
                            return;
                        }
                    }
                    Err(_) => {
                        client = None;
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
        let a = ChainAdapter::new();
        let r = a.open("substrate/chain/bogus", Value::Null).await;
        assert!(matches!(r, Err(AdapterError::UnknownTopic(_))));
    }

    #[test]
    fn declares_status_and_tail_topics() {
        let paths: Vec<&str> = TOPICS.iter().map(|t| t.path).collect();
        assert_eq!(
            paths,
            vec!["substrate/chain/status", "substrate/chain/tail"]
        );
    }
}
