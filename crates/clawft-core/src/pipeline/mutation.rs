//! Prompt mutation strategies (GEPA-inspired, v1).
//!
//! Simple string-level mutations that do not require an LLM call.
//! These transform a prompt based on trajectory data to explore
//! the prompt space.
//!
//! # Strategies
//!
//! - **Rephrase**: Restructure sentences for clarity
//! - **Add examples**: Inject successful trajectory patterns
//! - **Remove ineffective**: Strip phrases associated with poor outcomes
//! - **Emphasize**: Strengthen instructions that correlate with success

/// A recorded trajectory summary used as mutation input.
#[derive(Debug, Clone)]
pub struct TrajectoryHint {
    /// The user's original request content.
    pub request_content: String,
    /// Quality score (0.0--1.0) for this trajectory.
    pub quality_score: f32,
    /// Short feedback describing the trajectory outcome.
    pub feedback: String,
}

/// Available mutation strategies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationStrategy {
    /// Restructure sentences and add explicit instruction markers.
    Rephrase,
    /// Add examples derived from successful trajectories.
    AddExamples,
    /// Remove phrases associated with poor outcomes.
    RemoveIneffective,
    /// Strengthen key instruction phrases.
    Emphasize,
}

/// Mutate a prompt using the specified strategy and trajectory data.
///
/// Returns a new prompt string. The original is not modified.
///
/// # Arguments
///
/// - `prompt` -- The current prompt text to mutate
/// - `trajectories` -- Historical trajectory hints for context
/// - `strategy` -- Which mutation strategy to apply
pub fn mutate_prompt(
    prompt: &str,
    trajectories: &[TrajectoryHint],
    strategy: MutationStrategy,
) -> String {
    match strategy {
        MutationStrategy::Rephrase => rephrase(prompt),
        MutationStrategy::AddExamples => add_examples(prompt, trajectories),
        MutationStrategy::RemoveIneffective => remove_ineffective(prompt, trajectories),
        MutationStrategy::Emphasize => emphasize(prompt),
    }
}

/// Auto-select the best mutation strategy based on trajectory data.
///
/// Heuristics:
/// - If many poor trajectories exist, try removing ineffective patterns
/// - If good examples exist, try adding them
/// - Otherwise, rephrase for clarity
pub fn auto_select_strategy(trajectories: &[TrajectoryHint]) -> MutationStrategy {
    let poor_count = trajectories
        .iter()
        .filter(|t| t.quality_score < 0.6)
        .count();
    let good_count = trajectories
        .iter()
        .filter(|t| t.quality_score >= 0.8)
        .count();

    if poor_count > good_count && poor_count > 2 {
        MutationStrategy::RemoveIneffective
    } else if good_count >= 2 {
        MutationStrategy::AddExamples
    } else {
        MutationStrategy::Rephrase
    }
}

// ─── Strategy implementations ─────────────────────────────────────────

/// Rephrase: add instruction markers and restructure for clarity.
fn rephrase(prompt: &str) -> String {
    let mut result = String::with_capacity(prompt.len() + 100);

    let lines: Vec<&str> = prompt.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            result.push('\n');
            continue;
        }

        // Add explicit step markers to instruction-like lines
        if looks_like_instruction(trimmed) && !trimmed.starts_with(|c: char| c.is_ascii_digit()) {
            result.push_str(&format!("Step {}: {trimmed}\n", i + 1));
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }

    // Ensure trailing newline is clean
    while result.ends_with("\n\n") {
        result.pop();
    }

    result
}

/// Add examples from successful trajectories as a new section.
fn add_examples(prompt: &str, trajectories: &[TrajectoryHint]) -> String {
    let good_examples: Vec<&TrajectoryHint> = trajectories
        .iter()
        .filter(|t| t.quality_score >= 0.8)
        .take(3) // Limit to 3 examples to keep prompt concise
        .collect();

    if good_examples.is_empty() {
        return prompt.to_string();
    }

    let mut result = prompt.to_string();
    result.push_str("\n\n## Successful Examples\n\n");

    for (i, example) in good_examples.iter().enumerate() {
        // Truncate long examples
        let content = if example.request_content.len() > 200 {
            format!("{}...", &example.request_content[..197])
        } else {
            example.request_content.clone()
        };
        result.push_str(&format!(
            "Example {}: {}\n(Score: {:.2})\n\n",
            i + 1,
            content,
            example.quality_score
        ));
    }

    result
}

