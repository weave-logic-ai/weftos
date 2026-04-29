//! Types for container orchestration operations.

use serde::{Deserialize, Serialize};

/// Configuration for the container runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerConfig {
    /// Which runtime to use: `"docker"` or `"podman"`.
    #[serde(default = "default_runtime")]
    pub runtime: ContainerRuntime,

    /// Maximum number of concurrent container operations globally.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_ops: u32,
}

fn default_runtime() -> ContainerRuntime {
    ContainerRuntime::Docker
}

fn default_max_concurrent() -> u32 {
    3
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            runtime: default_runtime(),
            max_concurrent_ops: default_max_concurrent(),
        }
    }
}

/// Supported container runtimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerRuntime {
    Docker,
    Podman,
}

impl ContainerRuntime {
    /// Binary name for this runtime.
    pub fn binary(&self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
}

/// Allowed container subcommands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerSubcommand {
    Build,
    Run,
    Stop,
    Logs,
    List,
    Exec,
}

impl ContainerSubcommand {
    /// Parse a string into a known subcommand.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "build" => Some(Self::Build),
            "run" => Some(Self::Run),
            "stop" => Some(Self::Stop),
            "logs" => Some(Self::Logs),
            "list" | "ps" => Some(Self::List),
            "exec" => Some(Self::Exec),
            _ => None,
        }
    }

    /// The subcommand arguments for the container CLI.
    pub fn as_args(&self) -> &[&'static str] {
        match self {
            Self::Build => &["build"],
            Self::Run => &["run"],
            Self::Stop => &["stop"],
            Self::Logs => &["logs"],
            Self::List => &["ps"],
            Self::Exec => &["exec"],
        }
    }
}

/// Result of a container command execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerResult {
    /// Whether the command succeeded (exit code 0).
    pub success: bool,
    /// Exit code of the process.
    pub exit_code: Option<i32>,
    /// Standard output.
    pub stdout: String,
    /// Standard error output.
    pub stderr: String,
    /// The command that was executed (for logging/debugging).
    pub command: String,
}

// ---------------------------------------------------------------------------
// Input validation
// ---------------------------------------------------------------------------

/// Characters allowed in container/image names (alphanumeric, `-`, `_`, `.`, `/`, `:`).
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':'))
}

/// Characters allowed in tag values (alphanumeric, `-`, `_`, `.`).
pub fn is_valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Validate an environment variable assignment (`KEY=VALUE`).
/// Key must be alphanumeric + underscore. Value can be anything printable.
pub fn is_valid_env_var(s: &str) -> bool {
    if let Some(eq_pos) = s.find('=') {
        let key = &s[..eq_pos];
        !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_')
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_runtime_binary() {
        assert_eq!(ContainerRuntime::Docker.binary(), "docker");
        assert_eq!(ContainerRuntime::Podman.binary(), "podman");
    }

    #[test]
    fn container_runtime_serde() {
        let json = serde_json::to_string(&ContainerRuntime::Docker).unwrap();
        assert_eq!(json, r#""docker""#);
        let restored: ContainerRuntime = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, ContainerRuntime::Docker);
    }

    #[test]
    fn subcommand_roundtrip() {
        let cmds = ["build", "run", "stop", "logs", "list", "exec"];
        for cmd in cmds {
            assert!(
                ContainerSubcommand::parse(cmd).is_some(),
                "failed to parse: {cmd}"
            );
        }
        // ps is an alias for list
        assert_eq!(
            ContainerSubcommand::parse("ps"),
            Some(ContainerSubcommand::List)
        );
    }

    #[test]
    fn subcommand_unknown_returns_none() {
        assert!(ContainerSubcommand::parse("rm").is_none());
        assert!(ContainerSubcommand::parse("").is_none());
        assert!(ContainerSubcommand::parse("pull").is_none());
    }

    #[test]
    fn valid_container_names() {
        assert!(is_valid_name("my-container"));
        assert!(is_valid_name("registry.io/image:latest"));
        assert!(is_valid_name("ubuntu"));
        assert!(is_valid_name("my_app.v2"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("evil; rm -rf /"));
        assert!(!is_valid_name("name with spaces"));
    }

    #[test]
    fn valid_tags() {
        assert!(is_valid_tag("latest"));
        assert!(is_valid_tag("v1.0.0"));
        assert!(is_valid_tag("my_tag"));
        assert!(!is_valid_tag(""));
        assert!(!is_valid_tag("tag with spaces"));
    }

    #[test]
    fn valid_env_vars() {
        assert!(is_valid_env_var("KEY=value"));
        assert!(is_valid_env_var("MY_VAR=some value with spaces"));
        assert!(is_valid_env_var("A="));
        assert!(!is_valid_env_var("no-equals"));
        assert!(!is_valid_env_var("=no-key"));
        assert!(!is_valid_env_var("BAD-KEY=value"));
    }

    #[test]
    fn container_config_default() {
        let config = ContainerConfig::default();
        assert_eq!(config.runtime, ContainerRuntime::Docker);
        assert_eq!(config.max_concurrent_ops, 3);
    }
}
