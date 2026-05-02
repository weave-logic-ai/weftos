//! Per-agent sandbox enforcement.
//!
//! Translates agent configuration into a [`SandboxPolicy`] and enforces
//! sandbox restrictions at runtime.  The OS-level sandbox (seccomp +
//! landlock on Linux) is **not yet implemented**; when it is requested the
//! system logs a warning and falls back to WASM-only sandboxing.  On
//! non-Linux platforms the same WASM-only fallback applies.
//!
//! # Architecture
//!
//! ```text
//! Agent Config (config.toml)
//!       |
//!       v
//! SandboxPolicy (clawft-plugin)
//!       |
//!       v
//! SandboxEnforcer (this module)
//!       |
//!       +---> WASM sandbox (clawft-wasm)
//!       +---> OS sandbox (seccomp/landlock, Linux only)
//! ```
//!
//! # Audit Logging
//!
//! All sandbox decisions (allow/deny) are logged via the audit system.
//! Denied actions are logged at WARN level; allowed actions at DEBUG.

use clawft_plugin::sandbox::{SandboxAuditEntry, SandboxPolicy, SandboxType};
use std::sync::{Arc, Mutex};

/// Sandbox enforcer for a single agent.
///
/// Wraps a [`SandboxPolicy`] and provides enforcement methods that validate
/// actions against the policy and emit audit log entries.
pub struct SandboxEnforcer {
    /// The policy being enforced.
    policy: SandboxPolicy,
    /// Audit log (in-memory ring buffer, capped at `max_audit_entries`).
    audit_log: Arc<Mutex<Vec<SandboxAuditEntry>>>,
    /// Maximum audit entries to retain in memory.
    max_audit_entries: usize,
}

impl SandboxEnforcer {
    /// Create a new enforcer from a policy.
    pub fn new(policy: SandboxPolicy) -> Self {
        Self {
            policy,
            audit_log: Arc::new(Mutex::new(Vec::new())),
            max_audit_entries: 10_000,
        }
    }

    /// Get a reference to the underlying policy.
    pub fn policy(&self) -> &SandboxPolicy {
        &self.policy
    }

    /// Validate a tool invocation.
    ///
    /// Returns `Ok(())` if the tool is allowed, or `Err` with a reason.
    pub fn check_tool(&self, tool_name: &str) -> Result<(), String> {
        let allowed = self.policy.is_tool_allowed(tool_name);
        self.log_decision("tool_invoke", tool_name, allowed, "tool not in allowlist");
        if allowed {
            Ok(())
        } else {
            Err(format!(
                "agent '{}' is not allowed to use tool '{}'",
                self.policy.agent_id, tool_name
            ))
        }
    }

    /// Validate a network connection.
    pub fn check_network(&self, domain: &str) -> Result<(), String> {
        let allowed = self.policy.is_domain_allowed(domain);
        self.log_decision("network_connect", domain, allowed, "domain not allowed");
        if allowed {
            Ok(())
        } else {
            Err(format!(
                "agent '{}' is not allowed to connect to '{}'",
                self.policy.agent_id, domain
            ))
        }
    }

    /// Validate a file read operation.
    pub fn check_file_read(&self, path: &std::path::Path) -> Result<(), String> {
        let path_str = path.to_string_lossy();
        let allowed = self.policy.is_path_readable(path);
        self.log_decision("file_read", &path_str, allowed, "path not readable");
        if allowed {
            Ok(())
        } else {
            Err(format!(
                "agent '{}' cannot read '{}'",
                self.policy.agent_id, path_str
            ))
        }
    }

    /// Validate a file write operation.
    pub fn check_file_write(&self, path: &std::path::Path) -> Result<(), String> {
        let path_str = path.to_string_lossy();
        let allowed = self.policy.is_path_writable(path);
        self.log_decision("file_write", &path_str, allowed, "path not writable");
        if allowed {
            Ok(())
        } else {
            Err(format!(
                "agent '{}' cannot write '{}'",
                self.policy.agent_id, path_str
            ))
        }
    }

    /// Validate a command execution.
    ///
    /// Emits a `chain_event` for ExoChain auditing when the command is
    /// allowed (denied actions are already captured in the audit log).
    pub fn check_command(&self, command: &str) -> Result<(), String> {
        let allowed = self.policy.is_command_allowed(command);
        self.log_decision("command_exec", command, allowed, "command not allowed");

        // Chain event marker -- the daemon subscriber forwards this to
        // ChainManager::append("sandbox", "sandbox.execute", ...).
        crate::chain_event!(
            "sandbox",
            crate::chain_event::EVENT_KIND_SANDBOX_EXECUTE,
            {
                "agent_id": self.policy.agent_id,
                "action": "command_exec",
                "target": command,
                "allowed": allowed
            }
        );

        if allowed {
            Ok(())
        } else {
            Err(format!(
                "agent '{}' cannot execute command '{}'",
                self.policy.agent_id, command
            ))
        }
    }

