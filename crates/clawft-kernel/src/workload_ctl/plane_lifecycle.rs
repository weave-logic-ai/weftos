//! Controller-side lifecycle (ADR-099 section 7): mirror each instance's
//! state from its node, notice a `Dead` node, mark its instances `Lost` and
//! reschedule them through the same gate and placement engine as any
//! placement, and settle with a node that comes back.
//!
//! [`PlacementControlPlane::lifecycle_tick`] is one pass:
//!
//! 1. re-describe every target (reachability) and note how long each has
//!    been down;
//! 2. a node is `Dead` when membership says so, or it has been unreachable
//!    for `PlaneConfig::dead_after`;
//! 3. each instance on a `Dead` node becomes `Lost`. It is rescheduled
//!    unless the order pinned it, the operator or its kind said it must not
//!    move, it has no stored order, or it moved too often already; those
//!    raise an alert (chained) and stay `Lost`;
//! 4. a replacement goes through [`PlacementControlPlane::place`]: the gate
//!    asks `workload.place`, the engine sees only nodes that are `Alive`,
//!    verified members and not demoted below what governance allows, and
//!    the dead node is added to `avoid`. If nothing is placeable the
//!    instance stays `Lost` and the next tick tries again;
//! 5. the old record becomes `Rescheduled` and is kept. When its node
//!    returns, the orphan is unloaded there (signed, gated, chained), so the
//!    workload never runs twice;
//! 6. an instance that was `Lost` but is still on its node when it returns
//!    is adopted back; one the node no longer has is `Failed`.
//!
//! Every step is chained: `workload.lifecycle` for state changes and alerts,
//! `workload.migrate` for a reschedule.

use clawft_types::placement::engine::Liveness;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::lifecycle::{InstanceLife, LifecyclePolicy, LifecycleState};
use super::msg::{RefusalCode, method};
use super::plane::{CallFailure, PlacementControlPlane, PlacementRecord, PlaneError, now_ms};
use super::plane_place::PlaceOrder;
use crate::chain;

/// What the controller keeps per instance beyond its placement record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerLife {
    /// Current state and history.
    pub life: InstanceLife,
    /// May it move, and how often.
    pub policy: LifecyclePolicy,
    /// The order it was placed with (what a reschedule re-runs).
    #[serde(default)]
    pub order: Option<PlaceOrder>,
    /// Times it was moved to another node.
    #[serde(default)]
    pub reschedules: u32,
    /// Last reason a reschedule could not happen (chained once per change).
    #[serde(default)]
    pub last_error: Option<String>,
    /// It was meant to be running when its node was lost. A stopped,
    /// failed or finished instance is never started somewhere else.
    #[serde(default = "yes")]
    pub was_running: bool,
}

fn yes() -> bool {
    true
}

/// Something one [`PlacementControlPlane::lifecycle_tick`] did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LifecycleEvent {
    /// Instance id (on the node it was last placed on).
    pub instance_id: String,
    /// That node.
    pub node_id: String,
    /// `lost`, `rescheduled`, `alert`, `reschedule_failed`, `orphan_unloaded`,
    /// `recovered`, `failed` or `gone`.
    pub action: String,
    /// Human-readable detail.
    pub detail: String,
}

fn event(rec: &PlacementRecord, action: &str, detail: impl Into<String>) -> LifecycleEvent {
    LifecycleEvent {
        instance_id: rec.instance_id.clone(),
        node_id: rec.node_id.clone(),
        action: action.into(),
        detail: detail.into(),
    }
}

impl PlacementControlPlane {
    /// Remember how a fresh placement was ordered.
    pub(super) fn register_life(&self, rec: &PlacementRecord, order: &PlaceOrder, p: LifecyclePolicy) {
        let state = if order.start {
            LifecycleState::Running
        } else {
            LifecycleState::Loaded
        };
        if let Ok(mut l) = self.lives.lock() {
            l.insert(
                rec.instance_id.clone(),
                ControllerLife {
                    life: InstanceLife::new(state, now_ms()),
                    policy: p,
                    order: Some(order.clone()),
                    reschedules: 0,
                    last_error: None,
                    was_running: true,
                },
            );
        }
        self.persist();
    }

