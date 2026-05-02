//! Post-write verification and hallucination detection.
//!
//! Free-tier LLMs can hallucinate tool results — claiming files were written
//! when they weren't. This module provides:
//!
//! - **Post-write verification**: checks filesystem after write/edit tool calls
//! - **Hallucination score tracking**: EMA-smoothed per-session score
//! - **Complexity boost**: inflates routing complexity to push hallucination-prone
//!   sessions into higher-tier models via the existing TieredRouter

use std::path::{Path, PathBuf};

use clawft_platform::fs::FileSystem;

/// EMA smoothing factor for hallucination score updates.
pub const HALLUCINATION_EMA_ALPHA: f32 = 0.3;

/// Maximum complexity boost applied from hallucination score.
pub const MAX_HALLUCINATION_BOOST: f32 = 0.5;

/// Session metadata key for the hallucination score.
pub const HALLUCINATION_SCORE_KEY: &str = "hallucination_score";

/// Tool names that claim to write or edit files.
const WRITE_TOOL_NAMES: &[&str] = &[
    "write_file",
    "edit_file",
    "create_file",
    "write",
    "edit",
    "save_file",
    "patch_file",
];

/// Result of verifying a single write claim.
#[derive(Debug, Clone)]
pub struct VerificationResult {
    /// The tool call ID.
    pub tool_call_id: String,
    /// The claimed file path.
    pub claimed_path: PathBuf,
    /// Whether the file exists on disk.
    pub verified: bool,
}

/// Check whether a tool call claims to have written a file and extract the path.
///
/// Returns `(is_write_claim, optional_path)`. The path is extracted from the
/// tool result JSON, looking for common patterns like `{"path": "..."}` or
/// `"Successfully wrote N bytes to path"`.
pub fn parse_write_claim(
    tool_name: &str,
    result_json: &str,
) -> (bool, Option<String>) {
    let name_lower = tool_name.to_lowercase();
    let is_write = WRITE_TOOL_NAMES.iter().any(|w| name_lower.contains(w));

    if !is_write {
        return (false, None);
    }

    // Try to extract path from JSON result.
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(result_json) {
        // Check common JSON fields: "path", "file_path", "file"
        for key in &["path", "file_path", "file", "filename"] {
            if let Some(p) = val.get(key).and_then(|v| v.as_str())
                && !p.is_empty()
            {
                return (true, Some(p.to_string()));
            }
        }

        // Check nested: {"result": {"path": "..."}}
        if let Some(inner) = val.get("result") {
            for key in &["path", "file_path", "file"] {
                if let Some(p) = inner.get(key).and_then(|v| v.as_str())
                    && !p.is_empty()
                {
                    return (true, Some(p.to_string()));
                }
            }
        }
    }

    // Try to extract path from "Successfully wrote N bytes to <path>" pattern.
    if let Some(idx) = result_json.find(" to ") {
        let after = &result_json[idx + 4..];
        // Take until end of string or next quote/whitespace
        let path_str = after
            .trim_matches(|c: char| c == '"' || c == '\'' || c == '`')
            .split(['"', '\n'])
            .next()
            .unwrap_or("")
            .trim();
        if !path_str.is_empty() && path_str.len() < 512 {
            return (true, Some(path_str.to_string()));
        }
    }

    // It's a write tool but we couldn't extract the path.
    (true, None)
}

/// Verify write results by checking filesystem existence.
///
/// For each tool result that claims a write succeeded, checks whether the
/// file actually exists at `workspace / claimed_path`. Returns a
/// [`VerificationResult`] for each write claim found.
pub async fn verify_write_results(
    fs: &dyn FileSystem,
    workspace: &Path,
    tool_results: &[(String, String, String)], // (id, name, result_json)
) -> Vec<VerificationResult> {
    let mut results = Vec::new();

    for (id, name, result_json) in tool_results {
        let (is_write, maybe_path) = parse_write_claim(name, result_json);
        if !is_write {
            continue;
        }

        if let Some(ref path_str) = maybe_path {
            let full_path = if Path::new(path_str).is_absolute() {
                PathBuf::from(path_str)
            } else {
                workspace.join(path_str)
            };

            let verified = fs.exists(&full_path).await;
            results.push(VerificationResult {
                tool_call_id: id.clone(),
                claimed_path: full_path,
                verified,
            });
        }
        // If we couldn't extract a path, skip verification for this call.
    }

    results
}

/// Update the hallucination score using exponential moving average.
///
/// The score represents the proportion of recent write claims that were
/// hallucinated. Higher = more hallucinations.
///
/// - `current`: the current EMA score (0.0 if first time)
/// - `hallucinations`: count of hallucinated writes in this batch
/// - `successes`: count of verified writes in this batch
/// - `alpha`: EMA smoothing factor (higher = more weight on new data)
///
/// Returns the updated score in [0.0, 1.0].
pub fn update_hallucination_score(
    current: f32,
    hallucinations: usize,
    successes: usize,
    alpha: f32,
) -> f32 {
    let total = hallucinations + successes;
    if total == 0 {
        return current;
    }

    let batch_rate = hallucinations as f32 / total as f32;
    let new_score = alpha * batch_rate + (1.0 - alpha) * current;
    new_score.clamp(0.0, 1.0)
}

