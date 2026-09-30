//! Kernel governance configuration (ADR-103 D12).

use serde::{Deserialize, Serialize};

/// What the daemon allows a request that is "outside any project".
///
/// Outside a project means: no claimed project and the daemon is not bound
/// to one, or a claimed project that does not verify against the daemon's
/// binding or the manifest registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutsideProjectPolicy {
    /// Only an explicit allow-list of read methods; everything else is
    /// denied with `error_kind = "scope_denied"`. The default.
    #[default]
    ReadOnly,
    /// Only `kernel.status`, `kernel.handshake` and `project.*`.
    DenyAll,
    /// No scope restriction (the pre-ADR-103 behaviour).
    AllowAll,
}

/// `[kernel.governance]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GovernanceConfig {
    /// Policy for requests outside any project (`read_only` by default).
    #[serde(default, alias = "outsideProject")]
    pub outside_project: OutsideProjectPolicy,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KernelConfig;

    #[test]
    fn default_is_read_only() {
        assert_eq!(
            KernelConfig::default().governance.outside_project,
            OutsideProjectPolicy::ReadOnly
        );
        let k: KernelConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(k.governance.outside_project, OutsideProjectPolicy::ReadOnly);
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
            assert_eq!(k.governance.outside_project, want);
        }
        assert!(
            serde_json::from_str::<KernelConfig>(r#"{"governance":{"outside_project":"x"}}"#)
                .is_err()
        );
    }
}
