//! Task delegation engine.
//!
//! Routes tasks to the appropriate execution target (Local, Claude, Flow)
//! based on regex rule matching and complexity heuristics.
//!
//! Gated behind the `delegate` feature.

pub mod claude;
pub mod schema;

use clawft_types::delegation::{DelegationConfig, DelegationRule, DelegationTarget};
use regex::Regex;
use tracing::debug;

/// Engine that decides where to route a task based on configuration rules
/// and complexity heuristics.
pub struct DelegationEngine {
    config: DelegationConfig,
    compiled_rules: Vec<CompiledRule>,
}

/// A rule with its regex pre-compiled for efficient repeated matching.
struct CompiledRule {
    regex: Regex,
    target: DelegationTarget,
}

/// Complexity keywords that bump the complexity score.
const COMPLEXITY_KEYWORDS: &[&str] = &[
    "deploy",
    "refactor",
    "architect",
    "design",
    "optimize",
    "migrate",
    "security",
    "audit",
    "review",
    "analyze",
    "orchestrate",
    "coordinate",
    "integrate",
    "implement",
    "debug",
    "investigate",
    "comprehensive",
    "distributed",
    "concurrent",
    "parallel",
];

impl DelegationEngine {
    /// Create a new engine from the given configuration.
    ///
    /// Rules with invalid regex patterns are logged and skipped.
    pub fn new(config: DelegationConfig) -> Self {
        let compiled_rules = config
            .rules
            .iter()
            .filter_map(|rule: &DelegationRule| match Regex::new(&rule.pattern) {
                Ok(regex) => Some(CompiledRule {
                    regex,
                    target: rule.target,
                }),
                Err(e) => {
                    debug!(
                        pattern = %rule.pattern,
                        error = %e,
                        "skipping delegation rule with invalid regex"
                    );
                    None
                }
            })
            .collect();

        Self {
            config,
            compiled_rules,
        }
    }

    /// Decide which target should handle the given task.
    ///
    /// Evaluation order:
    /// 1. Walk compiled rules in order; first regex match wins.
    /// 2. If the matched target is `Claude` but `claude_available` is false,
    ///    fall back to `Local`.
    /// 3. If the matched target is `Flow`, treat as Claude (Flow delegation
    ///    removed in MCP-first architecture).
    /// 4. If no rule matches, use `Auto` mode (complexity heuristic).
    pub fn decide(
        &self,
        task: &str,
        claude_available: bool,
    ) -> DelegationTarget {
        // Try explicit rules first.
        for rule in &self.compiled_rules {
            if rule.regex.is_match(task) {
                let target =
                    self.resolve_availability(rule.target, claude_available);
                debug!(
                    task = %task,
                    matched_target = ?rule.target,
                    resolved_target = ?target,
                    "delegation rule matched"
                );
                return target;
            }
        }

        // No rule matched -- use Auto heuristic.
        self.auto_decide(task, claude_available)
    }

    /// Estimate task complexity on a 0.0..1.0 scale.
    ///
    /// Uses simple heuristics:
    /// - Normalised text length (longer = more complex)
    /// - Question mark density (questions suggest research)
    /// - Presence of complexity keywords
    pub fn complexity_estimate(task: &str) -> f32 {
        if task.is_empty() {
            return 0.0;
        }

        // Length factor: saturate at 500 characters.
        let len_factor = (task.len() as f32 / 500.0).min(1.0);

        // Question mark density.
        let qmark_count = task.chars().filter(|&c| c == '?').count() as f32;
        let qmark_factor = (qmark_count / 3.0).min(1.0);

        // Keyword hits.
        let lower = task.to_lowercase();
        let keyword_hits = COMPLEXITY_KEYWORDS
            .iter()
            .filter(|kw| lower.contains(*kw))
            .count() as f32;
        let keyword_factor = (keyword_hits / 4.0).min(1.0);

        // Weighted average: length 30%, questions 20%, keywords 50%.
        let score = len_factor * 0.3 + qmark_factor * 0.2 + keyword_factor * 0.5;
        score.min(1.0)
    }

    /// Auto-decide based on complexity.
    ///
    /// - Low complexity (< 0.3): Local
    /// - Medium/High complexity (>= 0.3): Claude (if available), else Local
    fn auto_decide(
        &self,
        task: &str,
        claude_available: bool,
    ) -> DelegationTarget {
        let complexity = Self::complexity_estimate(task);

        let target = if complexity < 0.3 {
            DelegationTarget::Local
        } else if claude_available && self.config.claude_enabled {
            DelegationTarget::Claude
        } else {
            DelegationTarget::Local
        };

        debug!(
            task = %task,
            complexity = complexity,
            target = ?target,
            "auto delegation decision"
        );

        target
    }

