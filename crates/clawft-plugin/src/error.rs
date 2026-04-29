//! Plugin error types.
//!
//! Defines [`PluginError`], the unified error type for all plugin operations
//! including loading, execution, permission checks, and resource limits.

use thiserror::Error;

/// Errors produced by plugin operations.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum PluginError {
    /// Plugin failed to load (bad manifest, missing WASM module, etc.).
    #[error("plugin load failed: {0}")]
    LoadFailed(String),

    /// Plugin execution failed at runtime.
    #[error("plugin execution failed: {0}")]
    ExecutionFailed(String),

    /// Operation denied by the permission sandbox.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// Plugin exceeded a resource limit (fuel, memory, rate limit).
    #[error("resource exhausted: {0}")]
    ResourceExhausted(String),

    /// Requested capability or feature is not implemented.
    #[error("not implemented: {0}")]
    NotImplemented(String),

    /// I/O error during plugin operation.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Serialization/deserialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Errors raised by skill loading and validation.
///
/// # WEFT-65
///
/// Surfaced when a skill's declared `allowed_tools` includes one or more
/// tools that are not present in the user/skill grant matrix. The skill
/// loader rejects load with this error rather than silently dropping the
/// ungranted entries.
#[non_exhaustive]
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SkillLoadError {
    /// One or more declared tools are not in the grant scope for the skill.
    ///
    /// The `denied` field contains the offending tool names (sorted, dedup'd).
    #[error(
        "skill '{skill}' declares tool(s) not in its grant scope: {denied:?}"
    )]
    ToolNotGranted {
        /// Skill that failed load.
        skill: String,
        /// Tools the skill asked for but is not granted.
        denied: Vec<String>,
    },

    /// Skill manifest could not be parsed.
    #[error("skill '{skill}' manifest invalid: {reason}")]
    ManifestInvalid {
        /// Skill name (or path, if name was unreadable).
        skill: String,
        /// Reason the manifest is rejected.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_load_failed() {
        let err = PluginError::LoadFailed("bad manifest".into());
        assert_eq!(err.to_string(), "plugin load failed: bad manifest");
    }

    #[test]
    fn error_display_execution_failed() {
        let err = PluginError::ExecutionFailed("runtime crash".into());
        assert_eq!(err.to_string(), "plugin execution failed: runtime crash");
    }

    #[test]
    fn error_display_permission_denied() {
        let err = PluginError::PermissionDenied("network access".into());
        assert_eq!(err.to_string(), "permission denied: network access");
    }

    #[test]
    fn error_display_resource_exhausted() {
        let err = PluginError::ResourceExhausted("fuel limit".into());
        assert_eq!(err.to_string(), "resource exhausted: fuel limit");
    }

    #[test]
    fn error_display_not_implemented() {
        let err = PluginError::NotImplemented("voice processing".into());
        assert_eq!(err.to_string(), "not implemented: voice processing");
    }

    #[test]
    fn error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err = PluginError::from(io_err);
        assert!(matches!(err, PluginError::Io(_)));
        assert!(err.to_string().contains("file missing"));
    }

    #[test]
    fn error_from_serde_json() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let err = PluginError::from(json_err);
        assert!(matches!(err, PluginError::Serialization(_)));
    }

    #[test]
    fn all_seven_variants_exist() {
        // Compile-time verification that all 7 variants exist and are constructable.
        let _variants: Vec<PluginError> = vec![
            PluginError::LoadFailed(String::new()),
            PluginError::ExecutionFailed(String::new()),
            PluginError::PermissionDenied(String::new()),
            PluginError::ResourceExhausted(String::new()),
            PluginError::NotImplemented(String::new()),
            PluginError::Io(std::io::Error::new(std::io::ErrorKind::Other, "")),
            PluginError::Serialization(
                serde_json::from_str::<serde_json::Value>("!").unwrap_err(),
            ),
        ];
        assert_eq!(_variants.len(), 7);
    }
}
