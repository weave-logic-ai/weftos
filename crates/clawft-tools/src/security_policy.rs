//! Command execution security policy.
//!
//! Provides configurable allowlist/denylist-based command validation with
//! defense-in-depth dangerous pattern detection. Used by shell and spawn
//! tools to gate which executables may be invoked.
//!
//! The core types ([`CommandPolicy`], [`PolicyMode`]) are defined in
//! [`clawft_types::security`] and re-exported here for convenience.
//! This module adds the [`PolicyError`] type used by tool implementations
//! and the [`extract_first_token`] helper.

use thiserror::Error;

// Re-export the canonical types from clawft-types.
pub use clawft_types::security::{
    CommandPolicy, CommandPolicyError, DEFAULT_COMMAND_ALLOWLIST, DEFAULT_DANGEROUS_PATTERNS,
    PolicyMode, extract_first_token,
};

/// Errors returned when a command fails policy validation.
///
/// This wraps [`CommandPolicyError`] with `thiserror` for ergonomic
/// use in tool implementations that return `Result<_, PolicyError>`.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum PolicyError {
    /// The command's executable is not on the allowlist.
    #[error("command not allowed: {command}")]
    NotAllowed { command: String },

    /// The command matched a denylist pattern.
    #[error("command blocked: {command} (matched pattern: {pattern})")]
    Blocked { command: String, pattern: String },

    /// The command matched a dangerous pattern (defense-in-depth check).
    #[error("dangerous command: {command} (matched pattern: {pattern})")]
    DangerousPattern { command: String, pattern: String },
}

