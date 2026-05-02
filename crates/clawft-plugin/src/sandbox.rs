//! Per-agent sandbox policy definitions.
//!
//! The [`SandboxPolicy`] struct defines the runtime security restrictions for
//! an agent or plugin. It maps from per-agent config (`~/.clawft/agents/<id>/config.toml`)
//! to enforceable sandbox rules.
//!
//! The [`SandboxType`] enum determines which isolation mechanism is used:
//! - `Wasm` -- WASM sandbox (cross-platform, default for WASM plugins)
//! - `OsSandbox` -- seccomp + landlock on Linux (default for native on Linux)
//! - `Combined` -- both WASM + OS sandbox layers
//!
//! **Secure by default**: The default sandbox type is NOT `None`. WASM plugins
//! get `Wasm`, native execution on Linux gets `OsSandbox`.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// Sandbox isolation mechanism.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxType {
    /// WASM sandbox via wasmtime WASI capabilities (cross-platform).
    Wasm,
    /// OS-level sandbox: seccomp + landlock on Linux.
    OsSandbox,
    /// Both WASM and OS-level sandbox layers.
    Combined,
}

impl Default for SandboxType {
    fn default() -> Self {
        // Secure by default: use OS sandbox on Linux, WASM elsewhere.
        if cfg!(target_os = "linux") {
            Self::OsSandbox
        } else {
            Self::Wasm
        }
    }
}

/// Network access policy for a sandboxed agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkPolicy {
    /// Whether network access is allowed at all.
    #[serde(default)]
    pub allow_network: bool,

    /// Allowed domain patterns (exact or wildcard `*.example.com`).
    #[serde(default)]
    pub allowed_domains: Vec<String>,

    /// Blocked domain patterns (takes precedence over allowed).
    #[serde(default)]
    pub blocked_domains: Vec<String>,

    /// Maximum outbound connections per minute.
    #[serde(default = "default_max_connections")]
    pub max_connections_per_minute: u32,
}

fn default_max_connections() -> u32 {
    30
}

/// Filesystem access policy for a sandboxed agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FilesystemPolicy {
    /// Paths the agent can read from.
    #[serde(default)]
    pub readable_paths: Vec<PathBuf>,

    /// Paths the agent can write to.
    #[serde(default)]
    pub writable_paths: Vec<PathBuf>,

    /// Whether the agent can create new files.
    #[serde(default)]
    pub allow_create: bool,

    /// Whether the agent can delete files.
    #[serde(default)]
    pub allow_delete: bool,

    /// Maximum individual file size in bytes (default: 8MB).
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,
}

fn default_max_file_size() -> u64 {
    8 * 1024 * 1024
}

/// Process execution policy for a sandboxed agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessPolicy {
    /// Whether the agent can execute shell commands.
    #[serde(default)]
    pub allow_shell: bool,

    /// Allowed command names (empty = none allowed unless `allow_shell` is true).
    #[serde(default)]
    pub allowed_commands: Vec<String>,

    /// Blocked command patterns (takes precedence over allowed).
    #[serde(default)]
    pub blocked_commands: Vec<String>,

    /// Maximum execution time per command in seconds.
    #[serde(default = "default_max_exec_time")]
    pub max_execution_seconds: u32,
}

fn default_max_exec_time() -> u32 {
    30
}

/// Environment variable access policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnvPolicy {
    /// Allowed environment variable names.
    #[serde(default)]
    pub allowed_vars: Vec<String>,

    /// Variables that are never accessible (hardcoded deny list).
    #[serde(default)]
    pub denied_vars: Vec<String>,
}

/// Per-agent sandbox policy.
///
/// Created from an agent's configuration and enforced at runtime by the
/// sandbox enforcement layer. Each agent's tool restrictions map to a
/// `SandboxPolicy`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// Agent or plugin identifier.
    pub agent_id: String,

    /// Sandbox isolation type.
    #[serde(default)]
    pub sandbox_type: SandboxType,

    /// Network access policy.
    #[serde(default)]
    pub network: NetworkPolicy,

    /// Filesystem access policy.
    #[serde(default)]
    pub filesystem: FilesystemPolicy,

    /// Process execution policy.
    #[serde(default)]
    pub process: ProcessPolicy,

    /// Environment variable access policy.
    #[serde(default)]
    pub env: EnvPolicy,

    /// Tools this agent is allowed to use (empty = all tools allowed).
    #[serde(default)]
    pub allowed_tools: Vec<String>,

    /// Tools explicitly denied to this agent.
    #[serde(default)]
    pub denied_tools: Vec<String>,

    /// Whether audit logging is enabled for this agent.
    #[serde(default = "default_true")]
    pub audit_logging: bool,
}

