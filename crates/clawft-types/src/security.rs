//! Runtime security policy types.
//!
//! Defines [`CommandPolicy`] and [`UrlPolicy`] -- the runtime
//! representations of command execution and URL safety policies.
//! These are constructed from the config-level [`CommandPolicyConfig`]
//! and [`UrlPolicyConfig`] at startup time.
//!
//! [`CommandPolicy::validate`] provides the standard command validation
//! logic (allowlist/denylist + dangerous pattern checks). Full URL/SSRF
//! validation lives in `clawft-tools::url_safety` which depends on
//! external crates (`url`, `ipnet`).

use std::collections::HashSet;

// ── Command Policy ──────────────────────────────────────────────────────

/// Whether the command policy operates in allowlist or denylist mode.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PolicyMode {
    /// Only commands whose basename appears in the allowlist are permitted.
    #[default]
    Allowlist,
    /// All commands are permitted unless they match a denylist pattern.
    Denylist,
}

/// Errors returned when a command fails policy validation.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandPolicyError {
    /// The command's executable is not on the allowlist.
    NotAllowed { command: String },
    /// The command matched a denylist pattern.
    Blocked { command: String, pattern: String },
    /// The command matched a dangerous pattern (defense-in-depth check).
    DangerousPattern { command: String, pattern: String },
}

impl std::fmt::Display for CommandPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAllowed { command } => write!(f, "command not allowed: {command}"),
            Self::Blocked { command, pattern } => {
                write!(f, "command blocked: {command} (matched pattern: {pattern})")
            }
            Self::DangerousPattern { command, pattern } => {
                write!(
                    f,
                    "dangerous command: {command} (matched pattern: {pattern})"
                )
            }
        }
    }
}

impl std::error::Error for CommandPolicyError {}

/// Configurable command execution policy (runtime representation).
///
/// Validates commands against an allowlist or denylist, and always checks
/// a set of dangerous patterns regardless of mode (defense-in-depth).
///
/// Constructed from [`super::config::CommandPolicyConfig`] at startup.
#[derive(Debug, Clone)]
pub struct CommandPolicy {
    /// Operating mode for the policy.
    pub mode: PolicyMode,
    /// Set of permitted executable basenames (used in `Allowlist` mode).
    pub allowlist: HashSet<String>,
    /// Patterns to block (substring match, case-insensitive; used in `Denylist` mode).
    pub denylist: Vec<String>,
    /// Patterns that are always checked regardless of mode (defense-in-depth).
    pub dangerous_patterns: Vec<String>,
}

/// The default set of safe executable basenames for allowlist mode.
///
/// Includes read-only utilities, common file operations (dangerous patterns
/// still block destructive variants like `rm -rf /`), shell builtins, and
/// development tools commonly needed by AI agent workflows.
pub const DEFAULT_COMMAND_ALLOWLIST: &[&str] = &[
    // Read-only / informational
    "echo",
    "cat",
    "ls",
    "pwd",
    "head",
    "tail",
    "wc",
    "grep",
    "find",
    "sort",
    "uniq",
    "diff",
    "date",
    "env",
    "true",
    "false",
    "test",
    "which",
    "basename",
    "dirname",
    "stat",
    "file",
    "readlink",
    // Text processing
    "sed",
    "awk",
    "cut",
    "tr",
    "tee",
    "xargs",
    // File operations (dangerous patterns still block e.g. rm -rf /)
    "mkdir",
    "cp",
    "mv",
    "touch",
    "rm",
    "ln",
    "chmod",
    // Shell builtins
    "cd",
    "export",
    "source",
    "type",
    "command",
    // Development tools
    "git",
    "cargo",
    "rustc",
    "npm",
    "npx",
    "node",
    "python",
    "python3",
    // ClawFT ecosystem
    "weft",
    "claude-flow",
];

/// The default set of dangerous patterns.
pub const DEFAULT_DANGEROUS_PATTERNS: &[&str] = &[
    "rm -rf /",
    "sudo ",
    "mkfs",
    "dd if=",
    ":(){ :|:& };:",
    "chmod 777 /",
    "> /dev/sd",
    "shutdown",
    "reboot",
    "poweroff",
    "format c:",
];

impl Default for CommandPolicy {
    fn default() -> Self {
        Self::safe_defaults()
    }
}