    /// Get the effective sandbox type for this platform.
    pub fn effective_sandbox_type(&self) -> SandboxType {
        self.policy.effective_sandbox_type()
    }

    /// Retrieve the current audit log entries.
    pub fn audit_entries(&self) -> Vec<SandboxAuditEntry> {
        self.audit_log.lock().unwrap().clone()
    }

    /// Clear the audit log.
    pub fn clear_audit_log(&self) {
        self.audit_log.lock().unwrap().clear();
    }

    /// Log a sandbox decision.
    fn log_decision(&self, action: &str, target: &str, allowed: bool, deny_reason: &str) {
        if !self.policy.audit_logging {
            return;
        }

        let entry = if allowed {
            tracing::debug!(
                agent = %self.policy.agent_id,
                action = action,
                target = target,
                "sandbox: allowed"
            );
            SandboxAuditEntry::allowed(&self.policy.agent_id, action, target)
        } else {
            tracing::warn!(
                agent = %self.policy.agent_id,
                action = action,
                target = target,
                reason = deny_reason,
                "sandbox: denied"
            );
            SandboxAuditEntry::denied(&self.policy.agent_id, action, target, deny_reason)
        };

        let mut log = self.audit_log.lock().unwrap();
        if log.len() >= self.max_audit_entries {
            let keep = self.max_audit_entries / 2;
            let drain_end = log.len() - keep;
            log.drain(..drain_end);
        }
        log.push(entry);
    }
}

/// Apply OS-level sandbox restrictions.
///
/// On Linux, this sets up seccomp and landlock rules based on the policy.
/// On other platforms, this is a no-op that logs a warning.
pub fn apply_os_sandbox(policy: &SandboxPolicy) -> Result<(), String> {
    let effective = policy.effective_sandbox_type();
    match effective {
        SandboxType::Wasm => {
            tracing::debug!(
                agent = %policy.agent_id,
                "using WASM-only sandbox (no OS sandbox)"
            );
            Ok(())
        }
        SandboxType::OsSandbox | SandboxType::Combined => {
            #[cfg(target_os = "linux")]
            {
                if let Err(e) = apply_linux_sandbox(policy) {
                    tracing::warn!(
                        agent = %policy.agent_id,
                        error = %e,
                        "OS sandbox unavailable, using WASM sandbox only"
                    );
                }
                Ok(())
            }
            #[cfg(not(target_os = "linux"))]
            {
                tracing::warn!(
                    agent = %policy.agent_id,
                    "OS sandbox requested but unavailable on this platform; \
                     WASM sandbox will be used instead"
                );
                Ok(())
            }
        }
        _ => {
            tracing::warn!(
                agent = %policy.agent_id,
                "unknown sandbox type variant; falling back to WASM sandbox"
            );
            Ok(())
        }
    }
}

