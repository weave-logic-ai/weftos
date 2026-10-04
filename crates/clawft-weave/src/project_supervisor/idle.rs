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

    /// True when `project_id`'s child had a session and has missed three
    /// heartbeats without registering again (a wedged or cut-off child).
    /// The supervisor restarts such a child after a grace period; sources
    /// without heartbeat data say `false`.
    fn lost_heartbeat(&self, _project_id: &str) -> bool {
        false
    }

    /// True when `project_id`'s session expired like [`lost_heartbeat`]
    /// (Self::lost_heartbeat) but its last beat said the child was busy, so
    /// it is spared until a much longer ceiling
    /// (`lost_heartbeat_busy_ceiling`): a child wedged while busy must
    /// not be kept for ever.
    fn lost_heartbeat_busy(&self, _project_id: &str) -> bool {
        false
    }

    /// True when `project_id` was adopted after a daemon restart and has not
    /// registered since (only the expired tombstone adoption filed exists).
    /// Such a child is never restarted for a lost heartbeat, so its silence
    /// is reported through `status` instead.
    fn unregistered_adopted(&self, _project_id: &str) -> bool {
        false
    }
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

    fn lost_heartbeat(&self, project_id: &str) -> bool {
        use crate::mesh_local_registry::{SessionState, registry};
        // Only a session that really lived and then went quiet counts: not
        // the tombstone adoption files (the child may never re-register, e.g.
        // an older build), and not a child whose last beat said it was busy
        // (a stalled heartbeat handler must not restart working children).
        registry().sessions().into_iter().any(|(s, state)| {
            s.facts.project_id == project_id
                && state == SessionState::Expired
                && !s.adopted
                && s.activity.busy.agents == 0
                && s.activity.busy.workloads == 0
                && s.activity.busy.streams == 0
        })
    }

    fn lost_heartbeat_busy(&self, project_id: &str) -> bool {
        use crate::mesh_local_registry::{SessionState, registry};
        registry().sessions().into_iter().any(|(s, state)| {
            s.facts.project_id == project_id
                && state == SessionState::Expired
                && !s.adopted
                && (s.activity.busy.agents > 0 || s.activity.busy.workloads > 0 || s.activity.busy.streams > 0)
        })
    }

    fn unregistered_adopted(&self, project_id: &str) -> bool {
        use crate::mesh_local_registry::{SessionState, registry};
        registry().sessions().into_iter().any(|(s, state)| {
            s.facts.project_id == project_id && state == SessionState::Expired && s.adopted
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

    fn facts(id: &str) -> crate::mesh_local_registry::NewSession {
        crate::mesh_local_registry::NewSession {
            project_id: id.to_owned(),
            socket: "/tmp/x.sock".into(),
            pid: 1,
            container: None,
            addresses: vec![id.to_owned()],
            topic_prefixes: vec![format!("chain/{id}/")],
            version: "0".into(),
            project_key_id: "k".into(),
            project_pubkey: [0; 32],
        }
    }

    /// Through the real (global) registry: only a session that lived and then
    /// went quiet, without a busy last beat, is a lost heartbeat.
    #[test]
    fn the_registry_reports_lost_heartbeats_but_not_adoption_tombstones_or_busy_children() {
        use crate::mesh_local_registry::registry;
        let reg = registry();
        let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(100);
        let src = RegistryActivity;
        // Adopted and never registered since: the tombstone is expired from
        // birth and must never count (an older build may not re-register).
        let adopted = "01J000000000000000000ADOPT";
        reg.adopt_expired(facts(adopted));
        assert!(!src.lost_heartbeat(adopted));
        // Registered, then silent for 100 s: lost.
        let lost = "01J00000000000000000000LOS";
        reg.register_at(facts(lost), long_ago).unwrap();
        assert!(src.lost_heartbeat(lost));
        // Same, but its last beat said it was busy: not restarted.
        let busy = "01J00000000000000000000BSY";
        let s = reg.register_at(facts(busy), long_ago).unwrap();
        let act = clawft_rpc::mesh_local::Activity {
            busy: clawft_rpc::mesh_local::Busy { agents: 1, workloads: 0, streams: 0 },
            ..Default::default()
        };
        reg.heartbeat_at(&s.session, act, long_ago + std::time::Duration::from_secs(1)).unwrap();
        assert!(!src.lost_heartbeat(busy));
        assert!(src.lost_heartbeat_busy(busy), "the busy child is on the long ceiling instead");
        assert!(!src.lost_heartbeat_busy(lost) && !src.lost_heartbeat_busy(adopted));
        assert!(!src.lost_heartbeat("01J00000000000000000000NON"));
        // The adoption tombstone is the "never re-registered" signal.
        assert!(src.unregistered_adopted(adopted));
        assert!(!src.unregistered_adopted(lost) && !src.unregistered_adopted(busy));
        for id in [adopted, lost, busy] {
            reg.evict(id);
        }
    }
}