fn default_true() -> bool {
    true
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            sandbox_type: SandboxType::default(),
            network: NetworkPolicy::default(),
            filesystem: FilesystemPolicy::default(),
            process: ProcessPolicy::default(),
            env: EnvPolicy::default(),
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
            audit_logging: true,
        }
    }
}

impl SandboxPolicy {
    /// Create a new sandbox policy for the given agent.
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            ..Default::default()
        }
    }

    /// Check whether a specific tool is allowed by this policy.
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        // Denied tools always take precedence.
        if self.denied_tools.iter().any(|t| t == tool_name) {
            return false;
        }
        // If allowed_tools is empty, all tools are allowed.
        if self.allowed_tools.is_empty() {
            return true;
        }
        self.allowed_tools.iter().any(|t| t == tool_name)
    }

    /// Check whether a domain is allowed by the network policy.
    pub fn is_domain_allowed(&self, domain: &str) -> bool {
        if !self.network.allow_network {
            return false;
        }
        // Check blocked domains first (takes precedence).
        for blocked in &self.network.blocked_domains {
            if domain_matches(domain, blocked) {
                return false;
            }
        }
        // If no allowed domains specified, all are allowed.
        if self.network.allowed_domains.is_empty() {
            return true;
        }
        self.network.allowed_domains.iter().any(|a| domain_matches(domain, a))
    }

    /// Check whether a file path is readable.
    ///
    /// Uses canonicalize + canonical-prefix comparison to defeat
    /// `..` traversal and unresolved symlinks. The target must exist
    /// for a read check; if it doesn't, the path is rejected.
    pub fn is_path_readable(&self, path: &std::path::Path) -> bool {
        is_path_within_allowlist(path, &self.filesystem.readable_paths, false)
    }

    /// Check whether a file path is writable.
    ///
    /// Uses canonicalize + canonical-prefix comparison. For not-yet-
    /// existing targets we canonicalize the parent directory and
    /// re-append the leaf so legitimate new-file creation is permitted.
    ///
    /// Identity hard-deny (agent-core-v1 Phase D1): writes to
    /// `.clawft/SOUL.md`, `.clawft/IDENTITY.md`, and
    /// `.clawft/SOUL.journal.md` are denied unconditionally — even
    /// when the workspace allowlist would otherwise cover them.
    /// `SOUL.journal.md` is the agent's self-observation log; agent
    /// writes to it must go through a substrate-mediated topic
    /// (Phase F1/F2), not direct filesystem writes.
    pub fn is_path_writable(&self, path: &std::path::Path) -> bool {
        if is_protected_identity_path(path) {
            return false;
        }
        is_path_within_allowlist(path, &self.filesystem.writable_paths, true)
    }

    /// Check whether a command is allowed by the process policy.
    pub fn is_command_allowed(&self, command: &str) -> bool {
        if !self.process.allow_shell {
            return false;
        }
        // Blocked commands take precedence.
        if self.process.blocked_commands.iter().any(|b| b == command) {
            return false;
        }
        // If allowed_commands is empty but allow_shell is true, all allowed.
        if self.process.allowed_commands.is_empty() {
            return true;
        }
        self.process.allowed_commands.iter().any(|a| a == command)
    }

    /// Collect the set of all effective tool names that are allowed.
    pub fn effective_tools(&self) -> HashSet<String> {
        let mut tools: HashSet<String> = self.allowed_tools.iter().cloned().collect();
        for denied in &self.denied_tools {
            tools.remove(denied);
        }
        tools
    }

    /// Return the platform-appropriate sandbox type.
    ///
    /// On macOS, downgrades `OsSandbox` and `Combined` to `Wasm` with a
    /// warning, since seccomp/landlock are Linux-only.
    pub fn effective_sandbox_type(&self) -> SandboxType {
        if cfg!(target_os = "linux") {
            return self.sandbox_type.clone();
        }
        // Non-Linux: WASM-only fallback.
        match &self.sandbox_type {
            SandboxType::OsSandbox | SandboxType::Combined => {
                tracing::warn!(
                    agent = %self.agent_id,
                    "OS sandbox unavailable on this platform; \
                     falling back to WASM-only sandbox"
                );
                SandboxType::Wasm
            }
            other => other.clone(),
        }
    }
}

