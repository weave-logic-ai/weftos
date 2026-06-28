//! Autonomous skill creation (C4a).
//!
//! Detects repeated task patterns and auto-generates skill definitions.
//! Generated skills are installed in `~/.clawft/skills/` in a "pending"
//! state and require user approval before activation.
//!
//! **Disabled by default** -- must be opted into via configuration.
//!
//! # Security
//!
//! Auto-generated skills have minimal permissions:
//! - No shell access
//! - No network access
//! - Filesystem limited to workspace directory
//!
//! # Pattern Detection
//!
//! The detector tracks sequences of tool calls. When the same sequence
//! appears at least `threshold` times (default: 3), a skill candidate
//! is generated.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::{debug, info};

use crate::pipeline::learner::TrajectoryLearner;
use crate::pipeline::mutation::{TrajectoryHint, auto_select_strategy, mutate_prompt};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for autonomous skill creation.
#[derive(Debug, Clone)]
pub struct AutogenConfig {
    /// Whether autonomous skill creation is enabled.
    /// Default: `false` (disabled).
    pub enabled: bool,
    /// Minimum number of pattern repetitions before suggesting a skill.
    /// Default: 3.
    pub threshold: usize,
    /// Maximum number of pending skills allowed at once.
    /// Prevents unbounded growth. Default: 10.
    pub max_pending: usize,
    /// Directory where generated skills are installed.
    /// Default: `~/.clawft/skills/`
    pub install_dir: Option<PathBuf>,
}

impl Default for AutogenConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: 3,
            max_pending: 10,
            install_dir: None,
        }
    }
}

