//! Idle stop (ADR-103 A6, decision 3): a project with `idle_stop_secs > 0`
//! and no activity for that long is stopped gracefully (final anchor first).
//!
//! Activity is what the child reports in `mesh.heartbeat` (package H's
//! registry implements [`ActivitySource`]): the last time anything other than
//! status, health and handshake calls happened, plus running agents,
//! workloads and open streams. **No data means busy**: the supervisor never
//! stops a project it cannot see.

/// What a child last reported.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Activity {
    /// Unix seconds of the last real activity.
    pub last_activity_unix: u64,
    /// Running agents.
    pub busy_agents: u32,
    /// Running workloads.
    pub busy_workloads: u32,
    /// Open streams.
    pub busy_streams: u32,
}

impl Activity {
    /// True when anything is running or open.
    pub fn busy(&self) -> bool {
        self.busy_agents > 0 || self.busy_workloads > 0 || self.busy_streams > 0
    }
}

/// Where the supervisor reads child activity from.
pub trait ActivitySource: Send + Sync {
    /// The latest report for `project_id`, `None` when there is none.
    fn activity(&self, project_id: &str) -> Option<Activity>;
}

/// Activity from the mesh-local registry: what each child last said in its
/// signed `mesh.heartbeat`. A project with no live session reports nothing,
/// so it is never idle-stopped.
#[derive(Debug, Default, Clone, Copy)]
pub struct RegistryActivity;

impl ActivitySource for RegistryActivity {
    fn activity(&self, project_id: &str) -> Option<Activity> {
        use crate::mesh_local_registry::{SessionState, registry};
        registry()
            .sessions()
            .into_iter()
            .find(|(s, state)| s.facts.project_id == project_id && *state == SessionState::Live)
            .map(|(s, _)| Activity {
                last_activity_unix: s.activity.last_activity_unix,
                busy_agents: s.activity.busy.agents,
                busy_workloads: s.activity.busy.workloads,
                busy_streams: s.activity.busy.streams,
            })
    }
}

/// The default source until a registry is installed: reports nothing, so no
/// project is ever idle-stopped.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoActivity;

impl ActivitySource for NoActivity {
    fn activity(&self, _project_id: &str) -> Option<Activity> {
        None
    }
}

/// Should a project with `idle_stop_secs` be stopped at `now_unix`?
pub fn should_stop(now_unix: u64, idle_stop_secs: u64, activity: Option<&Activity>) -> bool {
    if idle_stop_secs == 0 {
        return false;
    }
    let Some(a) = activity else { return false };
    !a.busy() && now_unix.saturating_sub(a.last_activity_unix) >= idle_stop_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(last: u64) -> Activity {
        Activity { last_activity_unix: last, ..Activity::default() }
    }

    #[test]
    fn stops_only_when_quiet_for_long_enough() {
        assert!(!should_stop(2000, 0, Some(&act(0))), "0 means never");
        assert!(!should_stop(2000, 1800, None), "no data means busy");
        assert!(!should_stop(2000, 1800, Some(&act(1000))));
        assert!(should_stop(2800, 1800, Some(&act(1000))));
    }

    #[test]
    fn any_busy_count_blocks_the_stop() {
        for a in [
            Activity { busy_agents: 1, ..act(0) },
            Activity { busy_workloads: 1, ..act(0) },
            Activity { busy_streams: 1, ..act(0) },
        ] {
            assert!(!should_stop(10_000, 1800, Some(&a)));
        }
    }
}