/// Convert a hallucination score to a complexity boost value.
///
/// Linear mapping: `score * 0.5`, capped at [`MAX_HALLUCINATION_BOOST`].
pub fn score_to_boost(score: f32) -> f32 {
    (score * MAX_HALLUCINATION_BOOST).min(MAX_HALLUCINATION_BOOST)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_write_claim tests ──────────────────────────────────────

    #[test]
    fn parse_write_claim_detects_write_file() {
        let (is_write, path) = parse_write_claim(
            "write_file",
            r#"{"path": "src/main.rs"}"#,
        );
        assert!(is_write);
        assert_eq!(path.as_deref(), Some("src/main.rs"));
    }

    #[test]
    fn parse_write_claim_detects_edit_file() {
        let (is_write, path) = parse_write_claim(
            "edit_file",
            r#"{"file_path": "lib.rs"}"#,
        );
        assert!(is_write);
        assert_eq!(path.as_deref(), Some("lib.rs"));
    }

    #[test]
    fn parse_write_claim_extracts_from_text() {
        let (is_write, path) = parse_write_claim(
            "write_file",
            r#""Successfully wrote 42 bytes to src/app.rs""#,
        );
        assert!(is_write);
        assert_eq!(path.as_deref(), Some("src/app.rs"));
    }

    #[test]
    fn parse_write_claim_ignores_non_write_tools() {
        let (is_write, _) = parse_write_claim(
            "web_search",
            r#"{"path": "something"}"#,
        );
        assert!(!is_write);
    }

    #[test]
    fn parse_write_claim_write_tool_no_path() {
        let (is_write, path) = parse_write_claim(
            "write_file",
            r#"{"status": "ok"}"#,
        );
        assert!(is_write);
        assert!(path.is_none());
    }

    // ── update_hallucination_score tests ─────────────────────────────

    #[test]
    fn ema_first_hallucination_from_zero() {
        let score = update_hallucination_score(0.0, 1, 0, HALLUCINATION_EMA_ALPHA);
        assert!((score - 0.3).abs() < f32::EPSILON, "score={score}");
    }

    #[test]
    fn ema_second_hallucination() {
        let s1 = update_hallucination_score(0.0, 1, 0, HALLUCINATION_EMA_ALPHA);
        let s2 = update_hallucination_score(s1, 1, 0, HALLUCINATION_EMA_ALPHA);
        // s2 = 0.3 * 1.0 + 0.7 * 0.3 = 0.3 + 0.21 = 0.51
        assert!((s2 - 0.51).abs() < 0.001, "score={s2}");
    }

    #[test]
    fn ema_success_decays_score() {
        let s1 = update_hallucination_score(0.5, 0, 1, HALLUCINATION_EMA_ALPHA);
        // s1 = 0.3 * 0.0 + 0.7 * 0.5 = 0.35
        assert!((s1 - 0.35).abs() < 0.001, "score={s1}");
    }

    #[test]
    fn ema_no_verifications_returns_current() {
        let score = update_hallucination_score(0.42, 0, 0, HALLUCINATION_EMA_ALPHA);
        assert!((score - 0.42).abs() < f32::EPSILON);
    }

    #[test]
    fn ema_clamped_to_unit_range() {
        let score = update_hallucination_score(1.0, 1, 0, HALLUCINATION_EMA_ALPHA);
        assert!((0.0..=1.0).contains(&score), "score={score}");
    }

    // ── score_to_boost tests ────────────────────────────────────────

    #[test]
    fn boost_zero_for_zero_score() {
        assert!((score_to_boost(0.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn boost_proportional_to_score() {
        let boost = score_to_boost(0.6);
        assert!((boost - 0.3).abs() < 0.001, "boost={boost}");
    }

    #[test]
    fn boost_capped_at_max() {
        let boost = score_to_boost(1.0);
        assert!(
            (boost - MAX_HALLUCINATION_BOOST).abs() < f32::EPSILON,
            "boost={boost}"
        );
    }

    #[test]
    fn boost_capped_above_one() {
        // Even if score somehow exceeded 1.0, boost should cap at max
        let boost = score_to_boost(2.0);
        assert!(
            boost <= MAX_HALLUCINATION_BOOST,
            "boost={boost} should be <= {}",
            MAX_HALLUCINATION_BOOST
        );
    }

    // ── verify_write_results tests ──────────────────────────────────

    #[tokio::test]
    async fn verify_existing_file() {
        // Use a temp file that actually exists
        let dir = std::env::temp_dir().join("clawft_verify_test");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let file = dir.join("exists.txt");
        tokio::fs::write(&file, "hello").await.unwrap();

        let native_fs = clawft_platform::fs::NativeFileSystem;
        let results = verify_write_results(
            &native_fs,
            &dir,
            &[(
                "call-1".into(),
                "write_file".into(),
                r#"{"path": "exists.txt"}"#.to_string(),
            )],
        )
        .await;

        assert_eq!(results.len(), 1);
        assert!(results[0].verified, "existing file should verify");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn verify_missing_file_detects_hallucination() {
        let dir = std::env::temp_dir().join("clawft_verify_miss");
        let _ = tokio::fs::create_dir_all(&dir).await;

        let native_fs = clawft_platform::fs::NativeFileSystem;
        let results = verify_write_results(
            &native_fs,
            &dir,
            &[(
                "call-2".into(),
                "write_file".into(),
                r#"{"path": "ghost.txt"}"#.into(),
            )],
        )
        .await;

        assert_eq!(results.len(), 1);
        assert!(
            !results[0].verified,
            "missing file should NOT verify (hallucination)"
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn verify_skips_non_write_tools() {
        let native_fs = clawft_platform::fs::NativeFileSystem;
        let results = verify_write_results(
            &native_fs,
            Path::new("/tmp"),
            &[(
                "call-3".into(),
                "web_search".into(),
                r#"{"path": "anything"}"#.into(),
            )],
        )
        .await;

        assert!(results.is_empty(), "non-write tools should be skipped");
    }
}