impl CommandPolicy {
    /// Create a policy with safe defaults.
    ///
    /// - Mode: `Allowlist`
    /// - Allowlist: common read-only / informational commands
    /// - Dangerous patterns: the standard set from `DEFAULT_DANGEROUS_PATTERNS`
    /// - Denylist: same patterns (used when mode is switched to `Denylist`)
    pub fn safe_defaults() -> Self {
        let allowlist = DEFAULT_COMMAND_ALLOWLIST
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let dangerous_patterns: Vec<String> = DEFAULT_DANGEROUS_PATTERNS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let denylist = dangerous_patterns.clone();

        Self {
            mode: PolicyMode::Allowlist,
            allowlist,
            denylist,
            dangerous_patterns,
        }
    }

    /// Create a new policy with explicit configuration.
    pub fn new(mode: PolicyMode, allowlist: HashSet<String>, denylist: Vec<String>) -> Self {
        let dangerous_patterns: Vec<String> = DEFAULT_DANGEROUS_PATTERNS
            .iter()
            .map(|s| (*s).to_string())
            .collect();

        Self {
            mode,
            allowlist,
            denylist,
            dangerous_patterns,
        }
    }

    /// Validate a command string against this policy.
    ///
    /// 1. Always checks dangerous patterns first (defense-in-depth).
    /// 2. In `Allowlist` mode, splits on shell compound operators (`&&`,
    ///    `||`, `;`, `|`) and validates every sub-command's basename.
    /// 3. In `Denylist` mode, checks all denylist patterns (case-insensitive
    ///    substring match).
    pub fn validate(&self, command: &str) -> Result<(), CommandPolicyError> {
        // Normalize whitespace (tabs, etc.) to spaces for pattern matching,
        // so that "sudo\tsomething" matches the "sudo " pattern.
        let normalized: String = command
            .chars()
            .map(|c| if c.is_whitespace() { ' ' } else { c })
            .collect();
        let lower = normalized.to_lowercase();

        // Step 1: Always check dangerous patterns (defense-in-depth).
        for pattern in &self.dangerous_patterns {
            if lower.contains(&pattern.to_lowercase()) {
                return Err(CommandPolicyError::DangerousPattern {
                    command: command.to_string(),
                    pattern: pattern.clone(),
                });
            }
        }

        // Step 2: Mode-specific checks.
        match self.mode {
            PolicyMode::Allowlist => {
                // Validate every sub-command in compound expressions
                // (e.g. "cd foo && claude-flow mcp status" checks both
                // "cd" and "claude-flow").
                for sub in split_shell_commands(command) {
                    let token = extract_first_token(sub);
                    if !self.allowlist.contains(token) {
                        return Err(CommandPolicyError::NotAllowed {
                            command: command.to_string(),
                        });
                    }
                }
            }
            PolicyMode::Denylist => {
                for pattern in &self.denylist {
                    if lower.contains(&pattern.to_lowercase()) {
                        return Err(CommandPolicyError::Blocked {
                            command: command.to_string(),
                            pattern: pattern.clone(),
                        });
                    }
                }
            }
        }

        Ok(())
    }
}

/// Split a command string on shell compound operators (`&&`, `||`, `;`, `|`).
///
/// Returns each sub-command as a trimmed slice. Two-character operators
/// (`&&`, `||`) are matched before single-character ones (`|`, `;`) so
/// that `||` is not mis-parsed as two pipes.
///
/// Note: this does **not** handle quoting; operators inside quoted strings
/// will cause harmless extra validation (safe direction: more checks, not
/// fewer).
pub fn split_shell_commands(command: &str) -> Vec<&str> {
    let bytes = command.as_bytes();
    let len = bytes.len();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;

    while i < len {
        // Two-character operators first (&&, ||).
        if i + 1 < len {
            let pair = [bytes[i], bytes[i + 1]];
            if pair == *b"&&" || pair == *b"||" {
                let part = command[start..i].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                i += 2;
                start = i;
                continue;
            }
        }
        // Single-character operators (; |).
        if bytes[i] == b';' || bytes[i] == b'|' {
            let part = command[start..i].trim();
            if !part.is_empty() {
                parts.push(part);
            }
            i += 1;
            start = i;
            continue;
        }
        i += 1;
    }

    // Remainder after last operator.
    let part = command[start..].trim();
    if !part.is_empty() {
        parts.push(part);
    }

    parts
}