/// Remove phrases from the prompt that appear correlated with poor outcomes.
fn remove_ineffective(prompt: &str, trajectories: &[TrajectoryHint]) -> String {
    // Collect feedback phrases from poor trajectories
    let poor_phrases: Vec<String> = trajectories
        .iter()
        .filter(|t| t.quality_score < 0.5)
        .flat_map(|t| {
            // Extract short phrases from feedback that might appear in the prompt
            t.feedback
                .split(['.', ',', ';'])
                .map(|s| s.trim().to_lowercase())
                .filter(|s| s.len() > 5 && s.len() < 60)
                .collect::<Vec<_>>()
        })
        .collect();

    if poor_phrases.is_empty() {
        return prompt.to_string();
    }

    let lower_prompt = prompt.to_lowercase();
    let mut result = prompt.to_string();

    // Remove lines from the prompt that contain poor-outcome phrases
    // (conservative: only remove if a phrase appears verbatim)
    for phrase in &poor_phrases {
        if lower_prompt.contains(phrase.as_str()) {
            // Remove the line containing this phrase
            let lines: Vec<&str> = result.lines().collect();
            let filtered: Vec<&str> = lines
                .into_iter()
                .filter(|line| !line.to_lowercase().contains(phrase.as_str()))
                .collect();
            result = filtered.join("\n");
        }
    }

    if result.trim().is_empty() {
        // Safety: don't return an empty prompt
        return prompt.to_string();
    }

    result
}

/// Emphasize key instruction phrases by making them more explicit.
fn emphasize(prompt: &str) -> String {
    let mut result = String::with_capacity(prompt.len() + 100);

    for line in prompt.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            result.push('\n');
            continue;
        }

        // Emphasize imperative instructions
        if looks_like_instruction(trimmed) {
            // Add emphasis markers
            result.push_str("IMPORTANT: ");
            result.push_str(trimmed);
            result.push('\n');
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }

    while result.ends_with("\n\n") {
        result.pop();
    }

    result
}