/// Linux-specific sandbox setup using seccomp and landlock.
///
/// **Not yet implemented.** Returns `Err` so callers can fall back to
/// WASM-only sandboxing.  When seccomp and landlock support is added this
/// function should apply the policy and return `Ok(())`.
#[cfg(target_os = "linux")]
fn apply_linux_sandbox(policy: &SandboxPolicy) -> Result<(), String> {
    tracing::warn!(
        agent = %policy.agent_id,
        "Linux OS sandbox (seccomp + landlock) is not yet implemented; \
         falling back to WASM-only sandboxing"
    );
    Err("Linux OS sandbox not yet implemented: seccomp and landlock support pending".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_plugin::sandbox::{FilesystemPolicy, NetworkPolicy, ProcessPolicy};
    use std::path::PathBuf;

    fn test_policy() -> SandboxPolicy {
        SandboxPolicy {
            agent_id: "test-agent".into(),
            sandbox_type: SandboxType::Wasm,
            network: NetworkPolicy {
                allow_network: true,
                allowed_domains: vec!["api.example.com".into()],
                ..Default::default()
            },
            filesystem: FilesystemPolicy {
                readable_paths: vec![PathBuf::from("/workspace")],
                writable_paths: vec![PathBuf::from("/tmp/output")],
                ..Default::default()
            },
            process: ProcessPolicy {
                allow_shell: true,
                allowed_commands: vec!["git".into(), "cargo".into()],
                blocked_commands: vec!["rm".into()],
                ..Default::default()
            },
            allowed_tools: vec!["read_file".into(), "write_file".into()],
            denied_tools: vec!["bash_exec".into()],
            ..Default::default()
        }
    }

    #[test]
    fn enforcer_allows_permitted_tool() {
        let enforcer = SandboxEnforcer::new(test_policy());
        assert!(enforcer.check_tool("read_file").is_ok());
    }

    #[test]
    fn enforcer_denies_unpermitted_tool() {
        let enforcer = SandboxEnforcer::new(test_policy());
        assert!(enforcer.check_tool("bash_exec").is_err());
        assert!(enforcer.check_tool("unknown_tool").is_err());
    }

    #[test]
    fn enforcer_allows_permitted_domain() {
        let enforcer = SandboxEnforcer::new(test_policy());
        assert!(enforcer.check_network("api.example.com").is_ok());
    }

    #[test]
    fn enforcer_denies_unpermitted_domain() {
        let enforcer = SandboxEnforcer::new(test_policy());
        assert!(enforcer.check_network("evil.com").is_err());
    }

    /// Build a policy whose readable + writable allowlists point at
    /// real, existing tempdirs. The canonicalize-prefix sandbox check
    /// (lifted into `clawft-plugin::sandbox` in agent-core-v1 A3)
    /// requires both the target and the allowlist roots to canonicalize
    /// successfully — i.e. to exist on disk — so hard-coded
    /// `/workspace` paths no longer work in tests.
    fn test_policy_with_dirs(read_dir: &std::path::Path, write_dir: &std::path::Path) -> SandboxPolicy {
        let mut policy = test_policy();
        policy.filesystem.readable_paths = vec![read_dir.to_path_buf()];
        policy.filesystem.writable_paths = vec![write_dir.to_path_buf()];
        policy
    }

    #[test]
    fn enforcer_allows_readable_path() {
        let read_dir = tempfile::tempdir().unwrap();
        // Place a real file under the readable root so canonicalize succeeds.
        let target = read_dir.path().join("main.rs");
        std::fs::write(&target, b"fn main() {}").unwrap();
        let enforcer =
            SandboxEnforcer::new(test_policy_with_dirs(read_dir.path(), read_dir.path()));
        assert!(enforcer.check_file_read(&target).is_ok());
    }

    #[test]
    fn enforcer_denies_unreadable_path() {
        let read_dir = tempfile::tempdir().unwrap();
        let enforcer =
            SandboxEnforcer::new(test_policy_with_dirs(read_dir.path(), read_dir.path()));
        assert!(enforcer
            .check_file_read(std::path::Path::new("/etc/passwd"))
            .is_err());
    }

    #[test]
    fn enforcer_allows_writable_path() {
        let write_dir = tempfile::tempdir().unwrap();
        // Writable check tolerates not-yet-existing targets so long as
        // the parent canonicalizes inside the allowlist.
        let target = write_dir.path().join("result.json");
        let enforcer =
            SandboxEnforcer::new(test_policy_with_dirs(write_dir.path(), write_dir.path()));
        assert!(enforcer.check_file_write(&target).is_ok());
    }

    #[test]
    fn enforcer_denies_unwritable_path() {
        let write_dir = tempfile::tempdir().unwrap();
        let enforcer =
            SandboxEnforcer::new(test_policy_with_dirs(write_dir.path(), write_dir.path()));
        assert!(enforcer
            .check_file_write(std::path::Path::new("/etc/config"))
            .is_err());
    }

    #[test]
    fn enforcer_allows_permitted_command() {
        let enforcer = SandboxEnforcer::new(test_policy());
        assert!(enforcer.check_command("git").is_ok());
        assert!(enforcer.check_command("cargo").is_ok());
    }

    #[test]
    fn enforcer_denies_blocked_command() {
        let enforcer = SandboxEnforcer::new(test_policy());
        assert!(enforcer.check_command("rm").is_err());
    }

    #[test]
    fn enforcer_denies_unlisted_command() {
        let enforcer = SandboxEnforcer::new(test_policy());
        assert!(enforcer.check_command("wget").is_err());
    }

    #[test]
    fn audit_log_records_decisions() {
        let enforcer = SandboxEnforcer::new(test_policy());
        let _ = enforcer.check_tool("read_file");
        let _ = enforcer.check_tool("bash_exec");
        let entries = enforcer.audit_entries();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].allowed);
        assert!(!entries[1].allowed);
    }

    #[test]
    fn audit_log_can_be_cleared() {
        let enforcer = SandboxEnforcer::new(test_policy());
        let _ = enforcer.check_tool("read_file");
        assert_eq!(enforcer.audit_entries().len(), 1);
        enforcer.clear_audit_log();
        assert!(enforcer.audit_entries().is_empty());
    }

    #[test]
    fn apply_os_sandbox_wasm_only() {
        let policy = SandboxPolicy {
            agent_id: "test".into(),
            sandbox_type: SandboxType::Wasm,
            ..Default::default()
        };
        assert!(apply_os_sandbox(&policy).is_ok());
    }

    #[test]
    fn apply_os_sandbox_os_type() {
        let policy = SandboxPolicy {
            agent_id: "test".into(),
            sandbox_type: SandboxType::OsSandbox,
            ..Default::default()
        };
        assert!(apply_os_sandbox(&policy).is_ok());
    }
}