/// Extract the first whitespace-delimited token from a command string,
/// stripping any leading path components (basename extraction).
///
/// # Examples
///
/// ```text
/// "echo foo"        -> "echo"
/// "/usr/bin/ls -la"  -> "ls"
/// "  cat file"      -> "cat"
/// ""                -> ""
/// ```
pub fn extract_first_token(command: &str) -> &str {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return "";
    }

    let token = trimmed.split_whitespace().next().unwrap_or("");

    // Strip path prefix: take everything after the last '/'.
    match token.rfind('/') {
        Some(pos) => &token[pos + 1..],
        None => token,
    }
}

// ── URL Policy ──────────────────────────────────────────────────────────

/// Runtime URL safety policy for SSRF protection.
///
/// Controls which URLs the web fetch tool is allowed to access.
/// Constructed from [`super::config::UrlPolicyConfig`] at startup.
///
/// Note: the full URL validation logic (DNS resolution, CIDR checks) lives
/// in `clawft-tools::url_safety::validate_url` which depends on external
/// crates. This struct is the shared data container.
#[derive(Debug, Clone)]
pub struct UrlPolicy {
    /// Whether URL safety checks are active.
    pub enabled: bool,
    /// Whether to allow requests to private/reserved IP ranges.
    pub allow_private: bool,
    /// Domains that bypass all safety checks.
    pub allowed_domains: HashSet<String>,
    /// Domains that are always blocked.
    pub blocked_domains: HashSet<String>,
}

impl Default for UrlPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_private: false,
            allowed_domains: HashSet::new(),
            blocked_domains: HashSet::new(),
        }
    }
}

impl UrlPolicy {
    /// Create a new policy with the given settings.
    pub fn new(
        enabled: bool,
        allow_private: bool,
        allowed_domains: HashSet<String>,
        blocked_domains: HashSet<String>,
    ) -> Self {
        Self {
            enabled,
            allow_private,
            allowed_domains,
            blocked_domains,
        }
    }

    /// Create a permissive policy that disables all checks.
    ///
    /// Intended for testing and development only.
    pub fn permissive() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // -- CommandPolicy construction --

    #[test]
    fn command_policy_safe_defaults() {
        let policy = CommandPolicy::safe_defaults();
        assert_eq!(policy.mode, PolicyMode::Allowlist);
        assert!(policy.allowlist.contains("echo"));
        assert!(policy.allowlist.contains("ls"));
        assert!(!policy.dangerous_patterns.is_empty());
    }

    #[test]
    fn command_policy_new() {
        let allowlist = HashSet::from(["curl".to_string()]);
        let denylist = vec!["rm".to_string()];
        let policy = CommandPolicy::new(PolicyMode::Denylist, allowlist, denylist);
        assert_eq!(policy.mode, PolicyMode::Denylist);
        assert!(policy.allowlist.contains("curl"));
        assert_eq!(policy.denylist, vec!["rm".to_string()]);
    }

    // -- CommandPolicy validation --

    #[test]
    fn allowlist_permits_echo() {
        let policy = CommandPolicy::safe_defaults();
        assert!(policy.validate("echo hello").is_ok());
    }

    #[test]
    fn allowlist_rejects_curl() {
        let policy = CommandPolicy::safe_defaults();
        let err = policy.validate("curl http://evil.com").unwrap_err();
        assert!(matches!(err, CommandPolicyError::NotAllowed { .. }));
    }

    #[test]
    fn dangerous_patterns_always_checked() {
        let policy = CommandPolicy::safe_defaults();
        let err = policy.validate("echo; rm -rf /").unwrap_err();
        assert!(matches!(err, CommandPolicyError::DangerousPattern { .. }));
    }

    #[test]
    fn denylist_mode_permits_unlisted() {
        let mut policy = CommandPolicy::safe_defaults();
        policy.mode = PolicyMode::Denylist;
        assert!(policy.validate("curl http://safe.com").is_ok());
    }

    #[test]
    fn tab_normalized_to_space() {
        let policy = CommandPolicy::safe_defaults();
        let result = policy.validate("sudo\tsomething");
        assert!(result.is_err());
    }

    // -- extract_first_token --

    #[test]
    fn extract_token_simple() {
        assert_eq!(extract_first_token("echo foo"), "echo");
    }

    #[test]
    fn extract_token_with_path() {
        assert_eq!(extract_first_token("/usr/bin/ls -la"), "ls");
    }

