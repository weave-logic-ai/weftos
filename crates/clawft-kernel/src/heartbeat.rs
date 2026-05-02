//! Heartbeat scheduler for agent lifecycle management.
//!
//! The [`HeartbeatScheduler`] manages periodic wake-up cycles for agents.
//! Each registered agent goes through a [`HeartbeatPhase`] cycle:
//! Wake -> CheckQueue -> Execute -> Sleep -> Report, then back to Wake.
//!
//! This is a separate abstraction from the cron system: cron handles
//! arbitrary scheduled jobs, while heartbeats manage the regular
//! tick-based lifecycle of agents.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Configuration for a single agent's heartbeat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    /// The agent this heartbeat belongs to.
    pub agent_id: String,
    /// How often the agent should wake up.
    pub interval: Duration,
    /// Whether the heartbeat is currently active.
    pub enabled: bool,
}

impl HeartbeatConfig {
    /// Create a new enabled heartbeat config.
    pub fn new(agent_id: impl Into<String>, interval: Duration) -> Self {
        Self {
            agent_id: agent_id.into(),
            interval,
            enabled: true,
        }
    }
}

/// Phase of an agent's heartbeat cycle.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum HeartbeatPhase {
    /// Agent is waking up and initialising for the tick.
    Wake,
    /// Agent is checking its work queue.
    CheckQueue,
    /// Agent is executing queued work.
    Execute,
    /// Agent is idle, waiting for the next tick.
    #[default]
    Sleep,
    /// Agent is reporting results from this cycle.
    Report,
}

impl HeartbeatPhase {
    /// Advance to the next phase in the cycle.
    pub fn next(self) -> Self {
        match self {
            Self::Wake => Self::CheckQueue,
            Self::CheckQueue => Self::Execute,
            Self::Execute => Self::Sleep,
            Self::Sleep => Self::Report,
            Self::Report => Self::Wake,
        }
    }
}


impl std::fmt::Display for HeartbeatPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wake => write!(f, "wake"),
            Self::CheckQueue => write!(f, "check_queue"),
            Self::Execute => write!(f, "execute"),
            Self::Sleep => write!(f, "sleep"),
            Self::Report => write!(f, "report"),
        }
    }
}

/// Internal state for a single agent's heartbeat.
#[derive(Debug, Clone)]
struct AgentHeartbeat {
    config: HeartbeatConfig,
    phase: HeartbeatPhase,
    /// Accumulated time since last wake. When >= interval, the agent wakes.
    elapsed: Duration,
    /// Number of completed cycles.
    tick_count: u64,
}

/// Result of a single tick for one agent.
#[derive(Debug, Clone)]
pub struct HeartbeatTickResult {
    /// The agent that needs to wake.
    pub agent_id: String,
    /// The phase the agent is entering.
    pub phase: HeartbeatPhase,
    /// How many complete cycles this agent has done.
    pub tick_count: u64,
}

/// Manages heartbeat schedules for multiple agents.
///
/// Call [`tick`] with the elapsed wall-clock delta. Agents whose
/// accumulated elapsed time exceeds their configured interval will
/// be returned as needing a wake-up.
#[derive(Debug)]
pub struct HeartbeatScheduler {
    agents: HashMap<String, AgentHeartbeat>,
}

impl HeartbeatScheduler {
    /// Create an empty scheduler.
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    /// Register an agent heartbeat. Replaces any existing entry.
    pub fn register(&mut self, config: HeartbeatConfig) {
        let id = config.agent_id.clone();
        self.agents.insert(
            id,
            AgentHeartbeat {
                config,
                phase: HeartbeatPhase::Sleep,
                elapsed: Duration::ZERO,
                tick_count: 0,
            },
        );
    }

    /// Remove an agent from the scheduler. Returns `true` if it existed.
    pub fn unregister(&mut self, agent_id: &str) -> bool {
        self.agents.remove(agent_id).is_some()
    }

    /// Advance all heartbeats by `delta` time.
    ///
    /// Returns a list of agents that need to wake up (their interval
    /// has elapsed). Agents that are disabled are skipped.
    pub fn tick(&mut self, delta: Duration) -> Vec<HeartbeatTickResult> {
        let mut wakeups = Vec::new();

        for hb in self.agents.values_mut() {
            if !hb.config.enabled {
                continue;
            }

            hb.elapsed += delta;

            if hb.elapsed >= hb.config.interval {
                hb.elapsed -= hb.config.interval;
                hb.phase = HeartbeatPhase::Wake;
                hb.tick_count += 1;

                wakeups.push(HeartbeatTickResult {
                    agent_id: hb.config.agent_id.clone(),
                    phase: hb.phase,
                    tick_count: hb.tick_count,
                });
            }
        }

        wakeups
    }