    pub(super) fn drop_life(&self, instance_id: &str) {
        if let Ok(mut l) = self.lives.lock() {
            l.remove(instance_id);
        }
    }

    /// Lifecycle state the controller holds for `instance_id`.
    pub fn lifecycle_of(&self, instance_id: &str) -> Option<LifecycleState> {
        self.lives
            .lock()
            .ok()
            .and_then(|l| l.get(instance_id).map(|c| c.life.state))
    }

    fn set_state(&self, rec: &PlacementRecord, to: LifecycleState, reason: &str, force: bool) {
        let kind = chain::EVENT_KIND_WORKLOAD_LIFECYCLE;
        let from = {
            let Ok(mut l) = self.lives.lock() else { return };
            let c = l.entry(rec.instance_id.clone()).or_insert_with(|| ControllerLife {
                life: InstanceLife::new(LifecycleState::Running, now_ms()),
                policy: LifecyclePolicy::default(),
                order: None,
                reschedules: 0,
                last_error: None,
                was_running: true,
            });
            if to == LifecycleState::Lost {
                c.was_running = c.life.state.is_live();
            }
            match c.life.transition(to, now_ms()) {
                Ok(f) => f,
                // What the node reports wins over what the table allows.
                Err(_) if force => {
                    let f = c.life.state;
                    c.life.state = to;
                    c.life.since_ms = now_ms();
                    f
                }
                Err(_) => return,
            }
        };
        self.chain_event(
            kind,
            json!({ "phase": "lifecycle", "instance_id": rec.instance_id, "node": rec.node_id,
                    "workload": rec.workload, "from": from, "to": to, "reason": reason }),
        );
    }

    /// Whether `node` is `Dead` now: membership says so, or it has been
    /// unreachable for `dead_after`.
    fn node_dead(&self, node: &str, now: u64) -> bool {
        if let Some(p) = self.membership.as_ref().and_then(|m| m.get_peer(node))
            && super::facts::liveness_of(&p.state) == Liveness::Dead
        {
            return true;
        }
        let since = self.down_since.lock().ok().and_then(|d| d.get(node).copied());
        since.is_some_and(|s| now.saturating_sub(s) >= self.cfg.dead_after.as_millis() as u64)
    }

    fn note_reachability(&self, now: u64) {
        let targets = self.targets();
        let Ok(mut d) = self.down_since.lock() else { return };
        for t in targets {
            if t.reachable {
                d.remove(&t.node_id);
            } else {
                d.entry(t.node_id).or_insert(now);
            }
        }
    }

    /// Why this instance must not be moved, if so.
    fn immovable(&self, rec: &PlacementRecord, c: &ControllerLife) -> Option<String> {
        let kind_ok = self
            .kinds
            .get(&rec.kind)
            .is_none_or(|k| k.migratable());
        match &c.order {
            _ if !c.was_running => Some("it was not running when the node was lost".into()),
            None => Some("no stored placement order (adopted instance)".into()),
            Some(o) if o.pin.is_some() => Some(format!("pinned to {}", o.pin.as_deref().unwrap_or(""))),
            Some(_) if !c.policy.migratable => Some("operator marked it non-migratable".into()),
            Some(_) if !kind_ok => Some(format!("kind {} is non-migratable", rec.kind)),
            Some(o) if !o.package_dir.is_dir() => Some("its package directory is gone".into()),
            Some(_) if c.reschedules >= c.policy.max_reschedules => Some(format!(
                "already rescheduled {} times",
                c.reschedules
            )),
            Some(_) => None,
        }
    }

