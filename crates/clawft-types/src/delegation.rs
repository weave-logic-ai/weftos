//! Delegation configuration types.
//!
//! Controls how tasks are dispatched between local execution, Claude AI,
//! and Claude Flow orchestration. Rules use regex patterns to match task
//! descriptions and route them to the appropriate target.

use serde::{Deserialize, Serialize};

// ── DelegationConfig ────────────────────────────────────────────────────

/// Root configuration for task delegation routing.
///
/// When a task arrives, rules are evaluated in order. The first matching
/// rule determines the target. If no rule matches, the `Auto` target is
/// used (which applies a complexity heuristic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationConfig {
    /// Whether Claude AI delegation is enabled.
    ///
    /// **Default divergence (footgun, documented intentionally):**
    /// - `DelegationConfig::default()` sets this to `true` so a freshly
    ///   constructed config — used when no `[delegation]` table is present —
    ///   degrades gracefully (the engine treats Claude as unavailable when
    ///   no API key is configured).
    /// - `serde(default)` here uses [`bool::default()`] (which is `false`),
    ///   so a `[delegation]` table that *omits* this key parses as
    ///   `claude_enabled = false`.
    ///
    /// In other words: write nothing → Claude on; write `[delegation]` with
    /// other keys but no `claude_enabled` → Claude off. Tests in this module
    /// pin both behaviours (`delegation_config_defaults`,
    /// `delegation_config_from_empty_json`); if you change the runtime
    /// default, update both.
    #[serde(default)]
    pub claude_enabled: bool,

    /// Claude model identifier (e.g. `"claude-sonnet-4-20250514"`).
    #[serde(default = "default_delegation_model", alias = "claudeModel")]
    pub claude_model: String,

    /// Maximum conversation turns per delegated task.
    #[serde(default = "default_max_turns", alias = "maxTurns")]
    pub max_turns: u32,

    /// Maximum tokens per Claude response.
    #[serde(default = "default_max_tokens", alias = "maxTokens")]
    pub max_tokens: u32,

    /// Whether Claude Flow orchestration is enabled.
    #[serde(default, alias = "claudeFlowEnabled")]
    pub claude_flow_enabled: bool,

    /// Ordered list of routing rules. First match wins.
    #[serde(default)]
    pub rules: Vec<DelegationRule>,

    /// Tool names that should never be delegated.
    #[serde(default, alias = "excludedTools")]
    pub excluded_tools: Vec<String>,
}

fn default_delegation_model() -> String {
    "claude-sonnet-4-20250514".into()
}

fn default_max_turns() -> u32 {
    10
}

fn default_max_tokens() -> u32 {
    4096
}

impl Default for DelegationConfig {
    fn default() -> Self {
        Self {
            claude_enabled: true, // Gracefully degrades if no API key
            claude_model: default_delegation_model(),
            max_turns: default_max_turns(),
            max_tokens: default_max_tokens(),
            claude_flow_enabled: false, // Stays false until Flow fully wired
            rules: Vec::new(),
            excluded_tools: Vec::new(),
        }
    }
}

// ── DelegationRule ──────────────────────────────────────────────────────

/// A single routing rule that maps a regex pattern to a delegation target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationRule {
    /// Regex pattern matched against the task description.
    pub pattern: String,

    /// Where to route matching tasks.
    pub target: DelegationTarget,
}

// ── DelegationTarget ────────────────────────────────────────────────────