    /// Resolve a target given current availability.
    fn resolve_availability(
        &self,
        target: DelegationTarget,
        claude_available: bool,
    ) -> DelegationTarget {
        match target {
            DelegationTarget::Claude if !claude_available || !self.config.claude_enabled => {
                DelegationTarget::Local
            }
            // Flow delegation removed — treat as Claude fallback.
            DelegationTarget::Flow => {
                if claude_available && self.config.claude_enabled {
                    DelegationTarget::Claude
                } else {
                    DelegationTarget::Local
                }
            }
            DelegationTarget::Auto => {
                // Should not normally appear in rules, but handle gracefully.
                DelegationTarget::Auto
            }
            other => other,
        }
    }

    /// Delegate a tool call to another agent via A2A IPC.
    ///
    /// When a kernel A2A router is available, tool delegation uses
    /// PID-addressed IPC instead of in-process dispatch. This enables
    /// cross-agent tool delegation within a single kernel instance.
    ///
    /// Falls back to returning the DelegationTarget for external handling
    /// when no A2A router is configured.
    pub fn delegate_tool_call(
        &self,
        task: &str,
        claude_available: bool,
        target_pid: Option<u64>,
    ) -> DelegationResult {
        let target = self.decide(task, claude_available);
        DelegationResult {
            target,
            target_pid,
            task: task.to_owned(),
        }
    }

    /// Get a reference to the underlying config.
    pub fn config(&self) -> &DelegationConfig {
        &self.config
    }
}

/// Result of a delegation decision.
///
/// This is a plain data type that carries enough information for the caller
/// (who has access to an A2A router) to dispatch via kernel IPC when a
/// `target_pid` is present.
#[derive(Debug, Clone)]
pub struct DelegationResult {
    /// Where to route the task.
    pub target: DelegationTarget,
    /// Optional PID for kernel-internal delegation.
    pub target_pid: Option<u64>,
    /// The task description.
    pub task: String,
}

impl DelegationResult {
    /// Whether this delegation should use A2A IPC.
    pub fn is_kernel_local(&self) -> bool {
        self.target_pid.is_some()
    }