    /// One pass of the controller lifecycle; see the module docs.
    pub async fn lifecycle_tick(&self) -> Vec<LifecycleEvent> {
        self.refresh().await;
        let now = now_ms();
        self.note_reachability(now);
        let mut out = Vec::new();
        self.mirror_states(&mut out).await;
        let known: Vec<String> = self.targets().into_iter().map(|t| t.node_id).collect();
        for rec in self.placements() {
            // Seed placements and instances on nodes we have no target for
            // are not this pass's business (a Seed is hardware-bound).
            if !known.contains(&rec.node_id) {
                continue;
            }
            let Some(state) = self.lifecycle_of(&rec.instance_id) else {
                self.set_state(&rec, LifecycleState::Running, "adopted by the lifecycle", false);
                continue;
            };
            let dead = self.node_dead(&rec.node_id, now);
            match (dead, state) {
                (false, LifecycleState::Rescheduled) => self.unload_orphan(&rec, &mut out).await,
                (_, LifecycleState::Rescheduled) => {}
                (true, LifecycleState::Lost) => self.try_reschedule(&rec, &mut out).await,
                (true, s) if !s.is_terminal() => {
                    self.set_state(&rec, LifecycleState::Lost, "node is dead", false);
                    out.push(event(&rec, "lost", format!("node {} is dead", rec.node_id)));
                    self.try_reschedule(&rec, &mut out).await;
                }
                (false, LifecycleState::Lost) => self.rejoined(&rec, &mut out).await,
                _ => {}
            }
        }
        self.persist();
        out
    }

    /// Mirror each reachable node's instance states and drop records the
    /// node no longer has (taken down by a revocation, or lost with a
    /// restart of the node).
    async fn mirror_states(&self, out: &mut Vec<LifecycleEvent>) {
        let recs = self.placements();
        for t in self.targets().into_iter().filter(|t| t.reachable) {
            let mine: Vec<&PlacementRecord> = recs.iter().filter(|r| r.node_id == t.node_id).collect();
            if mine.is_empty() {
                continue;
            }
            let Ok(Value::Array(rows)) = self.call(&t.node_id, method::STATUS, None, json!({})).await else {
                continue;
            };
            for rec in mine {
                let state = self.lifecycle_of(&rec.instance_id);
                if matches!(state, Some(LifecycleState::Rescheduled | LifecycleState::Lost)) {
                    continue; // handled by the rejoin path
                }
                let row = rows
                    .iter()
                    .find(|r| r["instance_id"].as_str() == Some(rec.instance_id.as_str()));
                match row {
                    None => {
                        self.set_state(rec, LifecycleState::Unloaded, "no longer on its node", false);
                        self.forget_instances(std::slice::from_ref(&rec.instance_id));
                        out.push(event(rec, "gone", "the node no longer holds it"));
                    }
                    Some(r) => {
                        let to = serde_json::from_value::<LifecycleState>(r["lifecycle"].clone()).ok();
                        if let Some(to) = to
                            && Some(to) != state
                        {
                            self.set_state(rec, to, "reported by its node", true);
                        }
                    }
                }
            }
        }
    }

    async fn try_reschedule(&self, rec: &PlacementRecord, out: &mut Vec<LifecycleEvent>) {
        let Some(c) = self.lives.lock().ok().and_then(|l| l.get(&rec.instance_id).cloned()) else {
            return;
        };
        if let Some(why) = self.immovable(rec, &c) {
            if c.last_error.as_deref() != Some(why.as_str()) {
                self.chain_event(
                    chain::EVENT_KIND_WORKLOAD_LIFECYCLE,
                    json!({ "phase": "alert", "instance_id": rec.instance_id, "node": rec.node_id,
                            "workload": rec.workload,
                            "reason": format!("node lost, not rescheduled: {why}") }),
                );
                self.note_error(&rec.instance_id, Some(why.clone()));
                out.push(event(rec, "alert", why));
            }
            return;
        }
        let mut order = c.order.clone().expect("immovable() requires an order");
        order.avoid.push(rec.node_id.clone());
        order.dry_run = false;
        // Same gate, same engine: nothing here bypasses placement.
        let report = match self.place_unregistered(&order).await {
            Ok(r) => r,
            Err(e) => return self.reschedule_failed(rec, e.to_string(), out),
        };
        let Some(new) = report.placed else {
            let why = report
                .attempts
                .last()
                .map(|a| format!("{}: {}", a.outcome, a.reason.clone().unwrap_or_default()))
                .unwrap_or_else(|| "no node accepted it".into());
            return self.reschedule_failed(rec, why, out);
        };
        // The replacement carries the history; the old record waits for its node.
        if let Ok(mut l) = self.lives.lock() {
            l.insert(
                new.instance_id.clone(),
                ControllerLife {
                    life: InstanceLife::new(
                        if order.start { LifecycleState::Running } else { LifecycleState::Loaded },
                        now_ms(),
                    ),
                    policy: c.policy,
                    order: c.order.clone(),
                    reschedules: c.reschedules + 1,
                    last_error: None,
                    was_running: true,
                },
            );
        }
        self.set_state(rec, LifecycleState::Rescheduled, &format!("replaced on {}", new.node_id), false);
        self.chain_event(
            chain::EVENT_KIND_WORKLOAD_MIGRATE,
            json!({ "phase": "rescheduled", "reason": "node_dead", "workload": rec.workload,
                    "from_node": rec.node_id, "from_instance": rec.instance_id,
                    "to_node": new.node_id, "to_instance": new.instance_id,
                    "decision_id": report.decision_id }),
        );
        out.push(event(
            rec,
            "rescheduled",
            format!("now {} on {}", new.instance_id, new.node_id),
        ));
    }