/// Where a task should be executed.
///
/// Serializes to snake_case (`"local"`, `"claude"`, `"flow"`, `"auto"`).
/// For backward compatibility, old PascalCase values (`"Local"`, `"Claude"`,
/// `"Flow"`, `"Auto"`) are accepted on deserialization via serde aliases.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationTarget {
    /// Execute locally (built-in tool pipeline).
    #[serde(alias = "Local")]
    Local,
    /// Delegate to Claude AI.
    #[serde(alias = "Claude")]
    Claude,
    /// Delegate to Claude Flow orchestration.
    #[serde(alias = "Flow")]
    Flow,
    /// Automatically decide based on complexity heuristics.
    #[serde(alias = "Auto")]
    #[default]
    Auto,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegation_config_defaults() {
        let cfg = DelegationConfig::default();
        assert!(cfg.claude_enabled); // M3: defaults to true, degrades gracefully
        assert_eq!(cfg.claude_model, "claude-sonnet-4-20250514");
        assert_eq!(cfg.max_turns, 10);
        assert_eq!(cfg.max_tokens, 4096);
        assert!(!cfg.claude_flow_enabled);
        assert!(cfg.rules.is_empty());
        assert!(cfg.excluded_tools.is_empty());
    }

    #[test]
    fn delegation_config_serde_roundtrip() {
        let cfg = DelegationConfig {
            claude_enabled: true,
            claude_model: "claude-opus-4-20250514".into(),
            max_turns: 5,
            max_tokens: 2048,
            claude_flow_enabled: true,
            rules: vec![
                DelegationRule {
                    pattern: r"(?i)deploy".into(),
                    target: DelegationTarget::Flow,
                },
                DelegationRule {
                    pattern: r"(?i)simple.*query".into(),
                    target: DelegationTarget::Local,
                },
            ],
            excluded_tools: vec!["shell_exec".into()],
        };

        let json = serde_json::to_string(&cfg).unwrap();
        let restored: DelegationConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.claude_enabled, cfg.claude_enabled);
        assert_eq!(restored.claude_model, cfg.claude_model);
        assert_eq!(restored.max_turns, cfg.max_turns);
        assert_eq!(restored.max_tokens, cfg.max_tokens);
        assert_eq!(restored.claude_flow_enabled, cfg.claude_flow_enabled);
        assert_eq!(restored.rules.len(), 2);
        assert_eq!(restored.rules[0].pattern, r"(?i)deploy");
        assert_eq!(restored.rules[0].target, DelegationTarget::Flow);
        assert_eq!(restored.rules[1].target, DelegationTarget::Local);
        assert_eq!(restored.excluded_tools, vec!["shell_exec"]);
    }

    #[test]
    fn delegation_config_from_empty_json() {
        // WEFT-203: pinning the documented divergence between
        // `Default::default()` (claude_enabled = true) and
        // `serde(default)` on `{}` (claude_enabled = false). See the
        // doc-comment on `DelegationConfig::claude_enabled`.
        let cfg: DelegationConfig = serde_json::from_str("{}").unwrap();
        assert!(!cfg.claude_enabled);
        assert_eq!(cfg.claude_model, "claude-sonnet-4-20250514");
        assert_eq!(cfg.max_turns, 10);
        assert_eq!(cfg.max_tokens, 4096);
        assert!(!cfg.claude_flow_enabled);
        assert!(cfg.rules.is_empty());
        assert!(cfg.excluded_tools.is_empty());
    }

    #[test]
    fn delegation_config_camel_case_aliases() {
        let json = r#"{
            "claudeModel": "test-model",
            "maxTurns": 3,
            "maxTokens": 1024,
            "claudeFlowEnabled": true,
            "excludedTools": ["dangerous_tool"]
        }"#;
        let cfg: DelegationConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.claude_model, "test-model");
        assert_eq!(cfg.max_turns, 3);
        assert_eq!(cfg.max_tokens, 1024);
        assert!(cfg.claude_flow_enabled);
        assert_eq!(cfg.excluded_tools, vec!["dangerous_tool"]);
    }

    #[test]
    fn delegation_target_serializes_snake_case() {
        let targets = [
            (DelegationTarget::Local, "\"local\""),
            (DelegationTarget::Claude, "\"claude\""),
            (DelegationTarget::Flow, "\"flow\""),
            (DelegationTarget::Auto, "\"auto\""),
        ];
        for (target, expected_json) in &targets {
            let json = serde_json::to_string(target).unwrap();
            assert_eq!(&json, expected_json);
            let restored: DelegationTarget = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, *target);
        }
    }

    #[test]
    fn delegation_target_deserializes_legacy_pascal_case() {
        // Backward compat: old PascalCase values still deserialize.
        let cases = [
            ("\"Local\"", DelegationTarget::Local),
            ("\"Claude\"", DelegationTarget::Claude),
            ("\"Flow\"", DelegationTarget::Flow),
            ("\"Auto\"", DelegationTarget::Auto),
        ];
        for (json, expected) in &cases {
            let restored: DelegationTarget = serde_json::from_str(json).unwrap();
            assert_eq!(restored, *expected);
        }
    }

    #[test]
    fn delegation_target_default_is_auto() {
        assert_eq!(DelegationTarget::default(), DelegationTarget::Auto);
    }
}
