//! The instance state machine and the bounded-restart policy (ADR-099
//! section 7). Pure: no I/O, no clock of its own (callers pass `now_ms`), so
//! every rule is table-testable.
//!
//! The same states serve both sides. A node's `workload-host` walks an
//! instance through `Loaded`, `Running`, `Unhealthy`, `Restarting`, `Failed`
//! and the teardown states; the controller adds `Lost` (its node is
//! `Dead`) and `Rescheduled` (a replacement was placed elsewhere).

use serde::{Deserialize, Serialize};

/// Where an instance is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    /// The controller asked for it.
    Requested,
    /// A node accepted the placement.
    Placed,
    /// The node is fetching the payload.
    Fetching,
    /// The payload verified.
    Verified,
    /// Loaded, not running.
    Loaded,
    /// Running and healthy.
    Running,
    /// Missed its health heartbeat; a restart is pending.
    Unhealthy,
    /// A restart is under way.
    Restarting,
    /// A stop was requested.
    Stopping,
    /// Stopped by the operator (not restarted).
    Stopped,
    /// A one-shot run ended cleanly.
    Finished,
    /// Restart budget spent, or it could not be started. Held for the
    /// operator; not retried by itself.
    Failed,
    /// Its node is `Dead`; the placer decides what happens next.
    Lost,
    /// A replacement was placed on another node. The old record only
    /// waits for its node to return so the orphan can be unloaded.
    Rescheduled,
    /// A revocation named it; it is being (or was) taken down.
    Revoked,
    /// Gone.
    Unloaded,
}

impl LifecycleState {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Placed => "placed",
            Self::Fetching => "fetching",
            Self::Verified => "verified",
            Self::Loaded => "loaded",
            Self::Running => "running",
            Self::Unhealthy => "unhealthy",
            Self::Restarting => "restarting",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Lost => "lost",
            Self::Rescheduled => "rescheduled",
            Self::Revoked => "revoked",
            Self::Unloaded => "unloaded",
        }
    }

    /// Nothing follows `Unloaded`.
    pub fn is_terminal(self) -> bool {
        self == Self::Unloaded
    }

    /// The instance is meant to be doing work.
    pub fn is_live(self) -> bool {
        matches!(self, Self::Running | Self::Unhealthy | Self::Restarting)
    }
}

/// Whether `from -> to` is a legal step.
pub fn can_transition(from: LifecycleState, to: LifecycleState) -> bool {
    use LifecycleState::*;
    if from == to || from.is_terminal() {
        return false;
    }
    // A revocation and a node loss can interrupt anything that exists; a
    // replaced record (`Rescheduled`) is only ever unloaded.
    if from != Rescheduled && matches!(to, Revoked | Lost) {
        return !(from == Revoked && to == Lost);
    }
    matches!(
        (from, to),
        (Requested, Placed | Failed)
            | (Placed, Fetching | Loaded | Running | Failed)
            | (Fetching, Verified | Failed)
            | (Verified, Loaded | Failed)
            | (Loaded, Running | Unloaded | Failed)
            | (Running, Unhealthy | Stopping | Stopped | Finished | Failed | Unloaded)
            | (Unhealthy, Running | Restarting | Stopping | Failed | Unloaded)
            | (Restarting, Running | Unhealthy | Failed | Stopping)
            | (Stopping, Stopped | Unloaded | Failed)
            | (Stopped, Running | Unloaded | Failed)
            | (Finished, Running | Unloaded)
            | (Failed, Running | Unloaded)
            | (Lost, Running | Rescheduled | Unloaded | Failed)
            | (Rescheduled, Unloaded)
            | (Revoked, Unloaded | Failed)
    )
}

/// A step that is not allowed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("illegal lifecycle step {from:?} -> {to:?}")]
pub struct IllegalTransition {
    /// Where it was.
    pub from: LifecycleState,
    /// Where it was asked to go.
    pub to: LifecycleState,
}

/// Bounded restarts: at most `max_restarts` inside `window_ms`, spaced by an
/// exponential backoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartPolicy {
    /// Restarts allowed inside the window (0 disables restarting).
    pub max_restarts: u32,
    /// The window restarts are counted in.
    pub window_ms: u64,
    /// Delay before the first restart; doubles with each one in the window.
    pub backoff_base_ms: u64,
    /// The backoff never exceeds this.
    pub backoff_max_ms: u64,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 3,
            window_ms: 600_000,
            backoff_base_ms: 1_000,
            backoff_max_ms: 30_000,
        }
    }
}