    fn note_error(&self, instance_id: &str, e: Option<String>) {
        if let Ok(mut l) = self.lives.lock()
            && let Some(c) = l.get_mut(instance_id)
        {
            c.last_error = e;
        }
    }

    fn reschedule_failed(&self, rec: &PlacementRecord, why: String, out: &mut Vec<LifecycleEvent>) {
        let first = self
            .lives
            .lock()
            .ok()
            .and_then(|l| l.get(&rec.instance_id).map(|c| c.last_error.clone()))
            .flatten();
        if first.as_deref() == Some(why.as_str()) {
            return; // same reason as last tick: already chained
        }
        self.chain_event(
            chain::EVENT_KIND_WORKLOAD_REFUSE,
            json!({ "phase": "reschedule", "instance_id": rec.instance_id, "node": rec.node_id,
                    "workload": rec.workload, "reason": why, "next": "retry on the next tick" }),
        );
        self.note_error(&rec.instance_id, Some(why.clone()));
        out.push(event(rec, "reschedule_failed", why));
    }

    /// An old record whose node answers again: unload what is left there.
    async fn unload_orphan(&self, rec: &PlacementRecord, out: &mut Vec<LifecycleEvent>) {
        match self.instance(method::UNLOAD, &rec.instance_id).await {
            Ok(_) => out.push(event(rec, "orphan_unloaded", "node returned; the replaced copy was unloaded")),
            Err(PlaneError::Call(CallFailure::Refused(r))) if r.code == RefusalCode::UnknownInstance => {
                // The node restarted and no longer holds it.
                self.drop_life(&rec.instance_id);
                self.forget_instances(std::slice::from_ref(&rec.instance_id));
                out.push(event(rec, "orphan_unloaded", "node returned without the replaced copy"));
            }
            Err(_) => {} // still unreachable or refused: retried next tick
        }
    }

    /// A `Lost` instance whose node is back: adopt it if it is still there.
    async fn rejoined(&self, rec: &PlacementRecord, out: &mut Vec<LifecycleEvent>) {
        let Ok(Value::Array(rows)) = self.call(&rec.node_id, method::STATUS, None, json!({})).await else {
            return;
        };
        let row = rows
            .iter()
            .find(|r| r["instance_id"].as_str() == Some(rec.instance_id.as_str()));
        match row {
            Some(_) => {
                self.set_state(rec, LifecycleState::Running, "node rejoined with the instance", false);
                self.note_error(&rec.instance_id, None);
                out.push(event(rec, "recovered", "node rejoined; the instance is still there"));
            }
            None => {
                self.set_state(rec, LifecycleState::Failed, "node rejoined without the instance", false);
                out.push(event(rec, "failed", "node rejoined without the instance"));
            }
        }
    }

    /// Run [`Self::lifecycle_tick`] every `period` for as long as the runtime lives.
    pub fn spawn_lifecycle(
        self: &std::sync::Arc<Self>,
        period: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        let me = std::sync::Arc::clone(self);
        tokio::spawn(async move {
            let mut t = tokio::time::interval(period);
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            t.tick().await;
            loop {
                t.tick().await;
                let acted = me.lifecycle_tick().await;
                if !acted.is_empty() {
                    tracing::warn!(n = acted.len(), "placement lifecycle");
                }
            }
        })
    }
}