    #[test]
    fn extract_token_empty() {
        assert_eq!(extract_first_token(""), "");
    }

    // -- UrlPolicy --

    #[test]
    fn url_policy_default() {
        let policy = UrlPolicy::default();
        assert!(policy.enabled);
        assert!(!policy.allow_private);
        assert!(policy.allowed_domains.is_empty());
        assert!(policy.blocked_domains.is_empty());
    }

    #[test]
    fn url_policy_permissive() {
        let policy = UrlPolicy::permissive();
        assert!(!policy.enabled);
    }

    #[test]
    fn url_policy_new() {
        let allowed = HashSet::from(["internal.corp".to_string()]);
        let blocked = HashSet::from(["evil.com".to_string()]);
        let policy = UrlPolicy::new(true, true, allowed, blocked);
        assert!(policy.enabled);
        assert!(policy.allow_private);
        assert!(policy.allowed_domains.contains("internal.corp"));
        assert!(policy.blocked_domains.contains("evil.com"));
    }

    #[test]
    fn policy_mode_default_is_allowlist() {
        assert_eq!(PolicyMode::default(), PolicyMode::Allowlist);
    }

    #[test]
    fn command_policy_error_display() {
        let err = CommandPolicyError::NotAllowed {
            command: "curl".into(),
        };
        assert_eq!(err.to_string(), "command not allowed: curl");
    }

    // -- split_shell_commands --

    #[test]
    fn split_simple_command() {
        assert_eq!(split_shell_commands("echo hello"), vec!["echo hello"]);
    }

    #[test]
    fn split_and_operator() {
        assert_eq!(
            split_shell_commands("cd foo && claude-flow mcp status"),
            vec!["cd foo", "claude-flow mcp status"]
        );
    }

    #[test]
    fn split_or_operator() {
        assert_eq!(
            split_shell_commands("ls /tmp || echo fallback"),
            vec!["ls /tmp", "echo fallback"]
        );
    }

    #[test]
    fn split_semicolon() {
        assert_eq!(
            split_shell_commands("echo a; echo b"),
            vec!["echo a", "echo b"]
        );
    }

    #[test]
    fn split_pipe() {
        assert_eq!(
            split_shell_commands("cat file | grep pattern"),
            vec!["cat file", "grep pattern"]
        );
    }

    #[test]
    fn split_mixed_operators() {
        assert_eq!(
            split_shell_commands("cd dir && git status | grep modified; echo done"),
            vec!["cd dir", "git status", "grep modified", "echo done"]
        );
    }

    #[test]
    fn split_empty() {
        let result: Vec<&str> = split_shell_commands("");
        assert!(result.is_empty());
    }

    // -- compound command validation --

    #[test]
    fn allowlist_permits_compound_when_all_allowed() {
        let policy = CommandPolicy::safe_defaults();
        // Both `cd` and `claude-flow` are now in the default allowlist.
        assert!(
            policy
                .validate("cd clawft && claude-flow mcp status")
                .is_ok()
        );
    }

    #[test]
    fn allowlist_rejects_compound_when_any_disallowed() {
        let policy = CommandPolicy::safe_defaults();
        // `curl` is still not on the allowlist.
        let err = policy
            .validate("echo hi && curl http://evil.com")
            .unwrap_err();
        assert!(matches!(err, CommandPolicyError::NotAllowed { .. }));
    }

    #[test]
    fn allowlist_permits_pipe_chain() {
        let policy = CommandPolicy::safe_defaults();
        assert!(policy.validate("cat file | grep pattern | sort").is_ok());
    }

    #[test]
    fn allowlist_permits_dev_tools() {
        let policy = CommandPolicy::safe_defaults();
        assert!(policy.validate("git status").is_ok());
        assert!(policy.validate("cargo build").is_ok());
        assert!(policy.validate("npx @claude-flow/cli@latest").is_ok());
        assert!(policy.validate("weft agent list").is_ok());
        assert!(policy.validate("npm install").is_ok());
    }

    #[test]
    fn dangerous_pattern_still_blocks_compound() {
        let policy = CommandPolicy::safe_defaults();
        // Dangerous pattern check runs before compound splitting.
        let err = policy.validate("echo hi && rm -rf /").unwrap_err();
        assert!(matches!(err, CommandPolicyError::DangerousPattern { .. }));
    }
}