impl From<CommandPolicyError> for PolicyError {
    fn from(err: CommandPolicyError) -> Self {
        match err {
            CommandPolicyError::NotAllowed { command } => PolicyError::NotAllowed { command },
            CommandPolicyError::Blocked { command, pattern } => {
                PolicyError::Blocked { command, pattern }
            }
            CommandPolicyError::DangerousPattern { command, pattern } => {
                PolicyError::DangerousPattern { command, pattern }
            }
            _ => PolicyError::NotAllowed {
                command: format!("{err}"),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- safe_defaults ---------------------------------------------------------

    #[test]
    fn safe_defaults_creates_correct_mode() {
        let policy = CommandPolicy::safe_defaults();
        assert_eq!(policy.mode, PolicyMode::Allowlist);
    }

    #[test]
    fn safe_defaults_has_expected_allowlist() {
        let policy = CommandPolicy::safe_defaults();
        for cmd in DEFAULT_COMMAND_ALLOWLIST {
            assert!(
                policy.allowlist.contains(*cmd),
                "{cmd} should be in allowlist"
            );
        }
    }

    #[test]
    fn safe_defaults_has_dangerous_patterns() {
        let policy = CommandPolicy::safe_defaults();
        assert_eq!(policy.dangerous_patterns.len(), 11);
    }

    #[test]
    fn safe_defaults_denylist_matches_dangerous() {
        let policy = CommandPolicy::safe_defaults();
        assert_eq!(policy.denylist, policy.dangerous_patterns);
    }

    // -- allowlist mode --------------------------------------------------------

    #[test]
    fn allowlist_permits_echo() {
        let policy = CommandPolicy::safe_defaults();
        assert!(policy.validate("echo hello").is_ok());
    }

    #[test]
    fn allowlist_permits_ls() {
        let policy = CommandPolicy::safe_defaults();
        assert!(policy.validate("ls -la").is_ok());
    }

    #[test]
    fn allowlist_permits_cat() {
        let policy = CommandPolicy::safe_defaults();
        assert!(policy.validate("cat file.txt").is_ok());
    }

    #[test]
    fn allowlist_permits_pwd() {
        let policy = CommandPolicy::safe_defaults();
        assert!(policy.validate("pwd").is_ok());
    }

    #[test]
    fn allowlist_rejects_curl() {
        let policy = CommandPolicy::safe_defaults();
        let err = policy.validate("curl http://evil.com").unwrap_err();
        assert!(matches!(err, CommandPolicyError::NotAllowed { .. }));
    }

    #[test]
    fn allowlist_rejects_wget() {
        let policy = CommandPolicy::safe_defaults();
        let err = policy.validate("wget http://evil.com").unwrap_err();
        assert!(matches!(err, CommandPolicyError::NotAllowed { .. }));
    }

    #[test]
    fn allowlist_rejects_nmap() {
        let policy = CommandPolicy::safe_defaults();
        let err = policy.validate("nmap -sS 10.0.0.0/24").unwrap_err();
        assert!(matches!(err, CommandPolicyError::NotAllowed { .. }));
    }

    #[test]
    fn allowlist_rejects_nc() {
        let policy = CommandPolicy::safe_defaults();
        let err = policy.validate("nc -l 4444").unwrap_err();
        assert!(matches!(err, CommandPolicyError::NotAllowed { .. }));
    }

    #[test]
    fn allowlist_rejects_bash() {
        let policy = CommandPolicy::safe_defaults();
        let err = policy.validate("bash -c \"evil\"").unwrap_err();
        assert!(matches!(err, CommandPolicyError::NotAllowed { .. }));
    }

    // -- denylist mode ---------------------------------------------------------

    #[test]
    fn denylist_permits_curl_when_not_denied() {
        let mut policy = CommandPolicy::safe_defaults();
        policy.mode = PolicyMode::Denylist;
        assert!(policy.validate("curl http://safe.com").is_ok());
    }

    #[test]
    fn denylist_blocks_rm_rf_root() {
        let mut policy = CommandPolicy::safe_defaults();
        policy.mode = PolicyMode::Denylist;
        let err = policy.validate("rm -rf /").unwrap_err();
        // Dangerous patterns are checked first, so this will be DangerousPattern.
        assert!(matches!(err, CommandPolicyError::DangerousPattern { .. }));
    }

    #[test]
    fn denylist_blocks_sudo() {
        let mut policy = CommandPolicy::safe_defaults();
        policy.mode = PolicyMode::Denylist;
        let err = policy.validate("sudo something").unwrap_err();
        assert!(matches!(err, CommandPolicyError::DangerousPattern { .. }));
    }

    // -- extract_first_token ---------------------------------------------------

    #[test]
    fn extract_first_token_simple() {
        assert_eq!(extract_first_token("echo foo"), "echo");
    }

    #[test]
    fn extract_first_token_with_path() {
        assert_eq!(extract_first_token("/usr/bin/ls -la"), "ls");
    }

    #[test]
    fn extract_first_token_leading_whitespace() {
        assert_eq!(extract_first_token("  cat file"), "cat");
    }

    #[test]
    fn extract_first_token_empty() {
        assert_eq!(extract_first_token(""), "");
    }

    #[test]
    fn extract_first_token_whitespace_only() {
        assert_eq!(extract_first_token("   "), "");
    }

    // -- dangerous patterns always checked -------------------------------------

    #[test]
    fn dangerous_patterns_checked_in_allowlist_mode() {
        let policy = CommandPolicy::safe_defaults();
        // "echo" is on the allowlist but the command contains a dangerous pattern.
        let err = policy.validate("echo; rm -rf /").unwrap_err();
        assert!(matches!(err, CommandPolicyError::DangerousPattern { .. }));
    }

    #[test]
    fn dangerous_patterns_checked_in_denylist_mode() {
        let mut policy = CommandPolicy::safe_defaults();
        policy.mode = PolicyMode::Denylist;
        let err = policy.validate("dd if=/dev/zero of=/dev/sda").unwrap_err();
        assert!(matches!(err, CommandPolicyError::DangerousPattern { .. }));
    }

    // -- case insensitivity ----------------------------------------------------

    #[test]
    fn case_insensitive_sudo_blocked() {
        let policy = CommandPolicy::safe_defaults();
        let err = policy.validate("SUDO something").unwrap_err();
        assert!(matches!(err, CommandPolicyError::DangerousPattern { .. }));
    }

    #[test]
    fn case_insensitive_mixed_case_blocked() {
        let policy = CommandPolicy::safe_defaults();
        let err = policy.validate("SuDo apt install evil").unwrap_err();
        assert!(matches!(err, CommandPolicyError::DangerousPattern { .. }));
    }

    // -- path traversal / basename extraction in allowlist ----------------------

    #[test]
    fn allowlist_rejects_path_to_unlisted_binary() {
        let policy = CommandPolicy::safe_defaults();
        // /usr/bin/curl -> basename "curl", which is not on the allowlist.
        let err = policy
            .validate("/usr/bin/curl http://evil.com")
            .unwrap_err();
        assert!(matches!(err, CommandPolicyError::NotAllowed { .. }));
    }

    #[test]
    fn allowlist_permits_path_to_listed_binary() {
        let policy = CommandPolicy::safe_defaults();
        // /usr/bin/ls -> basename "ls", which IS on the allowlist.
        assert!(policy.validate("/usr/bin/ls -la").is_ok());
    }

    // -- tab-separated commands ------------------------------------------------

    #[test]
    fn tab_in_sudo_command_still_matched() {
        let policy = CommandPolicy::safe_defaults();
        // Whitespace is normalized before pattern matching, so "sudo\t"
        // becomes "sudo " which matches the "sudo " dangerous pattern.
        let result = policy.validate("sudo\tsomething");
        assert!(result.is_err(), "tab-separated sudo should be blocked");
    }

    // -- PolicyError conversion ------------------------------------------------

    #[test]
    fn policy_error_from_command_policy_error() {
        let err = CommandPolicyError::NotAllowed {
            command: "curl".into(),
        };
        let policy_err: PolicyError = err.into();
        assert!(matches!(policy_err, PolicyError::NotAllowed { .. }));
        assert_eq!(policy_err.to_string(), "command not allowed: curl");
    }
}