/// Canonicalize `path` and check whether the result is contained in
/// any of the canonical roots derived from `allowlist`.
///
/// `allow_missing_target` controls behavior when `path` does not yet
/// exist on disk:
/// - `false` (read checks): reject. Symlink targets must already
///   resolve to a real file inside the allowlist.
/// - `true` (write checks): canonicalize the parent directory and
///   re-append the leaf, so creating a new file inside an allowed
///   root is permitted.
///
/// Mirrors the spike's `resolve_workspace_path` idiom in
/// `clawft-weave::daemon`. By routing both sides through
/// `Path::canonicalize`, traversal segments (`..`), unresolved
/// symlinks, and Windows extended-length prefixes (`\\?\`) all
/// normalize identically, so the `starts_with` check is sound on
/// every supported platform.
fn is_path_within_allowlist(
    path: &std::path::Path,
    allowlist: &[std::path::PathBuf],
    allow_missing_target: bool,
) -> bool {
    if allowlist.is_empty() {
        return false;
    }

    let target_canon = match path.canonicalize() {
        Ok(c) => c,
        Err(_) if allow_missing_target => {
            // Target may not exist yet (e.g. file we're about to
            // create). Canonicalize the parent directory and re-append
            // the leaf so we can still verify containment.
            let parent = match path.parent() {
                Some(p) if !p.as_os_str().is_empty() => p,
                _ => return false,
            };
            let parent_canon = match parent.canonicalize() {
                Ok(c) => c,
                Err(_) => return false,
            };
            match path.file_name() {
                Some(name) => parent_canon.join(name),
                None => return false,
            }
        }
        Err(_) => return false,
    };

    allowlist.iter().any(|allowed| match allowed.canonicalize() {
        Ok(allowed_canon) => target_canon.starts_with(&allowed_canon),
        Err(_) => false,
    })
}

/// File names inside `.clawft/` that are NEVER writable by the agent,
/// regardless of the workspace allowlist (agent-core-v1 Phase D1).
///
/// - `SOUL.md` and `IDENTITY.md`: identity files. Hand-edited by
///   the operator; agent writes would corrupt the binding-thread
///   integrity check.
/// - `SOUL.journal.md`: the append-only self-observation log. Agent
///   writes are legitimate but must flow through a substrate-mediated
///   topic with a `DerivedWriteGrant` (Phase F1/F2) so the
///   `chain.rs` witness records the append. Direct filesystem writes
///   bypass that audit, so we deny them here too.
const PROTECTED_IDENTITY_FILENAMES: &[&str] =
    &["SOUL.md", "IDENTITY.md", "SOUL.journal.md"];

/// Return `true` when `path`'s tail is `<...>/.clawft/<protected>`.
///
/// Operates on the lexical path so it works for both existing and
/// not-yet-existing targets (write checks call this before
/// canonicalize). The match is anchored on the **last two**
/// components: `.clawft/SOUL.md` matches; a stray `SOUL.md` outside
/// a `.clawft/` directory does not.
fn is_protected_identity_path(path: &std::path::Path) -> bool {
    let mut comps = path.components().rev();
    let leaf = match comps.next() {
        Some(std::path::Component::Normal(name)) => name,
        _ => return false,
    };
    let parent = match comps.next() {
        Some(std::path::Component::Normal(name)) => name,
        _ => return false,
    };
    if parent != std::ffi::OsStr::new(".clawft") {
        return false;
    }
    let leaf_str = match leaf.to_str() {
        Some(s) => s,
        None => return false,
    };
    PROTECTED_IDENTITY_FILENAMES.contains(&leaf_str)
}