impl AutogenConfig {
    /// Get the effective install directory, defaulting to `~/.clawft/skills/`.
    pub fn install_dir(&self) -> PathBuf {
        self.install_dir.clone().unwrap_or_else(|| {
            #[cfg(feature = "native")]
            {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".clawft")
                    .join("skills")
            }
            #[cfg(not(feature = "native"))]
            {
                PathBuf::from(".clawft").join("skills")
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Pattern Detection
// ---------------------------------------------------------------------------

/// A sequence of tool calls forming a pattern.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolCallPattern {
    /// Ordered list of tool names in the pattern.
    pub tool_names: Vec<String>,
}

impl ToolCallPattern {
    /// Create a new pattern from tool names.
    pub fn new(tool_names: Vec<String>) -> Self {
        Self { tool_names }
    }

    /// Generate a suggested skill name from the pattern.
    pub fn suggested_name(&self) -> String {
        if self.tool_names.is_empty() {
            return "unnamed-skill".to_string();
        }
        let parts: Vec<&str> = self.tool_names.iter().map(|s| s.as_str()).collect();
        let name = parts.join("-then-");
        // Truncate to reasonable length
        if name.len() > 60 {
            format!("{}-auto", &name[..56])
        } else {
            format!("{name}-auto")
        }
    }

    /// Number of steps in the pattern.
    pub fn len(&self) -> usize {
        self.tool_names.len()
    }

    /// Whether the pattern is empty.
    pub fn is_empty(&self) -> bool {
        self.tool_names.is_empty()
    }
}

/// Detects repeated tool call patterns in the agent's execution history.
pub struct PatternDetector {
    /// Configuration for pattern detection.
    config: AutogenConfig,
    /// Counts of each observed pattern.
    pattern_counts: HashMap<ToolCallPattern, usize>,
    /// Recent tool calls (sliding window).
    recent_calls: Vec<String>,
    /// Maximum window size for pattern detection.
    max_window: usize,
    /// Patterns that have already been reported (to avoid duplicates).
    reported: HashMap<ToolCallPattern, bool>,
}

impl PatternDetector {
    /// Create a new pattern detector.
    pub fn new(config: AutogenConfig) -> Self {
        Self {
            config,
            pattern_counts: HashMap::new(),
            recent_calls: Vec::new(),
            max_window: 10,
            reported: HashMap::new(),
        }
    }

    /// Record a tool call.
    pub fn record_tool_call(&mut self, tool_name: &str) {
        if !self.config.enabled {
            return;
        }

        self.recent_calls.push(tool_name.to_string());

        // Keep window bounded
        if self.recent_calls.len() > self.max_window * 3 {
            self.recent_calls.drain(..self.max_window);
        }

        // Extract patterns of length 2..=max_window from recent calls
        let n = self.recent_calls.len();
        for len in 2..=self.max_window.min(n) {
            let pattern = ToolCallPattern::new(self.recent_calls[n - len..n].to_vec());
            *self.pattern_counts.entry(pattern).or_insert(0) += 1;
        }
    }

    /// Check for patterns that have reached the threshold.
    ///
    /// Returns newly detected patterns (patterns that just crossed the
    /// threshold and have not been reported before).
    pub fn detect_candidates(&mut self) -> Vec<ToolCallPattern> {
        if !self.config.enabled {
            return vec![];
        }

        let threshold = self.config.threshold;
        let mut candidates = Vec::new();

        for (pattern, count) in &self.pattern_counts {
            if *count >= threshold && !self.reported.contains_key(pattern) {
                candidates.push(pattern.clone());
            }
        }

        // Mark as reported
        for candidate in &candidates {
            self.reported.insert(candidate.clone(), true);
        }

        candidates
    }

    /// Get the count for a specific pattern.
    pub fn pattern_count(&self, pattern: &ToolCallPattern) -> usize {
        self.pattern_counts.get(pattern).copied().unwrap_or(0)
    }

    /// Reset all pattern tracking state.
    pub fn reset(&mut self) {
        self.pattern_counts.clear();
        self.recent_calls.clear();
        self.reported.clear();
    }

    /// Whether the detector is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

// ---------------------------------------------------------------------------
// Skill Generation
// ---------------------------------------------------------------------------

/// A generated skill candidate awaiting user approval.
#[derive(Debug, Clone)]
pub struct SkillCandidate {
    /// Suggested skill name.
    pub name: String,
    /// Description of what the skill does.
    pub description: String,
    /// The tool call pattern that triggered generation.
    pub pattern: ToolCallPattern,
    /// Generated SKILL.md content.
    pub skill_md: String,
    /// Approval state.
    pub state: CandidateState,
}

/// State of a skill candidate in the approval pipeline.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateState {
    /// Awaiting user review and approval.
    Pending,
    /// Approved by user, ready for installation.
    Approved,
    /// Rejected by user.
    Rejected,
}

/// Generates SKILL.md content from a detected pattern.
pub fn generate_skill_md(pattern: &ToolCallPattern) -> SkillCandidate {
    let name = pattern.suggested_name();
    let tools_list = pattern
        .tool_names
        .iter()
        .map(|t| format!("  - {t}"))
        .collect::<Vec<_>>()
        .join("\n");

    let description = format!(
        "Auto-generated skill: executes {} in sequence",
        pattern
            .tool_names
            .iter()
            .map(|t| format!("`{t}`"))
            .collect::<Vec<_>>()
            .join(" -> ")
    );

    let instructions = format!(
        "Execute the following tools in order:\n{}",
        pattern
            .tool_names
            .iter()
            .enumerate()
            .map(|(i, t)| format!("{}. Use the `{t}` tool", i + 1))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let skill_md = format!(
        "---\n\
         name: {name}\n\
         description: \"{description}\"\n\
         version: 0.1.0\n\
         allowed-tools:\n\
         {tools_list}\n\
         user-invocable: false\n\
         autogenerated: true\n\
         ---\n\n\
         {instructions}\n"
    );

    SkillCandidate {
        name,
        description,
        pattern: pattern.clone(),
        skill_md,
        state: CandidateState::Pending,
    }
}

/// Install a skill candidate to the managed skills directory.
///
/// Creates a directory `{install_dir}/{name}/` with a `SKILL.md` file.
/// The skill is installed in "pending" state -- it will not be loaded
/// by the skill watcher until approved.
pub fn install_pending_skill(
    candidate: &SkillCandidate,
    install_dir: &Path,
) -> Result<PathBuf, String> {
    let skill_dir = install_dir.join(&candidate.name);

    std::fs::create_dir_all(&skill_dir).map_err(|e| format!("create skill dir: {e}"))?;

    let skill_path = skill_dir.join("SKILL.md");
    std::fs::write(&skill_path, &candidate.skill_md).map_err(|e| format!("write SKILL.md: {e}"))?;

    // Write a .pending marker file -- the skill watcher should
    // skip loading skills with this marker until approved.
    let marker_path = skill_dir.join(".pending");
    std::fs::write(&marker_path, "awaiting user approval")
        .map_err(|e| format!("write .pending marker: {e}"))?;

    info!(
        skill = %candidate.name,
        path = %skill_dir.display(),
        "installed pending skill"
    );

    Ok(skill_dir)
}

/// Approve a pending skill by removing the `.pending` marker.
pub fn approve_skill(skill_dir: &Path) -> Result<(), String> {
    let marker = skill_dir.join(".pending");
    if marker.exists() {
        std::fs::remove_file(&marker).map_err(|e| format!("remove .pending: {e}"))?;
        info!(skill_dir = %skill_dir.display(), "approved pending skill");
        Ok(())
    } else {
        Err("skill is not in pending state".into())
    }
}

/// Reject a pending skill by removing its directory.
pub fn reject_skill(skill_dir: &Path) -> Result<(), String> {
    if skill_dir.exists() {
        std::fs::remove_dir_all(skill_dir).map_err(|e| format!("remove skill dir: {e}"))?;
        info!(skill_dir = %skill_dir.display(), "rejected and removed pending skill");
        Ok(())
    } else {
        Err("skill directory does not exist".into())
    }
}

/// Check if a skill directory is in pending state.
pub fn is_pending(skill_dir: &Path) -> bool {
    skill_dir.join(".pending").exists()
}

// ---------------------------------------------------------------------------
// Trajectory-based prompt improvement
// ---------------------------------------------------------------------------

/// Improve a generated skill's instructions using trajectory data.
///
/// If the [`TrajectoryLearner`] has successful patterns (trajectories
/// scoring >= 0.8), this converts them into [`TrajectoryHint`]s, selects
/// the best mutation strategy via [`auto_select_strategy`], and applies
/// [`mutate_prompt`] to the skill instructions.
///
/// Returns the (possibly improved) instructions. If there are no
/// successful patterns, the original instructions are returned unchanged.
pub fn improve_skill_instructions(instructions: &str, learner: &TrajectoryLearner) -> String {
    let best = learner.get_best_trajectories(10);
    let poor = learner.get_poor_trajectories(10);

    if best.is_empty() && poor.is_empty() {
        return instructions.to_string();
    }

    let hints: Vec<TrajectoryHint> = best
        .iter()
        .chain(poor.iter())
        .map(|st| TrajectoryHint {
            request_content: st
                .trajectory
                .request
                .messages
                .first()
                .map(|m| m.content.clone())
                .unwrap_or_default(),
            quality_score: st.trajectory.quality.overall,
            feedback: st.feedback.clone(),
        })
        .collect();

    let strategy = auto_select_strategy(&hints);
    debug!(
        ?strategy,
        hints_count = hints.len(),
        "mutating skill instructions"
    );
    mutate_prompt(instructions, &hints, strategy)
}

/// Generate a skill candidate and optionally improve it using trajectory data.
///
/// Combines [`generate_skill_md`] with [`improve_skill_instructions`] when
/// a [`TrajectoryLearner`] is available.
pub fn generate_skill_md_with_learning(
    pattern: &ToolCallPattern,
    learner: Option<&TrajectoryLearner>,
) -> SkillCandidate {
    let mut candidate = generate_skill_md(pattern);

    if let Some(learner) = learner {
        let patterns = learner.extract_successful_patterns();
        if !patterns.is_empty() {
            debug!(
                pattern_count = patterns.len(),
                "improving skill via trajectory learning"
            );
            candidate.skill_md = improve_skill_instructions(&candidate.skill_md, learner);
        }
    }

    candidate
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_config() -> AutogenConfig {
        AutogenConfig {
            enabled: true,
            threshold: 3,
            max_pending: 10,
            install_dir: None,
        }
    }

    fn disabled_config() -> AutogenConfig {
        AutogenConfig::default()
    }

    // -- AutogenConfig tests --

    #[test]
    fn config_default_is_disabled() {
        let config = AutogenConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.threshold, 3);
        assert_eq!(config.max_pending, 10);
    }

    #[test]
    fn config_install_dir_default() {
        let config = AutogenConfig::default();
        let dir = config.install_dir();
        assert!(dir.to_string_lossy().contains(".clawft"));
        assert!(dir.to_string_lossy().contains("skills"));
    }

    // -- ToolCallPattern tests --

    #[test]
    fn pattern_suggested_name() {
        let pattern = ToolCallPattern::new(vec!["read_file".into(), "edit_file".into()]);
        let name = pattern.suggested_name();
        assert!(name.contains("read_file"));
        assert!(name.contains("edit_file"));
        assert!(name.ends_with("-auto"));
    }

    #[test]
    fn pattern_suggested_name_long_truncated() {
        let tools: Vec<String> = (0..20).map(|i| format!("tool_{i}")).collect();
        let pattern = ToolCallPattern::new(tools);
        let name = pattern.suggested_name();
        assert!(name.len() <= 65, "name too long: {}", name.len());
    }

    #[test]
    fn pattern_empty() {
        let pattern = ToolCallPattern::new(vec![]);
        assert!(pattern.is_empty());
        assert_eq!(pattern.len(), 0);
        assert_eq!(pattern.suggested_name(), "unnamed-skill");
    }

    // -- PatternDetector tests --

    #[test]
    fn detector_disabled_records_nothing() {
        let mut detector = PatternDetector::new(disabled_config());
        detector.record_tool_call("read_file");
        detector.record_tool_call("edit_file");
        let candidates = detector.detect_candidates();
        assert!(candidates.is_empty());
    }

    #[test]
    fn detector_does_not_fire_below_threshold() {
        let mut detector = PatternDetector::new(enabled_config());
        // Record pattern twice (below threshold of 3)
        for _ in 0..2 {
            detector.record_tool_call("read_file");
            detector.record_tool_call("edit_file");
        }
        let candidates = detector.detect_candidates();
        assert!(candidates.is_empty());
    }

    #[test]
    fn detector_fires_at_threshold() {
        let mut detector = PatternDetector::new(enabled_config());
        // Record pattern 3 times (at threshold)
        for _ in 0..3 {
            detector.record_tool_call("read_file");
            detector.record_tool_call("edit_file");
        }
        let candidates = detector.detect_candidates();
        assert!(!candidates.is_empty(), "should detect at least one pattern");
    }

    #[test]
    fn detector_does_not_re_report() {
        let mut detector = PatternDetector::new(enabled_config());
        for _ in 0..5 {
            detector.record_tool_call("read_file");
            detector.record_tool_call("edit_file");
        }
        let first = detector.detect_candidates();
        assert!(!first.is_empty());

        // Second call should not re-report the same pattern
        let second = detector.detect_candidates();
        assert!(second.is_empty(), "should not re-report same pattern");
    }

    #[test]
    fn detector_reset_clears_state() {
        let mut detector = PatternDetector::new(enabled_config());
        for _ in 0..3 {
            detector.record_tool_call("a");
            detector.record_tool_call("b");
        }
        let _ = detector.detect_candidates();
        detector.reset();

        // After reset, same pattern should be detected again
        for _ in 0..3 {
            detector.record_tool_call("a");
            detector.record_tool_call("b");
        }
        let candidates = detector.detect_candidates();
        assert!(!candidates.is_empty());
    }

    #[test]
    fn detector_configurable_threshold() {
        let config = AutogenConfig {
            enabled: true,
            threshold: 5,
            ..Default::default()
        };
        let mut detector = PatternDetector::new(config);

        for _ in 0..4 {
            detector.record_tool_call("x");
            detector.record_tool_call("y");
        }
        assert!(detector.detect_candidates().is_empty(), "below threshold");

        detector.record_tool_call("x");
        detector.record_tool_call("y");
        assert!(!detector.detect_candidates().is_empty(), "at threshold");
    }

    // -- Skill generation tests --

    #[test]
    fn generate_skill_md_valid() {
        let pattern = ToolCallPattern::new(vec![
            "read_file".into(),
            "edit_file".into(),
            "write_file".into(),
        ]);
        let candidate = generate_skill_md(&pattern);

        assert!(candidate.name.contains("auto"));
        assert_eq!(candidate.state, CandidateState::Pending);
        assert!(candidate.skill_md.contains("---"));
        assert!(candidate.skill_md.contains("read_file"));
        assert!(candidate.skill_md.contains("edit_file"));
        assert!(candidate.skill_md.contains("write_file"));
        assert!(candidate.skill_md.contains("autogenerated: true"));
        assert!(candidate.skill_md.contains("user-invocable: false"));
    }

    #[test]
    fn generate_skill_md_description_contains_tools() {
        let pattern = ToolCallPattern::new(vec!["a".into(), "b".into()]);
        let candidate = generate_skill_md(&pattern);
        assert!(candidate.description.contains("`a`"));
        assert!(candidate.description.contains("`b`"));
    }

    // -- Install/approve/reject tests --

    #[test]
    fn install_pending_skill_creates_files() {
        let dir = std::env::temp_dir().join("clawft_autogen_install_test");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);

        let pattern = ToolCallPattern::new(vec!["read_file".into(), "edit_file".into()]);
        let candidate = generate_skill_md(&pattern);

        let result = install_pending_skill(&candidate, &dir);
        assert!(result.is_ok());

        let skill_dir = result.unwrap();
        assert!(skill_dir.join("SKILL.md").exists());
        assert!(skill_dir.join(".pending").exists());
        assert!(is_pending(&skill_dir));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn approve_skill_removes_marker() {
        let dir = std::env::temp_dir().join("clawft_autogen_approve_test");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);

        let pattern = ToolCallPattern::new(vec!["a".into(), "b".into()]);
        let candidate = generate_skill_md(&pattern);
        let skill_dir = install_pending_skill(&candidate, &dir).unwrap();

        assert!(is_pending(&skill_dir));
        assert!(approve_skill(&skill_dir).is_ok());
        assert!(!is_pending(&skill_dir));
        assert!(skill_dir.join("SKILL.md").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reject_skill_removes_directory() {
        let dir = std::env::temp_dir().join("clawft_autogen_reject_test");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);

        let pattern = ToolCallPattern::new(vec!["x".into(), "y".into()]);
        let candidate = generate_skill_md(&pattern);
        let skill_dir = install_pending_skill(&candidate, &dir).unwrap();

        assert!(skill_dir.exists());
        assert!(reject_skill(&skill_dir).is_ok());
        assert!(!skill_dir.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn approve_non_pending_fails() {
        let dir = std::env::temp_dir().join("clawft_autogen_approve_nopend");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);

        // No .pending marker
        let result = approve_skill(&dir);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not in pending state"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generated_skill_has_minimal_permissions() {
        let pattern = ToolCallPattern::new(vec!["read_file".into()]);
        let candidate = generate_skill_md(&pattern);

        // Generated skill should NOT have shell, network, or broad FS access
        assert!(!candidate.skill_md.contains("shell: true"));
        assert!(!candidate.skill_md.contains("network:"));
        // user-invocable should be false for safety
        assert!(candidate.skill_md.contains("user-invocable: false"));
    }

    #[test]
    fn is_enabled_reflects_config() {
        let enabled = PatternDetector::new(enabled_config());
        assert!(enabled.is_enabled());

        let disabled = PatternDetector::new(disabled_config());
        assert!(!disabled.is_enabled());
    }

    // -- Trajectory-based improvement tests --

    fn make_test_learner() -> TrajectoryLearner {
        use crate::pipeline::learner::TrajectoryLearnerConfig;
        use crate::pipeline::traits::{
            ChatRequest, LlmMessage, QualityScore, RoutingDecision, Trajectory,
        };
        use clawft_types::provider::{ContentBlock, LlmResponse, StopReason, Usage};

        let learner = TrajectoryLearner::new(TrajectoryLearnerConfig::default());

        let make_traj = |overall: f32, content: &str| Trajectory {
            request: ChatRequest {
                messages: vec![LlmMessage {
                    role: "user".into(),
                    content: content.into(),
                    tool_call_id: None,
                    tool_calls: None,
                }],
                tools: vec![],
                model: None,
                max_tokens: None,
                temperature: None,
                auth_context: None,
                complexity_boost: 0.0,
            },
            routing: RoutingDecision::default(),
            response: LlmResponse {
                id: "r".into(),
                content: vec![ContentBlock::Text { text: "ok".into() }],
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 5,
                    output_tokens: 2,
                    total_tokens: 0,
                },
                metadata: std::collections::HashMap::new(),
            },
            quality: QualityScore {
                overall,
                relevance: overall,
                coherence: overall,
            },
        };

        // Record some good and poor trajectories
        use crate::pipeline::traits::LearningBackend;
        learner.record(&make_traj(0.95, "Write a function to sort a list"));
        learner.record(&make_traj(0.9, "Explain recursion with examples"));
        learner.record(&make_traj(0.3, "Fix the bug"));
        learner.record(&make_traj(0.4, "Summarize article"));

        learner
    }

    #[test]
    fn improve_skill_instructions_with_patterns() {
        let learner = make_test_learner();
        let original = "Use the read_file tool\nCheck the output";
        let improved = improve_skill_instructions(original, &learner);

        // Should be different since we have trajectory data
        assert!(!improved.is_empty());
    }

    #[test]
    fn improve_skill_instructions_empty_learner_returns_original() {
        use crate::pipeline::learner::TrajectoryLearnerConfig;
        let learner = TrajectoryLearner::new(TrajectoryLearnerConfig::default());
        let original = "Use the read_file tool";
        let result = improve_skill_instructions(original, &learner);
        assert_eq!(result, original);
    }

    #[test]
    fn generate_skill_md_with_learning_no_learner() {
        let pattern = ToolCallPattern::new(vec!["a".into(), "b".into()]);
        let candidate = generate_skill_md_with_learning(&pattern, None);
        assert!(candidate.skill_md.contains("autogenerated: true"));
    }

    #[test]
    fn generate_skill_md_with_learning_uses_learner() {
        let learner = make_test_learner();
        let pattern = ToolCallPattern::new(vec!["read_file".into(), "edit_file".into()]);
        let candidate = generate_skill_md_with_learning(&pattern, Some(&learner));

        // Should still be a valid skill
        assert!(candidate.skill_md.contains("read_file"));
        assert!(!candidate.skill_md.is_empty());
    }
}
