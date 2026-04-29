//! Types for cargo tool operations.

use serde::{Deserialize, Serialize};

/// Configuration for cargo commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CargoConfig {
    /// Working directory for cargo commands.
    #[serde(default)]
    pub working_dir: Option<String>,

    /// Path to the cargo binary. Defaults to `"cargo"`.
    #[serde(default = "default_cargo_binary")]
    pub cargo_binary: String,
}

fn default_cargo_binary() -> String {
    "cargo".to_string()
}

impl Default for CargoConfig {
    fn default() -> Self {
        Self {
            working_dir: None,
            cargo_binary: default_cargo_binary(),
        }
    }
}

/// Result of a cargo command execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CargoResult {
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

/// Allowed cargo subcommands. Used for validation to prevent
/// arbitrary command execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CargoSubcommand {
    Build,
    Test,
    Clippy,
    Check,
    Publish,
}

impl CargoSubcommand {
    /// Parse a string into a known cargo subcommand.
    /// Returns `None` for unrecognized commands.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "build" => Some(Self::Build),
            "test" => Some(Self::Test),
            "clippy" => Some(Self::Clippy),
            "check" => Some(Self::Check),
            "publish" => Some(Self::Publish),
            _ => None,
        }
    }

    /// The subcommand string for the cargo CLI.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Test => "test",
            Self::Clippy => "clippy",
            Self::Check => "check",
            Self::Publish => "publish",
        }
    }

    /// Whether this subcommand supports `--message-format=json`.
    pub fn supports_json_output(&self) -> bool {
        matches!(self, Self::Build | Self::Check | Self::Clippy)
    }
}

/// Validated flags for cargo commands. Prevents injection.
#[derive(Debug, Clone, Default)]
pub struct CargoFlags {
    /// Build in release mode.
    pub release: bool,

    /// Apply to the entire workspace.
    pub workspace: bool,

    /// Target a specific package.
    pub package: Option<String>,

    /// Use JSON message format for structured output.
    pub json_output: bool,

    /// Additional validated arguments (e.g., `--features`, `--no-default-features`).
    pub extra_args: Vec<String>,
}

/// Characters allowed in package names (alphanumeric, `-`, `_`).
fn is_valid_package_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// Characters allowed in feature names (alphanumeric, `-`, `_`, `/`).
fn is_valid_feature_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '/')
}

impl CargoFlags {
    /// Convert flags to command-line arguments.
    ///
    /// All arguments are built programmatically to prevent shell injection.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        if self.release {
            args.push("--release".to_string());
        }
        if self.workspace {
            args.push("--workspace".to_string());
        }
        if let Some(ref pkg) = self.package {
            args.push("-p".to_string());
            args.push(pkg.clone());
        }
        if self.json_output {
            args.push("--message-format=json".to_string());
        }

        args.extend(self.extra_args.iter().cloned());
        args
    }

    /// Parse and validate flags from a JSON parameters object.
    ///
    /// Returns an error string if any argument fails validation.
    pub fn from_params(params: &serde_json::Value) -> Result<Self, String> {
        let mut flags = Self::default();

        if let Some(release) = params.get("release").and_then(|v| v.as_bool()) {
            flags.release = release;
        }
        if let Some(workspace) = params.get("workspace").and_then(|v| v.as_bool()) {
            flags.workspace = workspace;
        }
        if let Some(pkg) = params.get("package").and_then(|v| v.as_str()) {
            if !is_valid_package_name(pkg) {
                return Err(format!("invalid package name: '{pkg}'"));
            }
            flags.package = Some(pkg.to_string());
        }
        if let Some(json) = params.get("json_output").and_then(|v| v.as_bool()) {
            flags.json_output = json;
        }
        if let Some(features) = params.get("features").and_then(|v| v.as_str()) {
            // Validate each feature name
            for feat in features.split(',') {
                let feat = feat.trim();
                if !is_valid_feature_name(feat) {
                    return Err(format!("invalid feature name: '{feat}'"));
                }
            }
            flags.extra_args.push("--features".to_string());
            flags.extra_args.push(features.to_string());
        }
        if params.get("no_default_features").and_then(|v| v.as_bool()).unwrap_or(false) {
            flags.extra_args.push("--no-default-features".to_string());
        }

        Ok(flags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_subcommand_roundtrip() {
        let cmds = ["build", "test", "clippy", "check", "publish"];
        for cmd in cmds {
            let parsed = CargoSubcommand::parse(cmd).unwrap();
            assert_eq!(parsed.as_str(), cmd);
        }
    }

    #[test]
    fn cargo_subcommand_unknown_returns_none() {
        assert!(CargoSubcommand::parse("run").is_none());
        assert!(CargoSubcommand::parse("install").is_none());
        assert!(CargoSubcommand::parse("").is_none());
    }

    #[test]
    fn cargo_subcommand_json_support() {
        assert!(CargoSubcommand::Build.supports_json_output());
        assert!(CargoSubcommand::Check.supports_json_output());
        assert!(CargoSubcommand::Clippy.supports_json_output());
        assert!(!CargoSubcommand::Test.supports_json_output());
        assert!(!CargoSubcommand::Publish.supports_json_output());
    }

    #[test]
    fn cargo_flags_to_args_empty() {
        let flags = CargoFlags::default();
        assert!(flags.to_args().is_empty());
    }

    #[test]
    fn cargo_flags_to_args_full() {
        let flags = CargoFlags {
            release: true,
            workspace: true,
            package: Some("my-crate".to_string()),
            json_output: true,
            extra_args: vec!["--features".to_string(), "serde".to_string()],
        };
        let args = flags.to_args();
        assert!(args.contains(&"--release".to_string()));
        assert!(args.contains(&"--workspace".to_string()));
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"my-crate".to_string()));
        assert!(args.contains(&"--message-format=json".to_string()));
    }

    #[test]
    fn cargo_flags_from_params_valid() {
        let params = serde_json::json!({
            "release": true,
            "package": "my-crate",
            "features": "serde,tokio",
            "no_default_features": true
        });
        let flags = CargoFlags::from_params(&params).unwrap();
        assert!(flags.release);
        assert_eq!(flags.package, Some("my-crate".to_string()));
        assert!(flags.extra_args.contains(&"--features".to_string()));
        assert!(flags.extra_args.contains(&"--no-default-features".to_string()));
    }

    #[test]
    fn cargo_flags_rejects_invalid_package_name() {
        let params = serde_json::json!({ "package": "evil; rm -rf /" });
        let result = CargoFlags::from_params(&params);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid package name"));
    }

    #[test]
    fn cargo_flags_rejects_invalid_feature_name() {
        let params = serde_json::json!({ "features": "ok,bad;evil" });
        let result = CargoFlags::from_params(&params);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid feature name"));
    }

    #[test]
    fn valid_package_names() {
        assert!(is_valid_package_name("my-crate"));
        assert!(is_valid_package_name("my_crate"));
        assert!(is_valid_package_name("crate123"));
        assert!(!is_valid_package_name(""));
        assert!(!is_valid_package_name("evil command"));
        assert!(!is_valid_package_name("path/../traversal"));
    }

    #[test]
    fn cargo_config_default() {
        let config = CargoConfig::default();
        assert!(config.working_dir.is_none());
        assert_eq!(config.cargo_binary, "cargo");
    }
}