    /// Return the current phase of a given agent.
    pub fn phase(&self, agent_id: &str) -> Option<HeartbeatPhase> {
        self.agents.get(agent_id).map(|hb| hb.phase)
    }

    /// Advance a specific agent to its next phase.
    pub fn advance_phase(&mut self, agent_id: &str) -> Option<HeartbeatPhase> {
        if let Some(hb) = self.agents.get_mut(agent_id) {
            hb.phase = hb.phase.next();
            Some(hb.phase)
        } else {
            None
        }
    }

    /// Number of registered agents.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Whether the scheduler has no registered agents.
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Return all registered agent IDs.
    pub fn agent_ids(&self) -> Vec<&str> {
        self.agents.keys().map(|s| s.as_str()).collect()
    }

    /// Get the tick count for a given agent.
    pub fn tick_count(&self, agent_id: &str) -> Option<u64> {
        self.agents.get(agent_id).map(|hb| hb.tick_count)
    }
}

impl Default for HeartbeatScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_config_new() {
        let cfg = HeartbeatConfig::new("agent-1", Duration::from_secs(10));
        assert_eq!(cfg.agent_id, "agent-1");
        assert_eq!(cfg.interval, Duration::from_secs(10));
        assert!(cfg.enabled);
    }

    #[test]
    fn heartbeat_phase_cycle() {
        let mut phase = HeartbeatPhase::Wake;
        phase = phase.next(); // CheckQueue
        assert_eq!(phase, HeartbeatPhase::CheckQueue);
        phase = phase.next(); // Execute
        assert_eq!(phase, HeartbeatPhase::Execute);
        phase = phase.next(); // Sleep
        assert_eq!(phase, HeartbeatPhase::Sleep);
        phase = phase.next(); // Report
        assert_eq!(phase, HeartbeatPhase::Report);
        phase = phase.next(); // Wake (cycle)
        assert_eq!(phase, HeartbeatPhase::Wake);
    }

    #[test]
    fn heartbeat_phase_default_is_sleep() {
        assert_eq!(HeartbeatPhase::default(), HeartbeatPhase::Sleep);
    }

    #[test]
    fn heartbeat_phase_display() {
        assert_eq!(HeartbeatPhase::Wake.to_string(), "wake");
        assert_eq!(HeartbeatPhase::CheckQueue.to_string(), "check_queue");
        assert_eq!(HeartbeatPhase::Execute.to_string(), "execute");
        assert_eq!(HeartbeatPhase::Sleep.to_string(), "sleep");
        assert_eq!(HeartbeatPhase::Report.to_string(), "report");
    }

    #[test]
    fn heartbeat_phase_serde() {
        let phases = [
            (HeartbeatPhase::Wake, "\"wake\""),
            (HeartbeatPhase::CheckQueue, "\"check_queue\""),
            (HeartbeatPhase::Execute, "\"execute\""),
            (HeartbeatPhase::Sleep, "\"sleep\""),
            (HeartbeatPhase::Report, "\"report\""),
        ];
        for (phase, expected) in &phases {
            let json = serde_json::to_string(phase).unwrap();
            assert_eq!(&json, expected);
            let restored: HeartbeatPhase = serde_json::from_str(&json).unwrap();
            assert_eq!(&restored, phase);
        }
    }

    #[test]
    fn scheduler_empty() {
        let sched = HeartbeatScheduler::new();
        assert!(sched.is_empty());
        assert_eq!(sched.len(), 0);
    }

    #[test]
    fn scheduler_register_unregister() {
        let mut sched = HeartbeatScheduler::new();
        sched.register(HeartbeatConfig::new("a1", Duration::from_secs(5)));
        assert_eq!(sched.len(), 1);
        assert!(!sched.is_empty());

        assert!(sched.unregister("a1"));
        assert!(sched.is_empty());
        assert!(!sched.unregister("a1")); // already removed
    }

    #[test]
    fn scheduler_tick_no_wakeup_before_interval() {
        let mut sched = HeartbeatScheduler::new();
        sched.register(HeartbeatConfig::new("a1", Duration::from_secs(10)));

        let wakeups = sched.tick(Duration::from_secs(5));
        assert!(wakeups.is_empty());
    }

    #[test]
    fn scheduler_tick_wakeup_at_interval() {
        let mut sched = HeartbeatScheduler::new();
        sched.register(HeartbeatConfig::new("a1", Duration::from_secs(10)));

        let wakeups = sched.tick(Duration::from_secs(10));
        assert_eq!(wakeups.len(), 1);
        assert_eq!(wakeups[0].agent_id, "a1");
        assert_eq!(wakeups[0].phase, HeartbeatPhase::Wake);
        assert_eq!(wakeups[0].tick_count, 1);
    }

    #[test]
    fn scheduler_tick_accumulates() {
        let mut sched = HeartbeatScheduler::new();
        sched.register(HeartbeatConfig::new("a1", Duration::from_secs(10)));

        // 4 + 4 = 8, not enough
        let w1 = sched.tick(Duration::from_secs(4));
        assert!(w1.is_empty());
        let w2 = sched.tick(Duration::from_secs(4));
        assert!(w2.is_empty());

        // 8 + 3 = 11 >= 10, wake up
        let w3 = sched.tick(Duration::from_secs(3));
        assert_eq!(w3.len(), 1);
    }

    #[test]
    fn scheduler_tick_disabled_agent_skipped() {
        let mut sched = HeartbeatScheduler::new();
        let mut cfg = HeartbeatConfig::new("a1", Duration::from_secs(1));
        cfg.enabled = false;
        sched.register(cfg);

        let wakeups = sched.tick(Duration::from_secs(5));
        assert!(wakeups.is_empty());
    }

    #[test]
    fn scheduler_tick_multiple_agents() {
        let mut sched = HeartbeatScheduler::new();
        sched.register(HeartbeatConfig::new("fast", Duration::from_secs(5)));
        sched.register(HeartbeatConfig::new("slow", Duration::from_secs(15)));

        // At t=5: fast wakes, slow does not
        let w1 = sched.tick(Duration::from_secs(5));
        assert_eq!(w1.len(), 1);
        assert_eq!(w1[0].agent_id, "fast");

        // At t=10: fast wakes again, slow still not
        let w2 = sched.tick(Duration::from_secs(5));
        assert_eq!(w2.len(), 1);
        assert_eq!(w2[0].agent_id, "fast");

        // At t=15: both wake
        let w3 = sched.tick(Duration::from_secs(5));
        assert_eq!(w3.len(), 2);
    }

    #[test]
    fn scheduler_phase_tracking() {
        let mut sched = HeartbeatScheduler::new();
        sched.register(HeartbeatConfig::new("a1", Duration::from_secs(1)));

        // Initially Sleep
        assert_eq!(sched.phase("a1"), Some(HeartbeatPhase::Sleep));

        // After tick, should be Wake
        sched.tick(Duration::from_secs(1));
        assert_eq!(sched.phase("a1"), Some(HeartbeatPhase::Wake));

        // Advance through phases
        assert_eq!(sched.advance_phase("a1"), Some(HeartbeatPhase::CheckQueue));
        assert_eq!(sched.advance_phase("a1"), Some(HeartbeatPhase::Execute));
        assert_eq!(sched.advance_phase("a1"), Some(HeartbeatPhase::Sleep));
        assert_eq!(sched.advance_phase("a1"), Some(HeartbeatPhase::Report));
        assert_eq!(sched.advance_phase("a1"), Some(HeartbeatPhase::Wake));
    }

    #[test]
    fn scheduler_phase_unknown_agent() {
        let sched = HeartbeatScheduler::new();
        assert_eq!(sched.phase("unknown"), None);
    }

    #[test]
    fn scheduler_advance_phase_unknown_agent() {
        let mut sched = HeartbeatScheduler::new();
        assert_eq!(sched.advance_phase("unknown"), None);
    }

    #[test]
    fn scheduler_agent_ids() {
        let mut sched = HeartbeatScheduler::new();
        sched.register(HeartbeatConfig::new("a1", Duration::from_secs(1)));
        sched.register(HeartbeatConfig::new("a2", Duration::from_secs(2)));
        let mut ids = sched.agent_ids();
        ids.sort();
        assert_eq!(ids, vec!["a1", "a2"]);
    }

    #[test]
    fn scheduler_tick_count() {
        let mut sched = HeartbeatScheduler::new();
        sched.register(HeartbeatConfig::new("a1", Duration::from_secs(1)));

        assert_eq!(sched.tick_count("a1"), Some(0));
        sched.tick(Duration::from_secs(1));
        assert_eq!(sched.tick_count("a1"), Some(1));
        sched.tick(Duration::from_secs(1));
        assert_eq!(sched.tick_count("a1"), Some(2));
    }

    #[test]
    fn scheduler_register_replaces() {
        let mut sched = HeartbeatScheduler::new();
        sched.register(HeartbeatConfig::new("a1", Duration::from_secs(5)));
        sched.tick(Duration::from_secs(5));
        assert_eq!(sched.tick_count("a1"), Some(1));

        // Re-register resets state
        sched.register(HeartbeatConfig::new("a1", Duration::from_secs(10)));
        assert_eq!(sched.tick_count("a1"), Some(0));
        assert_eq!(sched.phase("a1"), Some(HeartbeatPhase::Sleep));
    }

    #[test]
    fn heartbeat_config_serde_roundtrip() {
        let cfg = HeartbeatConfig::new("a1", Duration::from_secs(30));
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: HeartbeatConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.agent_id, "a1");
        assert_eq!(restored.interval, Duration::from_secs(30));
        assert!(restored.enabled);
    }
}