/// Heuristic: does this line look like an instruction?
fn looks_like_instruction(line: &str) -> bool {
    let lower = line.to_lowercase();
    let instruction_verbs = [
        "use ", "execute ", "run ", "create ", "write ", "read ",
        "check ", "ensure ", "verify ", "always ", "never ", "do ",
        "avoid ", "make ", "set ", "apply ", "follow ", "implement ",
    ];
    instruction_verbs.iter().any(|v| lower.starts_with(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_trajectories() -> Vec<TrajectoryHint> {
        vec![
            TrajectoryHint {
                request_content: "Write a function to sort a list".into(),
                quality_score: 0.9,
                feedback: "Excellent quality.".into(),
            },
            TrajectoryHint {
                request_content: "Explain recursion with examples".into(),
                quality_score: 0.85,
                feedback: "Good explanation with clear examples.".into(),
            },
            TrajectoryHint {
                request_content: "Fix the bug in my code".into(),
                quality_score: 0.3,
                feedback: "Low relevance: response may not address the request. Did not identify the actual bug.".into(),
            },
            TrajectoryHint {
                request_content: "Summarize this article".into(),
                quality_score: 0.4,
                feedback: "Low coherence: response was disjointed and unclear.".into(),
            },
        ]
    }

    #[test]
    fn mutate_rephrase_produces_different_output() {
        let prompt = "Use the read tool\nCheck the output\nWrite the result";
        let result = mutate_prompt(prompt, &[], MutationStrategy::Rephrase);
        assert_ne!(result, prompt);
        assert!(result.contains("Step"));
    }

    #[test]
    fn mutate_rephrase_preserves_non_instruction_lines() {
        let prompt = "This is context information.\nSome background detail.";
        let result = mutate_prompt(prompt, &[], MutationStrategy::Rephrase);
        assert!(result.contains("This is context information."));
        assert!(result.contains("Some background detail."));
    }

    #[test]
    fn mutate_add_examples_inserts_section() {
        let prompt = "You are a helpful assistant.";
        let trajectories = sample_trajectories();
        let result = mutate_prompt(prompt, &trajectories, MutationStrategy::AddExamples);

        assert!(result.contains("You are a helpful assistant."));
        assert!(result.contains("Successful Examples"));
        assert!(result.contains("Write a function"));
    }

    #[test]
    fn mutate_add_examples_noop_without_good_trajectories() {
        let prompt = "You are a helpful assistant.";
        let poor_only = vec![TrajectoryHint {
            request_content: "bad".into(),
            quality_score: 0.2,
            feedback: "poor".into(),
        }];
        let result = mutate_prompt(prompt, &poor_only, MutationStrategy::AddExamples);
        assert_eq!(result, prompt);
    }

    #[test]
    fn mutate_remove_ineffective_filters_matching_lines() {
        let prompt = "Use the search tool\nLow relevance: response may not address the request\nAlways verify output";
        let trajectories = sample_trajectories();
        let result = mutate_prompt(prompt, &trajectories, MutationStrategy::RemoveIneffective);

        // The line matching poor feedback should be removed
        assert!(!result.contains("Low relevance"));
        // Other lines should remain
        assert!(result.contains("Use the search tool"));
    }

    #[test]
    fn mutate_remove_ineffective_preserves_prompt_if_no_matches() {
        let prompt = "Completely unrelated content here.";
        let trajectories = sample_trajectories();
        let result = mutate_prompt(prompt, &trajectories, MutationStrategy::RemoveIneffective);
        assert_eq!(result, prompt);
    }

    #[test]
    fn mutate_remove_ineffective_never_returns_empty() {
        // Even if all lines match, the original should be returned
        let prompt = "low relevance: response may not address the request";
        let trajectories = sample_trajectories();
        let result = mutate_prompt(prompt, &trajectories, MutationStrategy::RemoveIneffective);
        assert!(!result.trim().is_empty());
    }

    #[test]
    fn mutate_emphasize_adds_importance_markers() {
        let prompt = "Use the correct format\nSome context\nAlways check output";
        let result = mutate_prompt(prompt, &[], MutationStrategy::Emphasize);
        assert!(result.contains("IMPORTANT: Use the correct format"));
        assert!(result.contains("IMPORTANT: Always check output"));
        // Non-instruction lines should not be emphasized
        assert!(result.contains("Some context"));
        assert!(!result.contains("IMPORTANT: Some context"));
    }

    #[test]
    fn auto_select_with_many_poor_trajectories() {
        let trajectories = vec![
            TrajectoryHint { request_content: "a".into(), quality_score: 0.2, feedback: "bad".into() },
            TrajectoryHint { request_content: "b".into(), quality_score: 0.3, feedback: "bad".into() },
            TrajectoryHint { request_content: "c".into(), quality_score: 0.4, feedback: "bad".into() },
        ];
        let strategy = auto_select_strategy(&trajectories);
        assert_eq!(strategy, MutationStrategy::RemoveIneffective);
    }

    #[test]
    fn auto_select_with_good_examples() {
        let trajectories = vec![
            TrajectoryHint { request_content: "a".into(), quality_score: 0.9, feedback: "good".into() },
            TrajectoryHint { request_content: "b".into(), quality_score: 0.85, feedback: "good".into() },
        ];
        let strategy = auto_select_strategy(&trajectories);
        assert_eq!(strategy, MutationStrategy::AddExamples);
    }

    #[test]
    fn auto_select_defaults_to_rephrase() {
        let trajectories = vec![
            TrajectoryHint { request_content: "a".into(), quality_score: 0.7, feedback: "ok".into() },
        ];
        let strategy = auto_select_strategy(&trajectories);
        assert_eq!(strategy, MutationStrategy::Rephrase);
    }

    #[test]
    fn auto_select_empty_trajectories() {
        let strategy = auto_select_strategy(&[]);
        assert_eq!(strategy, MutationStrategy::Rephrase);
    }

    #[test]
    fn mutate_prompt_roundtrip_all_strategies() {
        let prompt = "Use the search tool to find relevant docs\nEnsure accuracy\nSome context info";
        let trajectories = sample_trajectories();

        for strategy in [
            MutationStrategy::Rephrase,
            MutationStrategy::AddExamples,
            MutationStrategy::RemoveIneffective,
            MutationStrategy::Emphasize,
        ] {
            let result = mutate_prompt(prompt, &trajectories, strategy);
            assert!(
                !result.trim().is_empty(),
                "mutation {:?} produced empty result",
                strategy
            );
        }
    }

    #[test]
    fn add_examples_truncates_long_content() {
        let long_content = "x".repeat(300);
        let trajectories = vec![TrajectoryHint {
            request_content: long_content,
            quality_score: 0.9,
            feedback: "good".into(),
        }];
        let result = mutate_prompt("prompt", &trajectories, MutationStrategy::AddExamples);
        assert!(result.contains("..."));
        // Should not contain the full 300-char string
        assert!(result.len() < 500);
    }
}