    /// Build payload for A2A dispatch.
    ///
    /// Returns `(target_pid, payload)` if a target PID is set,
    /// or `None` if this delegation should be handled externally.
    pub fn to_ipc_message(&self, _from_pid: u64) -> Option<(u64, serde_json::Value)> {
        let target_pid = self.target_pid?;
        let payload = serde_json::json!({
            "cmd": "delegate",
            "task": self.task,
            "target": format!("{:?}", self.target),
        });
        Some((target_pid, payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_types::delegation::{DelegationConfig, DelegationRule, DelegationTarget};

    fn make_engine(rules: Vec<DelegationRule>) -> DelegationEngine {
        DelegationEngine::new(DelegationConfig {
            claude_enabled: true,
            rules,
            ..Default::default()
        })
    }

    #[test]
    fn rule_matching_dispatches_correctly() {
        let engine = make_engine(vec![
            DelegationRule {
                pattern: r"(?i)deploy".into(),
                target: DelegationTarget::Flow,
            },
            DelegationRule {
                pattern: r"(?i)^list\b".into(),
                target: DelegationTarget::Local,
            },
            DelegationRule {
                pattern: r"(?i)research|analyze".into(),
                target: DelegationTarget::Claude,
            },
        ]);

        // Flow rules now resolve to Claude (Flow delegation removed).
        assert_eq!(
            engine.decide("deploy to production", true),
            DelegationTarget::Claude
        );
        assert_eq!(
            engine.decide("list all files", true),
            DelegationTarget::Local
        );
        assert_eq!(
            engine.decide("analyze the codebase", true),
            DelegationTarget::Claude
        );
        assert_eq!(
            engine.decide("research best practices", true),
            DelegationTarget::Claude
        );
    }

    #[test]
    fn fallback_when_claude_unavailable() {
        let engine = make_engine(vec![DelegationRule {
            pattern: r"(?i)research".into(),
            target: DelegationTarget::Claude,
        }]);

        // Claude unavailable: should fall back to Local.
        assert_eq!(
            engine.decide("research AI patterns", false),
            DelegationTarget::Local
        );
    }

    #[test]
    fn flow_target_falls_back_to_claude() {
        let engine = make_engine(vec![DelegationRule {
            pattern: r"(?i)deploy".into(),
            target: DelegationTarget::Flow,
        }]);

        // Flow delegation removed; Claude available: fall back to Claude.
        assert_eq!(
            engine.decide("deploy to staging", true),
            DelegationTarget::Claude
        );

        // Claude also unavailable: fall back to Local.
        assert_eq!(
            engine.decide("deploy to staging", false),
            DelegationTarget::Local
        );
    }

    #[test]
    fn auto_mode_low_complexity_is_local() {
        let engine = make_engine(vec![]);
        // Short, simple task with no complexity keywords.
        assert_eq!(engine.decide("hi", true), DelegationTarget::Local);
    }

    #[test]
    fn auto_mode_high_complexity_routes_to_claude() {
        let engine = make_engine(vec![]);
        // Many keywords + long text + question marks to push score >= 0.7.
        let task = "Please architect and design a comprehensive distributed \
                    system with concurrent processing, then deploy, optimize, \
                    migrate, refactor the security audit and review the \
                    integration. Can you also investigate the debug logs and \
                    coordinate the orchestration???";
        let score = DelegationEngine::complexity_estimate(task);
        assert!(score >= 0.7, "expected >= 0.7, got {score}");
        // Flow removed — high complexity now routes to Claude.
        assert_eq!(engine.decide(task, true), DelegationTarget::Claude);
    }

    #[test]
    fn auto_mode_medium_complexity_routes_to_claude() {
        let engine = make_engine(vec![]);
        // Enough keywords and length to land in 0.3..0.7 range.
        let task = "Please review this function, analyze the performance \
                    characteristics, and investigate potential optimizations \
                    for the codebase";
        let score = DelegationEngine::complexity_estimate(task);
        assert!(
            (0.3..0.7).contains(&score),
            "expected 0.3..0.7, got {score}"
        );
        assert_eq!(engine.decide(task, true), DelegationTarget::Claude);
    }

    #[test]
    fn auto_mode_falls_back_to_local_when_services_disabled() {
        let engine = DelegationEngine::new(DelegationConfig {
            claude_enabled: false,
            ..Default::default()
        });
        let task = "architect and design a comprehensive distributed system \
                    with security audit and deploy orchestration??";
        assert_eq!(engine.decide(task, true), DelegationTarget::Local);
    }

    #[test]
    fn complexity_estimate_empty_is_zero() {
        assert_eq!(DelegationEngine::complexity_estimate(""), 0.0);
    }

    #[test]
    fn complexity_estimate_scales_with_keywords() {
        let low = DelegationEngine::complexity_estimate("hello world");
        let high = DelegationEngine::complexity_estimate(
            "architect and design a comprehensive distributed system \
             with security audit",
        );
        assert!(low < high, "low={low}, high={high}");
    }

    #[test]
    fn complexity_estimate_capped_at_one() {
        let very_complex = "deploy refactor architect design optimize migrate \
                           security audit review analyze orchestrate coordinate \
                           integrate implement debug investigate comprehensive \
                           distributed concurrent parallel????";
        let score = DelegationEngine::complexity_estimate(very_complex);
        assert!(score <= 1.0, "score={score}");
        assert!(score > 0.8, "should be high complexity, got {score}");
    }

    #[test]
    fn invalid_regex_skipped() {
        let engine = make_engine(vec![
            DelegationRule {
                pattern: r"[invalid".into(), // broken regex
                target: DelegationTarget::Claude,
            },
            DelegationRule {
                pattern: r"(?i)hello".into(),
                target: DelegationTarget::Local,
            },
        ]);
        // The broken rule is skipped; "hello" still matches.
        assert_eq!(
            engine.decide("hello world", true),
            DelegationTarget::Local
        );
    }

    #[test]
    fn delegate_tool_call_with_pid() {
        let engine = make_engine(vec![DelegationRule {
            pattern: r"(?i)deploy".into(),
            target: DelegationTarget::Claude,
        }]);
        let result = engine.delegate_tool_call("deploy service", true, Some(42));
        assert_eq!(result.target, DelegationTarget::Claude);
        assert_eq!(result.target_pid, Some(42));
        assert_eq!(result.task, "deploy service");
        assert!(result.is_kernel_local());
    }

    #[test]
    fn delegate_tool_call_without_pid() {
        let engine = make_engine(vec![]);
        let result = engine.delegate_tool_call("hello", true, None);
        assert_eq!(result.target, DelegationTarget::Local);
        assert_eq!(result.target_pid, None);
        assert!(!result.is_kernel_local());
    }

    #[test]
    fn delegation_result_to_ipc_message() {
        let result = DelegationResult {
            target: DelegationTarget::Claude,
            target_pid: Some(99),
            task: "analyze logs".to_owned(),
        };
        let (pid, payload) = result.to_ipc_message(1).expect("should produce message");
        assert_eq!(pid, 99);
        assert_eq!(payload["cmd"], "delegate");
        assert_eq!(payload["task"], "analyze logs");
        assert_eq!(payload["target"], "Claude");

        // Without a target_pid, to_ipc_message returns None.
        let no_pid = DelegationResult {
            target: DelegationTarget::Local,
            target_pid: None,
            task: "noop".to_owned(),
        };
        assert!(no_pid.to_ipc_message(1).is_none());
    }

    #[test]
    fn first_rule_wins() {
        let engine = make_engine(vec![
            DelegationRule {
                pattern: r"(?i)deploy".into(),
                target: DelegationTarget::Flow,
            },
            DelegationRule {
                pattern: r"(?i)deploy".into(),
                target: DelegationTarget::Local,
            },
        ]);
        // Flow rules resolve to Claude now.
        assert_eq!(
            engine.decide("deploy now", true),
            DelegationTarget::Claude
        );
    }
}