/// Check whether a domain matches a pattern (exact or wildcard).
fn domain_matches(domain: &str, pattern: &str) -> bool {
    let domain_lower = domain.to_lowercase();
    let pattern_lower = pattern.to_lowercase();

    if pattern_lower == "*" {
        return true;
    }
    if let Some(suffix) = pattern_lower.strip_prefix("*.") {
        return domain_lower.ends_with(&format!(".{suffix}"))
            || domain_lower == suffix;
    }
    domain_lower == pattern_lower
}

/// Audit log entry for a sandbox decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxAuditEntry {
    /// Timestamp (ISO 8601).
    pub timestamp: String,
    /// Agent identifier.
    pub agent_id: String,
    /// Action attempted (e.g., "file_read", "network_connect", "tool_invoke").
    pub action: String,
    /// Target of the action (e.g., file path, URL, tool name).
    pub target: String,
    /// Whether the action was allowed.
    pub allowed: bool,
    /// Reason for denial (if denied).
    pub reason: Option<String>,
}

impl SandboxAuditEntry {
    /// Create a new audit entry for an allowed action.
    pub fn allowed(
        agent_id: impl Into<String>,
        action: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: chrono::Utc::now().to_rfc3339(),
            agent_id: agent_id.into(),
            action: action.into(),
            target: target.into(),
            allowed: true,
            reason: None,
        }
    }

    /// Create a new audit entry for a denied action.
    pub fn denied(
        agent_id: impl Into<String>,
        action: impl Into<String>,
        target: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: chrono::Utc::now().to_rfc3339(),
            agent_id: agent_id.into(),
            action: action.into(),
            target: target.into(),
            allowed: false,
            reason: Some(reason.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn default_sandbox_type_is_not_none() {
        let st = SandboxType::default();
        // On Linux, should be OsSandbox; on other platforms, Wasm.
        // Either way, it is NOT "None".
        assert!(matches!(st, SandboxType::OsSandbox | SandboxType::Wasm));
    }

    #[test]
    fn default_policy_has_secure_defaults() {
        let policy = SandboxPolicy::default();
        assert!(!policy.network.allow_network);
        assert!(policy.filesystem.readable_paths.is_empty());
        assert!(policy.filesystem.writable_paths.is_empty());
        assert!(!policy.process.allow_shell);
        assert!(policy.audit_logging);
    }

    #[test]
    fn tool_allowed_when_list_empty() {
        let policy = SandboxPolicy::new("test-agent");
        assert!(policy.is_tool_allowed("any_tool"));
    }

    #[test]
    fn tool_denied_when_in_denied_list() {
        let policy = SandboxPolicy {
            agent_id: "test".into(),
            denied_tools: vec!["dangerous_tool".into()],
            ..Default::default()
        };
        assert!(!policy.is_tool_allowed("dangerous_tool"));
    }

    #[test]
    fn tool_allowed_only_when_in_allowed_list() {
        let policy = SandboxPolicy {
            agent_id: "test".into(),
            allowed_tools: vec!["read_file".into(), "grep".into()],
            ..Default::default()
        };
        assert!(policy.is_tool_allowed("read_file"));
        assert!(policy.is_tool_allowed("grep"));
        assert!(!policy.is_tool_allowed("bash"));
    }

    #[test]
    fn denied_takes_precedence_over_allowed() {
        let policy = SandboxPolicy {
            agent_id: "test".into(),
            allowed_tools: vec!["bash".into()],
            denied_tools: vec!["bash".into()],
            ..Default::default()
        };
        assert!(!policy.is_tool_allowed("bash"));
    }

    #[test]
    fn domain_not_allowed_when_network_disabled() {
        let policy = SandboxPolicy::new("test");
        assert!(!policy.is_domain_allowed("example.com"));
    }

    #[test]
    fn domain_allowed_with_exact_match() {
        let policy = SandboxPolicy {
            agent_id: "test".into(),
            network: NetworkPolicy {
                allow_network: true,
                allowed_domains: vec!["api.example.com".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(policy.is_domain_allowed("api.example.com"));
        assert!(!policy.is_domain_allowed("evil.com"));
    }

    #[test]
    fn domain_wildcard_match() {
        let policy = SandboxPolicy {
            agent_id: "test".into(),
            network: NetworkPolicy {
                allow_network: true,
                allowed_domains: vec!["*.example.com".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(policy.is_domain_allowed("sub.example.com"));
        assert!(policy.is_domain_allowed("example.com"));
        assert!(!policy.is_domain_allowed("evil.com"));
    }

    #[test]
    fn blocked_domain_takes_precedence() {
        let policy = SandboxPolicy {
            agent_id: "test".into(),
            network: NetworkPolicy {
                allow_network: true,
                allowed_domains: vec!["*.example.com".into()],
                blocked_domains: vec!["evil.example.com".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!policy.is_domain_allowed("evil.example.com"));
        assert!(policy.is_domain_allowed("good.example.com"));
    }

    #[test]
    fn path_readable_check() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let file = workspace.path().join("file.rs");
        std::fs::write(&file, b"fn main() {}").expect("write file");

        let policy = SandboxPolicy {
            agent_id: "test".into(),
            filesystem: FilesystemPolicy {
                readable_paths: vec![workspace.path().to_path_buf()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(policy.is_path_readable(&file));
        // A path outside the allowlist must reject. /etc exists on
        // Unix; on platforms where it doesn't, canonicalize fails and
        // the helper still rejects.
        assert!(!policy.is_path_readable(Path::new("/etc")));
    }

    #[test]
    fn path_writable_check() {
        let sandbox = tempfile::tempdir().expect("tempdir");
        let target = sandbox.path().join("output.txt");
        // Write target does not yet exist — must still permit because
        // the parent canonicalizes inside the allowlist.
        let policy = SandboxPolicy {
            agent_id: "test".into(),
            filesystem: FilesystemPolicy {
                writable_paths: vec![sandbox.path().to_path_buf()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(policy.is_path_writable(&target));
        assert!(!policy.is_path_writable(Path::new("/etc")));
    }

    #[test]
    fn path_readable_rejects_dotdot_traversal() {
        // /workspace/../etc/passwd-style escape. Set up two real
        // siblings under a shared root so canonicalize has something
        // to chew on; then ask the policy whether
        // <workspace>/../<sibling>/secret is readable. The lexical
        // starts_with implementation said yes; the canonical one must
        // say no.
        let root = tempfile::tempdir().expect("tempdir");
        let workspace = root.path().join("workspace");
        let secrets = root.path().join("etc");
        std::fs::create_dir(&workspace).expect("mkdir workspace");
        std::fs::create_dir(&secrets).expect("mkdir etc");
        let secret_file = secrets.join("passwd");
        std::fs::write(&secret_file, b"root:x:0:0").expect("write secret");

        let policy = SandboxPolicy {
            agent_id: "test".into(),
            filesystem: FilesystemPolicy {
                readable_paths: vec![workspace.clone()],
                ..Default::default()
            },
            ..Default::default()
        };

        let traversal = workspace.join("..").join("etc").join("passwd");
        assert!(
            !policy.is_path_readable(&traversal),
            "lexical starts_with would have accepted this; canonicalize must reject"
        );
    }

    #[cfg(unix)]
    #[test]
    fn path_readable_rejects_symlink_escape() {
        let root = tempfile::tempdir().expect("tempdir");
        let workspace = root.path().join("workspace");
        let outside = root.path().join("outside");
        std::fs::create_dir(&workspace).expect("mkdir workspace");
        std::fs::create_dir(&outside).expect("mkdir outside");
        let secret = outside.join("secret.txt");
        std::fs::write(&secret, b"secret").expect("write secret");

        // workspace/escape -> ../outside/secret.txt
        let link = workspace.join("escape");
        std::os::unix::fs::symlink(&secret, &link).expect("create symlink");

        let policy = SandboxPolicy {
            agent_id: "test".into(),
            filesystem: FilesystemPolicy {
                readable_paths: vec![workspace.clone()],
                ..Default::default()
            },
            ..Default::default()
        };

        // The lexical check would have accepted `link` since it's
        // literally inside `workspace`; canonicalize must follow the
        // symlink and reject because the target is outside.
        assert!(!policy.is_path_readable(&link));
    }

    #[test]
    fn path_writable_permits_not_yet_existing_target() {
        let sandbox = tempfile::tempdir().expect("tempdir");
        let new_file = sandbox.path().join("does-not-exist-yet.txt");
        assert!(
            !new_file.exists(),
            "precondition: target must not exist for this test"
        );

        let policy = SandboxPolicy {
            agent_id: "test".into(),
            filesystem: FilesystemPolicy {
                writable_paths: vec![sandbox.path().to_path_buf()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(policy.is_path_writable(&new_file));

        // But if the parent itself doesn't exist, reject.
        let nested = sandbox.path().join("missing-dir").join("file.txt");
        assert!(!policy.is_path_writable(&nested));
    }

    #[test]
    fn path_readable_allowlist_with_trailing_slash() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let file = workspace.path().join("file.rs");
        std::fs::write(&file, b"hi").expect("write file");

        // PathBuf reconstructed with a trailing separator. Both sides
        // go through canonicalize, so the comparison must still match.
        let mut allowed_with_slash = workspace.path().as_os_str().to_owned();
        allowed_with_slash.push(std::path::MAIN_SEPARATOR.to_string());
        let policy = SandboxPolicy {
            agent_id: "test".into(),
            filesystem: FilesystemPolicy {
                readable_paths: vec![PathBuf::from(allowed_with_slash)],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(policy.is_path_readable(&file));
    }

    #[test]
    fn command_not_allowed_when_shell_disabled() {
        let policy = SandboxPolicy::new("test");
        assert!(!policy.is_command_allowed("ls"));
    }

    #[test]
    fn command_allowed_when_shell_enabled() {
        let policy = SandboxPolicy {
            agent_id: "test".into(),
            process: ProcessPolicy {
                allow_shell: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(policy.is_command_allowed("ls"));
    }

    #[test]
    fn command_blocked_takes_precedence() {
        let policy = SandboxPolicy {
            agent_id: "test".into(),
            process: ProcessPolicy {
                allow_shell: true,
                allowed_commands: vec!["rm".into()],
                blocked_commands: vec!["rm".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!policy.is_command_allowed("rm"));
    }

    #[test]
    fn effective_tools_excludes_denied() {
        let policy = SandboxPolicy {
            agent_id: "test".into(),
            allowed_tools: vec!["read".into(), "write".into(), "bash".into()],
            denied_tools: vec!["bash".into()],
            ..Default::default()
        };
        let effective = policy.effective_tools();
        assert!(effective.contains("read"));
        assert!(effective.contains("write"));
        assert!(!effective.contains("bash"));
    }

    #[test]
    fn audit_entry_allowed() {
        let entry = SandboxAuditEntry::allowed("agent-1", "file_read", "/tmp/test.txt");
        assert!(entry.allowed);
        assert!(entry.reason.is_none());
    }

    #[test]
    fn audit_entry_denied() {
        let entry = SandboxAuditEntry::denied(
            "agent-1",
            "network_connect",
            "evil.com",
            "domain not in allowlist",
        );
        assert!(!entry.allowed);
        assert_eq!(entry.reason.as_deref(), Some("domain not in allowlist"));
    }

    #[test]
    fn domain_matches_star() {
        assert!(domain_matches("anything.com", "*"));
    }

    #[test]
    fn domain_matches_case_insensitive() {
        assert!(domain_matches("API.Example.COM", "api.example.com"));
    }

    #[test]
    fn sandbox_policy_serialization_roundtrip() {
        let policy = SandboxPolicy {
            agent_id: "test-agent".into(),
            sandbox_type: SandboxType::Combined,
            network: NetworkPolicy {
                allow_network: true,
                allowed_domains: vec!["*.example.com".into()],
                blocked_domains: vec!["evil.example.com".into()],
                max_connections_per_minute: 60,
            },
            filesystem: FilesystemPolicy {
                readable_paths: vec![PathBuf::from("/workspace")],
                writable_paths: vec![PathBuf::from("/tmp")],
                allow_create: true,
                allow_delete: false,
                max_file_size: 4 * 1024 * 1024,
            },
            process: ProcessPolicy {
                allow_shell: true,
                allowed_commands: vec!["git".into(), "cargo".into()],
                blocked_commands: vec!["rm".into()],
                max_execution_seconds: 60,
            },
            env: EnvPolicy {
                allowed_vars: vec!["HOME".into()],
                denied_vars: vec!["AWS_SECRET_ACCESS_KEY".into()],
            },
            allowed_tools: vec!["read_file".into()],
            denied_tools: vec!["bash".into()],
            audit_logging: true,
        };
        let json = serde_json::to_string(&policy).unwrap();
        let restored: SandboxPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.agent_id, "test-agent");
        assert_eq!(restored.sandbox_type, SandboxType::Combined);
        assert!(restored.network.allow_network);
        assert!(restored.audit_logging);
    }

    // ── Identity hard-deny (agent-core-v1 Phase D1) ─────────────────

    fn identity_writable_policy(workspace: &std::path::Path) -> SandboxPolicy {
        SandboxPolicy {
            agent_id: "identity-test".into(),
            filesystem: FilesystemPolicy {
                writable_paths: vec![workspace.to_path_buf()],
                allow_create: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn writes_to_clawft_soul_md_are_denied_even_when_allowlisted() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let clawft = workspace.path().join(".clawft");
        std::fs::create_dir_all(&clawft).unwrap();
        std::fs::write(clawft.join("SOUL.md"), "ignored").unwrap();

        let policy = identity_writable_policy(workspace.path());
        // Sanity: a sibling file in the same workspace IS writable.
        assert!(policy.is_path_writable(&workspace.path().join("note.txt")));
        // Identity files are denied.
        assert!(!policy.is_path_writable(&clawft.join("SOUL.md")));
    }

    #[test]
    fn writes_to_clawft_identity_md_are_denied_even_when_allowlisted() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let clawft = workspace.path().join(".clawft");
        std::fs::create_dir_all(&clawft).unwrap();
        std::fs::write(clawft.join("IDENTITY.md"), "ignored").unwrap();

        let policy = identity_writable_policy(workspace.path());
        assert!(!policy.is_path_writable(&clawft.join("IDENTITY.md")));
    }

    #[test]
    fn writes_to_clawft_soul_journal_md_are_denied_even_when_allowlisted() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let clawft = workspace.path().join(".clawft");
        std::fs::create_dir_all(&clawft).unwrap();
        // SOUL.journal.md doesn't exist yet — the deny must hit even
        // for the not-yet-existing-target path.
        let policy = identity_writable_policy(workspace.path());
        assert!(!policy.is_path_writable(&clawft.join("SOUL.journal.md")));
    }

    #[test]
    fn deny_is_filename_anchored_under_dot_clawft() {
        // Same filenames OUTSIDE a `.clawft/` dir do not match the
        // hard-deny rule. (The allowlist still gets the final say.)
        let workspace = tempfile::tempdir().expect("tempdir");
        let policy = identity_writable_policy(workspace.path());
        // A workspace-root SOUL.md must still be writable — this
        // protects against false positives when an agent project
        // happens to author a top-level `SOUL.md` doc.
        let stray = workspace.path().join("SOUL.md");
        assert!(policy.is_path_writable(&stray));
    }

    #[test]
    fn protected_identity_path_predicate_matches_expected_set() {
        assert!(is_protected_identity_path(std::path::Path::new(
            "/workspace/.clawft/SOUL.md"
        )));
        assert!(is_protected_identity_path(std::path::Path::new(
            "/workspace/.clawft/IDENTITY.md"
        )));
        assert!(is_protected_identity_path(std::path::Path::new(
            "/workspace/.clawft/SOUL.journal.md"
        )));
        // Negative: similarly-named files outside `.clawft/` don't match.
        assert!(!is_protected_identity_path(std::path::Path::new(
            "/workspace/SOUL.md"
        )));
        assert!(!is_protected_identity_path(std::path::Path::new(
            "/workspace/.clawft/agents.md"
        )));
        // Negative: a directory called `.clawft/SOUL.md/` does NOT
        // match because we anchor on the leaf-as-file.
        assert!(is_protected_identity_path(std::path::Path::new(
            ".clawft/SOUL.md"
        )));
    }
}
