//! Per-skill allowed-tools intersection validator (WEFT-65).
//!
//! Skills may declare `allowed_tools: [...]` in their manifest. At load
//! time, the loader intersects this set with the user/skill grant matrix
//! (the tools the operator has actually authorized for the skill). Any
//! declared tool not present in the grant set causes
//! [`SkillLoadError::ToolNotGranted`] and the skill is refused.
//!
//! The validator is a small, dependency-free helper so it can be invoked
//! from `clawft-core` (skill loader), the CLI (preflight check before
//! install), or third-party tooling without dragging in the full
//! security stack.

use std::collections::BTreeSet;

use crate::error::SkillLoadError;

/// Validate that every tool in `declared` is also present in `granted`.
///
/// Returns `Ok(())` on success; on failure returns
/// [`SkillLoadError::ToolNotGranted`] populated with the sorted, dedup'd
/// list of offending tools.
///
/// # Arguments
///
/// * `skill` - Skill name (used in the error for diagnostics).
/// * `declared` - Tools the skill asked for (`SkillDefinition.allowed_tools`).
/// * `granted` - Tools the user/skill grant matrix permits for this skill.
///
/// # Notes
///
/// - Comparison is case-sensitive. Callers responsible for normalization
///   if needed (e.g. lowercasing tool IDs).
/// - An empty `declared` always succeeds, regardless of `granted`.
/// - An empty `granted` rejects any non-empty `declared`.
pub fn validate_allowed_tools(
    skill: &str,
    declared: &[String],
    granted: &[String],
) -> Result<(), SkillLoadError> {
    if declared.is_empty() {
        return Ok(());
    }
    let granted_set: BTreeSet<&str> = granted.iter().map(String::as_str).collect();

    let mut denied: BTreeSet<String> = BTreeSet::new();
    for tool in declared {
        if !granted_set.contains(tool.as_str()) {
            denied.insert(tool.clone());
        }
    }

    if denied.is_empty() {
        Ok(())
    } else {
        Err(SkillLoadError::ToolNotGranted {
            skill: skill.to_string(),
            denied: denied.into_iter().collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_declared_always_succeeds() {
        let r = validate_allowed_tools("skill_a", &[], &[]);
        assert!(r.is_ok());
        let r = validate_allowed_tools("skill_a", &[], &["Read".into()]);
        assert!(r.is_ok());
    }

    #[test]
    fn all_granted_succeeds() {
        let declared = vec!["Read".to_string(), "Edit".to_string()];
        let granted = vec![
            "Read".to_string(),
            "Edit".to_string(),
            "WebSearch".to_string(),
        ];
        let r = validate_allowed_tools("safe_skill", &declared, &granted);
        assert!(r.is_ok());
    }

    #[test]
    fn one_ungranted_fails() {
        let declared = vec!["Read".to_string(), "shell.exec".to_string()];
        let granted = vec!["Read".to_string()];
        let err = validate_allowed_tools("risky_skill", &declared, &granted)
            .expect_err("expected ToolNotGranted");
        match err {
            SkillLoadError::ToolNotGranted { skill, denied } => {
                assert_eq!(skill, "risky_skill");
                assert_eq!(denied, vec!["shell.exec"]);
            }
            other => panic!("expected ToolNotGranted, got {other:?}"),
        }
    }

    #[test]
    fn empty_granted_rejects_non_empty_declared() {
        let declared = vec!["Read".to_string()];
        let granted: Vec<String> = vec![];
        let err = validate_allowed_tools("locked_skill", &declared, &granted)
            .expect_err("expected ToolNotGranted");
        match err {
            SkillLoadError::ToolNotGranted { denied, .. } => {
                assert_eq!(denied, vec!["Read"]);
            }
            other => panic!("expected ToolNotGranted, got {other:?}"),
        }
    }

    #[test]
    fn denied_set_is_sorted_and_deduped() {
        // declared has duplicates and unsorted order.
        let declared = vec![
            "shell.exec".to_string(),
            "WebFetch".to_string(),
            "shell.exec".to_string(),
            "WebFetch".to_string(),
        ];
        let granted: Vec<String> = vec![];
        let err = validate_allowed_tools("multi_denied", &declared, &granted)
            .expect_err("expected ToolNotGranted");
        match err {
            SkillLoadError::ToolNotGranted { denied, .. } => {
                // BTreeSet sorts; "WebFetch" < "shell.exec" by lexicographic
                // (uppercase 'W' = 0x57 < lowercase 's' = 0x73).
                assert_eq!(denied, vec!["WebFetch", "shell.exec"]);
            }
            other => panic!("expected ToolNotGranted, got {other:?}"),
        }
    }

    #[test]
    fn case_sensitive_match() {
        // "read" != "Read"
        let declared = vec!["Read".to_string()];
        let granted = vec!["read".to_string()];
        let err = validate_allowed_tools("case_skill", &declared, &granted)
            .expect_err("expected ToolNotGranted (case-sensitive)");
        match err {
            SkillLoadError::ToolNotGranted { denied, .. } => {
                assert_eq!(denied, vec!["Read"]);
            }
            other => panic!("expected ToolNotGranted, got {other:?}"),
        }
    }

    #[test]
    fn shell_exec_negative_case() {
        // The spec'd negative test: skill declares shell.exec but grant
        // matrix doesn't include shell.
        let declared = vec!["shell.exec".to_string(), "Read".to_string()];
        let granted = vec!["Read".to_string(), "Edit".to_string()];
        let err = validate_allowed_tools("needs_shell", &declared, &granted)
            .expect_err("user did not grant shell -> reject load");
        if let SkillLoadError::ToolNotGranted { denied, .. } = err {
            assert!(denied.contains(&"shell.exec".to_string()));
        } else {
            panic!("expected ToolNotGranted");
        }
    }

    #[test]
    fn shell_exec_positive_case() {
        // User HAS granted shell access; skill should load.
        let declared = vec!["shell.exec".to_string(), "Read".to_string()];
        let granted = vec!["shell.exec".to_string(), "Read".to_string()];
        let r = validate_allowed_tools("needs_shell_ok", &declared, &granted);
        assert!(r.is_ok());
    }

    #[test]
    fn error_display_format() {
        let err = SkillLoadError::ToolNotGranted {
            skill: "demo".into(),
            denied: vec!["foo.bar".into()],
        };
        let msg = err.to_string();
        assert!(msg.contains("demo"));
        assert!(msg.contains("foo.bar"));
    }
}