/// What to do about an unhealthy instance right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartDecision {
    /// Restart it now; this is attempt `attempt` (1-based) in the window.
    Restart {
        /// Attempt number inside the window.
        attempt: u32,
    },
    /// Not yet: the backoff ends at `until_ms`.
    Wait {
        /// Earliest time to restart.
        until_ms: u64,
    },
    /// The budget is spent: mark it failed.
    Exhausted,
}

/// One instance's life on one side of the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceLife {
    /// Current state.
    pub state: LifecycleState,
    /// When it entered the state (ms).
    pub since_ms: u64,
    /// Consecutive bad polls.
    pub misses: u32,
    /// Times of the restarts still inside the window (ms, ascending).
    pub restarts: Vec<u64>,
    /// When the last poll happened (ms); `0` before the first.
    pub last_poll_ms: u64,
}

impl InstanceLife {
    /// A new life in `state`.
    pub fn new(state: LifecycleState, now_ms: u64) -> Self {
        Self {
            state,
            since_ms: now_ms,
            misses: 0,
            restarts: Vec::new(),
            last_poll_ms: 0,
        }
    }

    /// Take a legal step (the miss count resets on entering `Running`).
    pub fn transition(
        &mut self,
        to: LifecycleState,
        now_ms: u64,
    ) -> Result<LifecycleState, IllegalTransition> {
        let from = self.state;
        if !can_transition(from, to) {
            return Err(IllegalTransition { from, to });
        }
        self.state = to;
        self.since_ms = now_ms;
        if to == LifecycleState::Running {
            self.misses = 0;
        }
        Ok(from)
    }

    /// Whether `policy` allows a restart at `now_ms`. Restarts older than
    /// the window no longer count. Does not record anything.
    pub fn restart_decision(&self, policy: &RestartPolicy, now_ms: u64) -> RestartDecision {
        let recent: Vec<u64> = self
            .restarts
            .iter()
            .copied()
            .filter(|t| now_ms.saturating_sub(*t) < policy.window_ms)
            .collect();
        let n = recent.len() as u32;
        if n >= policy.max_restarts {
            return RestartDecision::Exhausted;
        }
        // The wait before restart n+1 is the base for n = 1, then doubles.
        let shift = n.saturating_sub(1).min(20);
        let backoff = policy
            .backoff_base_ms
            .saturating_mul(1u64 << shift)
            .min(policy.backoff_max_ms);
        match recent.last() {
            Some(last) if now_ms < last.saturating_add(backoff) => RestartDecision::Wait {
                until_ms: last.saturating_add(backoff),
            },
            _ => RestartDecision::Restart { attempt: n + 1 },
        }
    }

    /// Record a restart at `now_ms` (and forget those outside the window).
    pub fn record_restart(&mut self, policy: &RestartPolicy, now_ms: u64) {
        self.restarts
            .retain(|t| now_ms.saturating_sub(*t) < policy.window_ms);
        self.restarts.push(now_ms);
    }
}

/// Operator-visible lifecycle settings of one placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecyclePolicy {
    /// May the placer move it when its node is lost? `false` raises an
    /// alert instead. A pin (`PlaceOrder::pin`) also holds it in place.
    pub migratable: bool,
    /// Bound on how many times one instance is rescheduled (a flapping
    /// mesh must not move a workload forever).
    pub max_reschedules: u32,
}

impl Default for LifecyclePolicy {
    fn default() -> Self {
        Self {
            migratable: true,
            max_reschedules: 5,
        }
    }
}

/// Capability namespaces that name something physically attached to or
/// local to one node: a sensor, a device, a data feed on a node's LAN.
/// `accel` is deliberately not here: any node with the same accelerator is
/// as good a home, so failover is the point. `trust`, `store`, `model`,
/// `cpu`, `os`, `runtime`, `mem`, `node`, `perf` are not hardware either.
const HARDWARE_NAMESPACES: &[&str] = &["sensor", "device", "feed"];

/// Whether `spec` needs something attached to (or local to) a particular
/// node: any requirement in [`HARDWARE_NAMESPACES`]. Such a workload's data
/// source is tied to its node, so moving it is not a failover; it is held
/// and alerted instead. An allow-list: a new namespace never pins by
/// accident.
pub fn needs_attached_hardware(spec: &clawft_types::placement::engine::WorkloadSpec) -> bool {
    use clawft_types::placement::IdSelector;
    let reqs = spec
        .requirements
        .common
        .iter()
        .chain(spec.requirements.variants.iter().flat_map(|v| v.requirements.iter()));
    reqs.into_iter().any(|r| {
        let id = match &r.selector {
            IdSelector::Exact(c) => c.as_str(),
            IdSelector::Prefix(p) => p.as_str(),
        };
        let ns = id.split('.').next().unwrap_or(id);
        HARDWARE_NAMESPACES.contains(&ns)
    })
}
