//! Voice command shortcuts.
//!
//! Maps spoken trigger phrases to direct tool invocations, bypassing
//! the full LLM pipeline for common commands. The
//! [`VoiceCommandRegistry`] performs exact prefix matching and
//! Levenshtein-based fuzzy matching.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A voice command shortcut that maps a spoken phrase to a tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceCommand {
    /// Trigger phrases (any of these activates the command).
    /// Matched after STT, case-insensitive, with fuzzy tolerance.
    pub triggers: Vec<String>,
    /// Tool name to invoke.
    pub tool: String,
    /// Static parameters to pass to the tool.
    #[serde(default)]
    pub params: serde_json::Value,
    /// Whether this command requires voice confirmation before executing.
    #[serde(default)]
    pub confirm: bool,
    /// Human-readable description (for help listing).
    pub description: String,
}

/// Registry of voice command shortcuts.
///
/// Supports exact prefix matching and Levenshtein fuzzy matching
/// (edit distance <= 2) on the trigger phrase portion of transcriptions.
pub struct VoiceCommandRegistry {
    commands: Vec<VoiceCommand>,
    /// Precomputed lowercase triggers for fast matching.
    trigger_index: HashMap<String, usize>,
}

impl VoiceCommandRegistry {
    /// Build a registry from a list of voice commands.
    pub fn new(commands: Vec<VoiceCommand>) -> Self {
        let mut trigger_index = HashMap::new();
        for (idx, cmd) in commands.iter().enumerate() {
            for trigger in &cmd.triggers {
                trigger_index.insert(trigger.to_lowercase(), idx);
            }
        }
        Self {
            commands,
            trigger_index,
        }
    }

    /// Match a transcribed phrase against registered commands.
    ///
    /// Returns the matched command if the transcription starts with
    /// or closely matches a registered trigger phrase (Levenshtein
    /// distance <= 2).
    pub fn match_command(&self, transcription: &str) -> Option<&VoiceCommand> {
        let lower = transcription.to_lowercase();
        let lower = lower.trim();

        // Pass 1: exact prefix match
        for (trigger, idx) in &self.trigger_index {
            if lower.starts_with(trigger.as_str()) {
                return Some(&self.commands[*idx]);
            }
        }

        // Pass 2: fuzzy match on the first N words (Levenshtein <= 2)
        let words: Vec<&str> = lower.split_whitespace().collect();
        for (trigger, idx) in &self.trigger_index {
            let trigger_words: Vec<&str> = trigger.split_whitespace().collect();
            if words.len() >= trigger_words.len() {
                let spoken = words[..trigger_words.len()].join(" ");
                if levenshtein_distance(&spoken, trigger) <= 2 {
                    return Some(&self.commands[*idx]);
                }
            }
        }

        None
    }

    /// List all registered commands.
    pub fn list(&self) -> &[VoiceCommand] {
        &self.commands
    }

    /// Build a registry with the default built-in commands.
    pub fn with_defaults() -> Self {
        let commands = vec![
            VoiceCommand {
                triggers: vec!["stop listening".into(), "stop voice".into()],
                tool: "voice_stop".into(),
                params: serde_json::json!({}),
                confirm: false,
                description: "Stop the voice listening session.".into(),
            },
            VoiceCommand {
                triggers: vec!["what time is it".into(), "current time".into()],
                tool: "system_info".into(),
                params: serde_json::json!({"query": "time"}),
                confirm: false,
                description: "Show the current time.".into(),
            },
            VoiceCommand {
                triggers: vec!["list files".into(), "show files".into()],
                tool: "list_directory".into(),
                params: serde_json::json!({"path": "."}),
                confirm: false,
                description: "List files in the current directory.".into(),
            },
        ];
        Self::new(commands)
    }
}

/// Simple Levenshtein distance (edit distance) between two strings.
///
/// Standard dynamic programming implementation. Used for fuzzy matching
/// of voice commands where minor transcription errors are expected.
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();

    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for (i, row) in dp.iter_mut().enumerate().take(m + 1) {
        row[0] = i;
    }
    for j in 0..=n {
        dp[0][j] = j;
    }

    for i in 1..=m {
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }

    dp[m][n]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry() -> VoiceCommandRegistry {
        VoiceCommandRegistry::with_defaults()
    }

    #[test]
    fn exact_match_stop_listening() {
        let registry = test_registry();
        let cmd = registry.match_command("stop listening").unwrap();
        assert_eq!(cmd.tool, "voice_stop");
    }

    #[test]
    fn exact_match_what_time() {
        let registry = test_registry();
        let cmd = registry.match_command("what time is it").unwrap();
        assert_eq!(cmd.tool, "system_info");
    }

    #[test]
    fn exact_match_list_files_with_suffix() {
        let registry = test_registry();
        let cmd = registry
            .match_command("list files in the current directory")
            .unwrap();
        assert_eq!(cmd.tool, "list_directory");
    }

    #[test]
    fn exact_match_case_insensitive() {
        let registry = test_registry();
        let cmd = registry.match_command("Stop Listening").unwrap();
        assert_eq!(cmd.tool, "voice_stop");
    }

    #[test]
    fn fuzzy_match_within_distance_2() {
        let registry = test_registry();
        // "stopp listening" has distance 1 from "stop listening"
        let cmd = registry.match_command("stopp listening");
        assert!(cmd.is_some());
        assert_eq!(cmd.unwrap().tool, "voice_stop");
    }

    #[test]
    fn no_match_for_unrelated_phrase() {
        let registry = test_registry();
        let cmd = registry.match_command("tell me a joke");
        assert!(cmd.is_none());
    }

    #[test]
    fn levenshtein_identical_strings() {
        assert_eq!(levenshtein_distance("hello", "hello"), 0);
    }

    #[test]
    fn levenshtein_one_insertion() {
        assert_eq!(levenshtein_distance("hell", "hello"), 1);
    }

    #[test]
    fn levenshtein_one_deletion() {
        assert_eq!(levenshtein_distance("hello", "helo"), 1);
    }

    #[test]
    fn levenshtein_one_substitution() {
        assert_eq!(levenshtein_distance("hello", "hallo"), 1);
    }

    #[test]
    fn levenshtein_completely_different() {
        assert_eq!(levenshtein_distance("abc", "xyz"), 3);
    }

    #[test]
    fn levenshtein_empty_strings() {
        assert_eq!(levenshtein_distance("", ""), 0);
        assert_eq!(levenshtein_distance("abc", ""), 3);
        assert_eq!(levenshtein_distance("", "abc"), 3);
    }

    #[test]
    fn voice_command_serde_roundtrip() {
        let cmd = VoiceCommand {
            triggers: vec!["test".into()],
            tool: "test_tool".into(),
            params: serde_json::json!({"key": "value"}),
            confirm: true,
            description: "A test command".into(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let restored: VoiceCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.triggers, vec!["test"]);
        assert_eq!(restored.tool, "test_tool");
        assert!(restored.confirm);
    }

    #[test]
    fn registry_list_returns_all_commands() {
        let registry = test_registry();
        assert_eq!(registry.list().len(), 3);
    }

    #[test]
    fn custom_commands_registry() {
        let commands = vec![VoiceCommand {
            triggers: vec!["deploy now".into(), "ship it".into()],
            tool: "deploy".into(),
            params: serde_json::json!({"env": "production"}),
            confirm: true,
            description: "Deploy to production.".into(),
        }];
        let registry = VoiceCommandRegistry::new(commands);
        let cmd = registry.match_command("ship it").unwrap();
        assert_eq!(cmd.tool, "deploy");
        assert!(cmd.confirm);
    }
}
