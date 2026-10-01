//! Kernel governance configuration (ADR-103 D12).

use serde::{Deserialize, Serialize};

/// What the daemon allows a request that is "outside any project".
///
/// Outside a project means: no claimed project and the daemon is not bound
/// to one, or a claimed project that does not verify against the daemon's
/// binding or the manifest registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutsideProjectPolicy {
    /// Only an explicit allow-list of read methods; everything else is
    /// denied with `error_kind = "project_required"`. The default for the
    /// user-profile daemon only (see [`default_outside_policy`]).
    ReadOnly,
    /// Only `kernel.status`, `kernel.handshake` and `project.*`.
    DenyAll,
    /// No scope restriction (the pre-ADR-103 behaviour).
    AllowAll,
}

/// `[kernel.governance]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GovernanceConfig {
    /// Policy for requests outside any project. `None` (unset) resolves to
    /// [`default_outside_policy`]; an explicit value always wins.
    #[serde(default, alias = "outsideProject", skip_serializing_if = "Option::is_none")]
    pub outside_project: Option<OutsideProjectPolicy>,
}

impl GovernanceConfig {
    /// The effective policy for a daemon (`is_user_profile`: it runs the
    /// `--profile user` root).
    pub fn effective_outside_project(&self, is_user_profile: bool) -> OutsideProjectPolicy {
        self.outside_project
            .unwrap_or_else(|| default_outside_policy(is_user_profile))
    }
}

/// Default outside-project policy when none is configured.
///
/// ADR-103 D12 (amendment pending): only the user-profile daemon, which
/// serves many projects and has no project of its own, defaults to
/// `read_only`. Every other root (project-bound, legacy `~/.clawft`,
/// env-isolated) defaults to `allow_all`, preserving pre-ADR-103 behaviour
/// until those installs migrate to the user profile.
pub fn default_outside_policy(is_user_profile: bool) -> OutsideProjectPolicy {
    if is_user_profile {
        OutsideProjectPolicy::ReadOnly
    } else {
        OutsideProjectPolicy::AllowAll
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KernelConfig;

    #[test]
    fn default_is_profile_keyed() {
        let g = KernelConfig::default().governance;
        assert_eq!(g.outside_project, None);
        assert_eq!(g.effective_outside_project(true), OutsideProjectPolicy::ReadOnly);
        assert_eq!(g.effective_outside_project(false), OutsideProjectPolicy::AllowAll);
        let k: KernelConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(k.governance, g);
    }

    #[test]
    fn explicit_policy_wins_on_every_profile() {
        let g = GovernanceConfig { outside_project: Some(OutsideProjectPolicy::DenyAll) };
        assert_eq!(g.effective_outside_project(true), OutsideProjectPolicy::DenyAll);
        assert_eq!(g.effective_outside_project(false), OutsideProjectPolicy::DenyAll);
    }

    #[test]
    fn parses_each_named_policy() {
        for (s, want) in [
            ("read_only", OutsideProjectPolicy::ReadOnly),
            ("deny_all", OutsideProjectPolicy::DenyAll),
            ("allow_all", OutsideProjectPolicy::AllowAll),
        ] {
            let k: KernelConfig =
                serde_json::from_str(&format!(r#"{{"governance":{{"outside_project":"{s}"}}}}"#))
                    .unwrap();
            assert_eq!(k.governance.outside_project, Some(want));
        }
        assert!(
            serde_json::from_str::<KernelConfig>(r#"{"governance":{"outside_project":"x"}}"#)
                .is_err()
        );
    }
}
