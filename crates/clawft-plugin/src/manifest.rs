//! Plugin manifest types.
//!
//! Defines [`PluginManifest`], [`PluginCapability`], [`PluginPermissions`],
//! and [`PluginResourceConfig`] -- the schema for plugin metadata parsed
//! from `clawft.plugin.json` or `.yaml` files.

use serde::{Deserialize, Serialize};

use crate::PluginError;

/// Plugin manifest parsed from `clawft.plugin.json` or `.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique plugin identifier (reverse-domain, e.g., `"com.example.my-plugin"`).
    pub id: String,

    /// Human-readable plugin name.
    pub name: String,

    /// Semantic version string (must be valid semver).
    pub version: String,

    /// Capabilities this plugin provides.
    pub capabilities: Vec<PluginCapability>,

    /// Permissions the plugin requests.
    #[serde(default)]
    pub permissions: PluginPermissions,

    /// Resource limits configuration.
    #[serde(default)]
    pub resources: PluginResourceConfig,

    /// Path to the WASM module (relative to plugin directory).
    #[serde(default)]
    pub wasm_module: Option<String>,

    /// Skills provided by this plugin.
    #[serde(default)]
    pub skills: Vec<String>,

    /// Tools provided by this plugin.
    #[serde(default)]
    pub tools: Vec<String>,

    /// Voice capability declaration (WEFT-556 / SC-10).
    ///
    /// Plugins MUST declare this section AND list `PluginCapability::Voice`
    /// in [`PluginManifest::capabilities`] before the WASM host will
    /// forward voice transcripts or accept synthesize-audio host calls.
    /// Each sub-permission is independently gated against the user's
    /// per-plugin grant matrix at load time.
    ///
    /// `None` (the default) means the plugin requests no voice access.
    #[serde(default)]
    pub voice: Option<VoiceCapability>,
}

/// Per-plugin voice capability declaration (WEFT-556 / SC-10).
///
/// A plugin requests voice access by setting this in its manifest:
///
/// ```json
/// {
///   "capabilities": ["voice"],
///   "voice": {
///     "read_transcripts": true,
///     "dispatch_commands": false,
///     "synthesize_audio": true,
///     "transcript_topics": ["weftos.voice.transcripts.v1"]
///   }
/// }
/// ```
///
/// Each sub-permission is independent and must be granted by the user
/// (via [`VoiceGrants`](crate::manifest::VoiceGrants) loaded from
/// `~/.clawft/config.json`'s `plugins.voice_grants.<plugin_id>`).
///
/// At load time the manifest is intersected with the grant matrix via
/// [`validate_voice_capability`]; any `true` here that the user has not
/// granted causes [`SkillLoadError::VoiceCapabilityNotGranted`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceCapability {
    /// Plugin RECEIVES voice transcripts as input via the WASM host's
    /// substrate-publish forwarding path.
    #[serde(default, alias = "readTranscripts")]
    pub read_transcripts: bool,

    /// Plugin may dispatch commands derived from voice transcripts back
    /// into the daemon's RPC surface. Strictly more powerful than
    /// `read_transcripts` and must be granted independently.
    #[serde(default, alias = "dispatchCommands")]
    pub dispatch_commands: bool,

    /// Plugin may call `host.synthesize_audio(text)` to produce TTS
    /// output. Without this sub-permission the host returns
    /// [`WasmHostError::CapabilityDenied`](crate::error::WasmHostError::CapabilityDenied)
    /// at runtime.
    #[serde(default, alias = "synthesizeAudio")]
    pub synthesize_audio: bool,

    /// Specific substrate transcript topics the plugin wants to
    /// subscribe to. Empty = the plugin accepts whatever default
    /// topic the host forwards (typically the daemon's
    /// `voice.consumer.transcript_topic`). Non-empty topics are still
    /// gated by `read_transcripts`.
    #[serde(default, alias = "transcriptTopics")]
    pub transcript_topics: Vec<String>,
}

impl VoiceCapability {
    /// Returns `true` when no sub-permission is requested. A plugin in
    /// this state has effectively declared no voice access; the host
    /// treats it the same as a missing manifest entry.
    pub fn is_empty(&self) -> bool {
        !self.read_transcripts
            && !self.dispatch_commands
            && !self.synthesize_audio
            && self.transcript_topics.is_empty()
    }
}

/// Per-plugin voice sub-permission grants (WEFT-556 / SC-10).
///
/// Loaded from `~/.clawft/config.json`'s
/// `plugins.voice_grants.<plugin_id>` and consumed by
/// [`validate_voice_capability`]. Each field mirrors the corresponding
/// [`VoiceCapability`] field but represents what the **operator** has
/// approved rather than what the plugin **requested**.
///
/// Defaults to all-false: the operator must explicitly opt each
/// sub-permission in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceGrants {
    /// Operator allows the plugin to receive voice transcripts.
    #[serde(default, alias = "readTranscripts")]
    pub read_transcripts: bool,

    /// Operator allows the plugin to dispatch commands derived from
    /// voice transcripts.
    #[serde(default, alias = "dispatchCommands")]
    pub dispatch_commands: bool,

    /// Operator allows the plugin to synthesize TTS audio.
    #[serde(default, alias = "synthesizeAudio")]
    pub synthesize_audio: bool,

    /// Topics the operator has whitelisted. Empty = any topic the
    /// plugin requests is allowed (subject to the boolean grants
    /// above). Non-empty = each topic in the manifest must appear in
    /// this list.
    #[serde(default, alias = "transcriptTopics")]
    pub transcript_topics: Vec<String>,
}

/// Plugin capability types.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    /// Tool execution capability.
    Tool,
    /// Channel adapter capability.
    Channel,
    /// Pipeline stage capability.
    PipelineStage,
    /// Skill definition capability.
    Skill,
    /// Memory backend capability.
    MemoryBackend,
    /// Reserved for Workstream G (voice/audio).
    Voice,
}

/// Permissions requested by a plugin.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginPermissions {
    /// Allowed network hosts. Empty = no network. `["*"]` = all hosts.
    #[serde(default)]
    pub network: Vec<String>,

    /// Allowed filesystem paths.
    #[serde(default)]
    pub filesystem: Vec<String>,

    /// Allowed environment variable names.
    #[serde(default)]
    pub env_vars: Vec<String>,

    /// Whether the plugin can execute shell commands.
    #[serde(default)]
    pub shell: bool,
}

/// Resource limits for plugin execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginResourceConfig {
    /// Maximum WASM fuel per invocation (default: 1,000,000,000).
    #[serde(default = "default_max_fuel")]
    pub max_fuel: u64,

    /// Maximum WASM memory in MB (default: 16).
    #[serde(default = "default_max_memory_mb")]
    pub max_memory_mb: usize,

    /// Maximum HTTP requests per minute (default: 10).
    #[serde(default = "default_max_http_rpm")]
    pub max_http_requests_per_minute: u64,

    /// Maximum log messages per minute (default: 100).
    #[serde(default = "default_max_log_rpm")]
    pub max_log_messages_per_minute: u64,

    /// Maximum execution wall-clock seconds (default: 30).
    #[serde(default = "default_max_exec_seconds")]
    pub max_execution_seconds: u64,

    /// Maximum WASM table elements (default: 10,000).
    #[serde(default = "default_max_table_elements")]
    pub max_table_elements: u32,
}

fn default_max_fuel() -> u64 {
    1_000_000_000
}
fn default_max_memory_mb() -> usize {
    16
}
fn default_max_http_rpm() -> u64 {
    10
}
fn default_max_log_rpm() -> u64 {
    100
}
fn default_max_exec_seconds() -> u64 {
    30
}
fn default_max_table_elements() -> u32 {
    10_000
}

impl Default for PluginResourceConfig {
    fn default() -> Self {
        Self {
            max_fuel: default_max_fuel(),
            max_memory_mb: default_max_memory_mb(),
            max_http_requests_per_minute: default_max_http_rpm(),
            max_log_messages_per_minute: default_max_log_rpm(),
            max_execution_seconds: default_max_exec_seconds(),
            max_table_elements: default_max_table_elements(),
        }
    }
}

/// Represents new permissions requested by a plugin version upgrade
/// that were not present in the previously approved permission set.
///
/// Used to determine which permissions need user re-approval when a
/// plugin updates its manifest.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PermissionDiff {
    /// Network hosts requested that were not previously approved.
    pub new_network: Vec<String>,
    /// Filesystem paths requested that were not previously approved.
    pub new_filesystem: Vec<String>,
    /// Environment variables requested that were not previously approved.
    pub new_env_vars: Vec<String>,
    /// Whether shell access is being escalated from `false` to `true`.
    pub shell_escalation: bool,
}

impl PermissionDiff {
    /// Returns `true` if no new permissions are being requested.
    pub fn is_empty(&self) -> bool {
        self.new_network.is_empty()
            && self.new_filesystem.is_empty()
            && self.new_env_vars.is_empty()
            && !self.shell_escalation
    }
}

impl PluginPermissions {
    /// Compute the diff between previously approved permissions and newly
    /// requested permissions.
    ///
    /// Returns a [`PermissionDiff`] containing only the items in `requested`
    /// that are not present in `approved`. For the `shell` field, only an
    /// escalation from `false` to `true` counts as a new permission.
    pub fn diff(approved: &PluginPermissions, requested: &PluginPermissions) -> PermissionDiff {
        let new_network = requested
            .network
            .iter()
            .filter(|item| !approved.network.contains(item))
            .cloned()
            .collect();

        let new_filesystem = requested
            .filesystem
            .iter()
            .filter(|item| !approved.filesystem.contains(item))
            .cloned()
            .collect();

        let new_env_vars = requested
            .env_vars
            .iter()
            .filter(|item| !approved.env_vars.contains(item))
            .cloned()
            .collect();

        let shell_escalation = !approved.shell && requested.shell;

        PermissionDiff {
            new_network,
            new_filesystem,
            new_env_vars,
            shell_escalation,
        }
    }
}

impl PluginManifest {
    /// Validate the manifest. Returns an error describing the first
    /// validation failure, or `Ok(())` if the manifest is valid.
    pub fn validate(&self) -> Result<(), PluginError> {
        if self.id.is_empty() {
            return Err(PluginError::LoadFailed(
                "manifest: id is required".into(),
            ));
        }
        if self.id.len() > 128 {
            return Err(PluginError::LoadFailed(
                "manifest: id must be 128 characters or fewer".into(),
            ));
        }
        if !self
            .id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-' || c == '_')
        {
            return Err(PluginError::LoadFailed(
                "manifest: id must contain only alphanumeric characters, dots, hyphens, and underscores".into(),
            ));
        }
        if self.name.is_empty() {
            return Err(PluginError::LoadFailed(
                "manifest: name is required".into(),
            ));
        }
        // Validate semver
        if semver::Version::parse(&self.version).is_err() {
            return Err(PluginError::LoadFailed(format!(
                "manifest: invalid semver version '{}'",
                self.version
            )));
        }
        if self.capabilities.is_empty() {
            return Err(PluginError::LoadFailed(
                "manifest: at least one capability is required".into(),
            ));
        }
        Ok(())
    }

    /// Parse a manifest from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, PluginError> {
        let manifest: Self = serde_json::from_str(json)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Parse a manifest from a YAML string.
    ///
    /// Note: `serde_yaml` is NOT a dependency of `clawft-plugin` to keep the
    /// crate lightweight. YAML manifest parsing is handled in the loader
    /// layer (C3) which calls `serde_yaml::from_str()` and then constructs a
    /// `PluginManifest`. This method is a convenience stub.
    pub fn from_yaml(_yaml: &str) -> Result<Self, PluginError> {
        Err(PluginError::NotImplemented(
            "YAML manifest parsing deferred to C3 skill loader".into(),
        ))
    }

    /// Parse a manifest from the legacy `.weftos-plugin.toml` format.
    ///
    /// # WEFT-64
    ///
    /// The legacy TOML schema is:
    ///
    /// ```toml
    /// [plugin]
    /// name = "my-plugin"            # → manifest.name
    /// type = "tool"                 # mapped to manifest.capabilities
    /// version = "0.1.0"             # → manifest.version
    /// description = ""
    /// author = ""
    /// license = "MIT OR Apache-2.0"
    ///
    /// [compatibility]
    /// weftos_min_version = "0.4.0"  # informational; not in PluginManifest
    /// ```
    ///
    /// The canonical format going forward is `clawft.plugin.json`. This
    /// reader accepts the legacy TOML, converts it to a `PluginManifest`,
    /// and emits a `tracing::warn!` deprecation notice. Callers that want
    /// to surface the warning to end-users should also print to stderr.
    ///
    /// Conversion rules:
    ///
    /// - `[plugin].name` → `manifest.name` and `manifest.id`
    ///   (`id` synthesized as `weftos.plugin.<name>` if no id key is present)
    /// - `[plugin].version` → `manifest.version`
    /// - `[plugin].type` → mapped to a single capability:
    ///   - `"tool"` → `Tool`
    ///   - `"channel"` → `Channel`
    ///   - `"analyzer"` → `PipelineStage`
    ///   - `"skill"` → `Skill`
    ///   - anything else → `Tool` (default)
    /// - `permissions` and `resources` use crate defaults (legacy TOML did
    ///   not encode them).
    pub fn from_legacy_toml(toml_str: &str) -> Result<Self, PluginError> {
        tracing::warn!(
            "loading deprecated .weftos-plugin.toml manifest format; \
             please migrate to clawft.plugin.json"
        );

        // Parse minimally without bringing in toml as a hard dep: clawft-plugin
        // already does not list toml in Cargo.toml. We do a hand-rolled
        // sectioned scan for the keys we need. This avoids dragging the toml
        // crate into clawft-plugin's dep graph.
        let parsed = parse_legacy_toml(toml_str)
            .map_err(|e| PluginError::LoadFailed(format!("legacy TOML parse: {e}")))?;

        let plugin = parsed.get("plugin").ok_or_else(|| {
            PluginError::LoadFailed(
                "legacy TOML: missing [plugin] table".into(),
            )
        })?;

        let name = plugin.get("name").cloned().ok_or_else(|| {
            PluginError::LoadFailed(
                "legacy TOML: missing [plugin].name".into(),
            )
        })?;

        let version = plugin
            .get("version")
            .cloned()
            .unwrap_or_else(|| "0.1.0".to_string());

        let plugin_type = plugin
            .get("type")
            .map(|s| s.as_str())
            .unwrap_or("tool");
        let capability = match plugin_type {
            "tool" => PluginCapability::Tool,
            "channel" => PluginCapability::Channel,
            "analyzer" => PluginCapability::PipelineStage,
            "skill" => PluginCapability::Skill,
            "memory" | "memory_backend" => PluginCapability::MemoryBackend,
            "voice" => PluginCapability::Voice,
            _ => PluginCapability::Tool,
        };

        let id = plugin
            .get("id")
            .cloned()
            .unwrap_or_else(|| format!("weftos.plugin.{name}"));

        let manifest = PluginManifest {
            id,
            name,
            version,
            capabilities: vec![capability],
            permissions: PluginPermissions::default(),
            resources: PluginResourceConfig::default(),
            wasm_module: None,
            skills: Vec::new(),
            tools: Vec::new(),
            voice: None,
        };

        manifest.validate()?;
        Ok(manifest)
    }
}

// ── WEFT-556 / SC-10: voice capability gating ───────────────────────────

use crate::error::SkillLoadError;

/// Validate the manifest's [`VoiceCapability`] against the operator's
/// per-plugin [`VoiceGrants`] (WEFT-556 / SC-10).
///
/// Returns:
///
/// - `Ok(())` if the manifest declares no voice capability, OR if every
///   sub-permission set in the manifest is also granted by the operator.
/// - [`SkillLoadError::VoiceCapabilityNotGranted`] if any sub-permission
///   the manifest requests is not granted, OR if the manifest declares
///   `voice` capability membership without listing it in the
///   `capabilities` array, OR if a transcript topic in the manifest is
///   not in the operator's allowlist (when the allowlist is non-empty).
///
/// # Behavior matrix
///
/// | manifest.voice | capabilities ⊇ Voice | grants exist? | result |
/// |----------------|----------------------|---------------|--------|
/// | None | – | – | Ok |
/// | Some(empty) | – | – | Ok |
/// | Some(any) | no | – | Err (capabilities mismatch) |
/// | Some(req) | yes | None | Err (not granted) |
/// | Some(req) | yes | Some(g) | Ok iff `req ⊆ g` |
pub fn validate_voice_capability(
    plugin_id: &str,
    capabilities: &[PluginCapability],
    requested: Option<&VoiceCapability>,
    granted: Option<&VoiceGrants>,
) -> Result<(), SkillLoadError> {
    let Some(req) = requested else {
        // Plugin declared no voice section at all.
        return Ok(());
    };

    // An all-false manifest entry is treated identically to None: the
    // plugin asked for nothing, so nothing to validate.
    if req.is_empty() {
        return Ok(());
    }

    // The plugin asks for at least one voice sub-permission. The top-level
    // capabilities array MUST also list `Voice`, otherwise the manifest
    // is internally inconsistent.
    if !capabilities.contains(&PluginCapability::Voice) {
        return Err(SkillLoadError::VoiceCapabilityNotGranted {
            plugin: plugin_id.to_string(),
            denied: vec!["voice (capability not declared in capabilities[])".into()],
        });
    }

    let Some(grants) = granted else {
        // Plugin requested voice perms but the operator has not granted
        // anything for this plugin id. Reject with a list of every
        // sub-permission the plugin asked for.
        return Err(SkillLoadError::VoiceCapabilityNotGranted {
            plugin: plugin_id.to_string(),
            denied: collect_requested_perms(req),
        });
    };

    let mut denied: Vec<String> = Vec::new();
    if req.read_transcripts && !grants.read_transcripts {
        denied.push("voice.read_transcripts".into());
    }
    if req.dispatch_commands && !grants.dispatch_commands {
        denied.push("voice.dispatch_commands".into());
    }
    if req.synthesize_audio && !grants.synthesize_audio {
        denied.push("voice.synthesize_audio".into());
    }
    if !grants.transcript_topics.is_empty() {
        for topic in &req.transcript_topics {
            if !grants.transcript_topics.contains(topic) {
                denied.push(format!("voice.transcript_topic:{topic}"));
            }
        }
    }

    if denied.is_empty() {
        Ok(())
    } else {
        denied.sort();
        denied.dedup();
        Err(SkillLoadError::VoiceCapabilityNotGranted {
            plugin: plugin_id.to_string(),
            denied,
        })
    }
}

fn collect_requested_perms(req: &VoiceCapability) -> Vec<String> {
    let mut out = Vec::new();
    if req.read_transcripts {
        out.push("voice.read_transcripts".into());
    }
    if req.dispatch_commands {
        out.push("voice.dispatch_commands".into());
    }
    if req.synthesize_audio {
        out.push("voice.synthesize_audio".into());
    }
    for topic in &req.transcript_topics {
        out.push(format!("voice.transcript_topic:{topic}"));
    }
    out
}

/// Minimal hand-rolled TOML scanner that recognizes simple `[section]` lines
/// and `key = "string"` entries. Sufficient for the legacy
/// `.weftos-plugin.toml` schema, which only uses string scalars under
/// `[plugin]` and `[compatibility]`. Comments (`#`) and blank lines are
/// skipped. Unquoted values are treated as strings up to end-of-line.
fn parse_legacy_toml(
    s: &str,
) -> std::result::Result<std::collections::HashMap<String, std::collections::HashMap<String, String>>, String>
{
    use std::collections::HashMap;

    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current_section: Option<String> = None;

    for (lineno, raw) in s.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Strip trailing comments from value lines.
        if let Some(stripped) = line.strip_prefix('[') {
            let close = stripped
                .find(']')
                .ok_or_else(|| format!("line {}: unterminated [section]", lineno + 1))?;
            let name = stripped[..close].trim().to_string();
            current_section = Some(name);
            continue;
        }

        let section = current_section.as_ref().ok_or_else(|| {
            format!("line {}: key=value outside any [section]", lineno + 1)
        })?;

        let eq = line
            .find('=')
            .ok_or_else(|| format!("line {}: expected key = value", lineno + 1))?;
        let key = line[..eq].trim().to_string();
        let mut value = line[eq + 1..].trim().to_string();

        // Handle quoted values: extract substring up to the matching closing
        // quote, then ignore everything after (e.g. trailing comment).
        if value.starts_with('"') || value.starts_with('\'') {
            let quote_char = value.chars().next().unwrap();
            let body = &value[1..];
            if let Some(close) = body.find(quote_char) {
                value = body[..close].to_string();
            } else {
                return Err(format!("line {}: unterminated quoted value", lineno + 1));
            }
        } else {
            // Unquoted: strip trailing inline comment.
            if let Some(hash) = value.find('#') {
                value = value[..hash].trim().to_string();
            }
        }

        out.entry(section.clone())
            .or_default()
            .insert(key, value);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_manifest_json() -> String {
        serde_json::json!({
            "id": "com.example.test-plugin",
            "name": "Test Plugin",
            "version": "1.0.0",
            "capabilities": ["tool", "skill"],
            "permissions": {
                "network": ["api.example.com"],
                "filesystem": ["/tmp/plugin"],
                "env_vars": ["MY_API_KEY"],
                "shell": false
            },
            "resources": {
                "max_fuel": 500_000_000u64,
                "max_memory_mb": 8,
                "max_http_requests_per_minute": 5,
                "max_log_messages_per_minute": 50,
                "max_execution_seconds": 15,
                "max_table_elements": 5000
            },
            "wasm_module": "plugin.wasm",
            "skills": ["code-review"],
            "tools": ["lint_code"]
        })
        .to_string()
    }

    #[test]
    fn test_manifest_parse_json() {
        let json = valid_manifest_json();
        let manifest = PluginManifest::from_json(&json).unwrap();
        assert_eq!(manifest.id, "com.example.test-plugin");
        assert_eq!(manifest.name, "Test Plugin");
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.capabilities.len(), 2);
        assert_eq!(manifest.capabilities[0], PluginCapability::Tool);
        assert_eq!(manifest.capabilities[1], PluginCapability::Skill);
        assert_eq!(manifest.permissions.network, vec!["api.example.com"]);
        assert_eq!(manifest.permissions.filesystem, vec!["/tmp/plugin"]);
        assert_eq!(manifest.permissions.env_vars, vec!["MY_API_KEY"]);
        assert!(!manifest.permissions.shell);
        assert_eq!(manifest.resources.max_fuel, 500_000_000);
        assert_eq!(manifest.resources.max_memory_mb, 8);
        assert_eq!(manifest.resources.max_http_requests_per_minute, 5);
        assert_eq!(manifest.resources.max_log_messages_per_minute, 50);
        assert_eq!(manifest.resources.max_execution_seconds, 15);
        assert_eq!(manifest.resources.max_table_elements, 5000);
        assert_eq!(manifest.wasm_module, Some("plugin.wasm".into()));
        assert_eq!(manifest.skills, vec!["code-review"]);
        assert_eq!(manifest.tools, vec!["lint_code"]);
    }

    #[test]
    fn test_manifest_parse_yaml_returns_not_implemented() {
        let result = PluginManifest::from_yaml("name: test");
        assert!(result.is_err());
        match result.unwrap_err() {
            PluginError::NotImplemented(msg) => {
                assert!(msg.contains("YAML manifest parsing deferred"));
            }
            other => panic!("expected NotImplemented, got: {other}"),
        }
    }

    #[test]
    fn test_manifest_missing_id_fails() {
        let json = serde_json::json!({
            "id": "",
            "name": "Test",
            "version": "1.0.0",
            "capabilities": ["tool"]
        })
        .to_string();
        let err = PluginManifest::from_json(&json).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("id is required"), "got: {msg}");
    }

    #[test]
    fn test_manifest_invalid_version_fails() {
        let json = serde_json::json!({
            "id": "com.test",
            "name": "Test",
            "version": "not-semver",
            "capabilities": ["tool"]
        })
        .to_string();
        let err = PluginManifest::from_json(&json).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid semver"), "got: {msg}");
    }

    #[test]
    fn test_manifest_empty_capabilities_fails() {
        let json = serde_json::json!({
            "id": "com.test",
            "name": "Test",
            "version": "1.0.0",
            "capabilities": []
        })
        .to_string();
        let err = PluginManifest::from_json(&json).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("at least one capability"),
            "got: {msg}"
        );
    }

    #[test]
    fn test_manifest_missing_name_fails() {
        let json = serde_json::json!({
            "id": "com.test",
            "name": "",
            "version": "1.0.0",
            "capabilities": ["tool"]
        })
        .to_string();
        let err = PluginManifest::from_json(&json).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("name is required"), "got: {msg}");
    }

    #[test]
    fn test_plugin_capability_serde_roundtrip() {
        let capabilities = vec![
            PluginCapability::Tool,
            PluginCapability::Channel,
            PluginCapability::PipelineStage,
            PluginCapability::Skill,
            PluginCapability::MemoryBackend,
            PluginCapability::Voice,
        ];
        for cap in &capabilities {
            let json = serde_json::to_string(cap).unwrap();
            let restored: PluginCapability = serde_json::from_str(&json).unwrap();
            assert_eq!(&restored, cap);
        }
    }

    #[test]
    fn test_plugin_capability_json_values() {
        assert_eq!(
            serde_json::to_string(&PluginCapability::Tool).unwrap(),
            "\"tool\""
        );
        assert_eq!(
            serde_json::to_string(&PluginCapability::Channel).unwrap(),
            "\"channel\""
        );
        assert_eq!(
            serde_json::to_string(&PluginCapability::PipelineStage).unwrap(),
            "\"pipeline_stage\""
        );
        assert_eq!(
            serde_json::to_string(&PluginCapability::Skill).unwrap(),
            "\"skill\""
        );
        assert_eq!(
            serde_json::to_string(&PluginCapability::MemoryBackend).unwrap(),
            "\"memory_backend\""
        );
        assert_eq!(
            serde_json::to_string(&PluginCapability::Voice).unwrap(),
            "\"voice\""
        );
    }

    #[test]
    fn test_permissions_default_is_empty() {
        let perms = PluginPermissions::default();
        assert!(perms.network.is_empty());
        assert!(perms.filesystem.is_empty());
        assert!(perms.env_vars.is_empty());
        assert!(!perms.shell);
    }

    #[test]
    fn test_resource_config_defaults() {
        let config = PluginResourceConfig::default();
        assert_eq!(config.max_fuel, 1_000_000_000);
        assert_eq!(config.max_memory_mb, 16);
        assert_eq!(config.max_http_requests_per_minute, 10);
        assert_eq!(config.max_log_messages_per_minute, 100);
        assert_eq!(config.max_execution_seconds, 30);
        assert_eq!(config.max_table_elements, 10_000);
    }

    #[test]
    fn test_manifest_with_defaults() {
        let json = serde_json::json!({
            "id": "com.test.minimal",
            "name": "Minimal",
            "version": "0.1.0",
            "capabilities": ["tool"]
        })
        .to_string();
        let manifest = PluginManifest::from_json(&json).unwrap();
        // Permissions default to empty
        assert!(manifest.permissions.network.is_empty());
        assert!(!manifest.permissions.shell);
        // Resources default to standard values
        assert_eq!(manifest.resources.max_fuel, 1_000_000_000);
        assert_eq!(manifest.resources.max_memory_mb, 16);
        // Optional fields default to None/empty
        assert!(manifest.wasm_module.is_none());
        assert!(manifest.skills.is_empty());
        assert!(manifest.tools.is_empty());
    }

    #[test]
    fn test_manifest_serde_roundtrip() {
        let json = valid_manifest_json();
        let manifest = PluginManifest::from_json(&json).unwrap();
        let serialized = serde_json::to_string(&manifest).unwrap();
        let restored = PluginManifest::from_json(&serialized).unwrap();
        assert_eq!(manifest.id, restored.id);
        assert_eq!(manifest.name, restored.name);
        assert_eq!(manifest.version, restored.version);
        assert_eq!(manifest.capabilities, restored.capabilities);
    }

    #[test]
    fn test_permissions_serde_roundtrip() {
        let perms = PluginPermissions {
            network: vec!["*.example.com".into(), "api.test.com".into()],
            filesystem: vec!["/tmp".into(), "/data".into()],
            env_vars: vec!["MY_KEY".into()],
            shell: true,
        };
        let json = serde_json::to_string(&perms).unwrap();
        let restored: PluginPermissions = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.network, perms.network);
        assert_eq!(restored.filesystem, perms.filesystem);
        assert_eq!(restored.env_vars, perms.env_vars);
        assert_eq!(restored.shell, perms.shell);
    }

    // -- PermissionDiff tests --

    #[test]
    fn diff_identical_permissions_is_empty() {
        let perms = PluginPermissions {
            network: vec!["api.example.com".into()],
            filesystem: vec!["/tmp".into()],
            env_vars: vec!["HOME".into()],
            shell: true,
        };
        let diff = PluginPermissions::diff(&perms, &perms);
        assert!(diff.is_empty());
        assert_eq!(diff, PermissionDiff::default());
    }

    #[test]
    fn diff_detects_new_network_hosts() {
        let approved = PluginPermissions {
            network: vec!["api.example.com".into()],
            ..Default::default()
        };
        let requested = PluginPermissions {
            network: vec!["api.example.com".into(), "cdn.example.com".into()],
            ..Default::default()
        };
        let diff = PluginPermissions::diff(&approved, &requested);
        assert_eq!(diff.new_network, vec!["cdn.example.com"]);
        assert!(diff.new_filesystem.is_empty());
        assert!(diff.new_env_vars.is_empty());
        assert!(!diff.shell_escalation);
        assert!(!diff.is_empty());
    }

    #[test]
    fn diff_detects_new_filesystem_paths() {
        let approved = PluginPermissions {
            filesystem: vec!["/tmp".into()],
            ..Default::default()
        };
        let requested = PluginPermissions {
            filesystem: vec!["/tmp".into(), "/data".into()],
            ..Default::default()
        };
        let diff = PluginPermissions::diff(&approved, &requested);
        assert_eq!(diff.new_filesystem, vec!["/data"]);
    }

    #[test]
    fn diff_detects_new_env_vars() {
        let approved = PluginPermissions {
            env_vars: vec!["HOME".into()],
            ..Default::default()
        };
        let requested = PluginPermissions {
            env_vars: vec!["HOME".into(), "API_KEY".into()],
            ..Default::default()
        };
        let diff = PluginPermissions::diff(&approved, &requested);
        assert_eq!(diff.new_env_vars, vec!["API_KEY"]);
    }

    #[test]
    fn diff_detects_shell_escalation() {
        let approved = PluginPermissions {
            shell: false,
            ..Default::default()
        };
        let requested = PluginPermissions {
            shell: true,
            ..Default::default()
        };
        let diff = PluginPermissions::diff(&approved, &requested);
        assert!(diff.shell_escalation);
        assert!(!diff.is_empty());
    }

    #[test]
    fn diff_no_shell_escalation_when_already_approved() {
        let approved = PluginPermissions {
            shell: true,
            ..Default::default()
        };
        let requested = PluginPermissions {
            shell: true,
            ..Default::default()
        };
        let diff = PluginPermissions::diff(&approved, &requested);
        assert!(!diff.shell_escalation);
    }

    #[test]
    fn diff_no_shell_escalation_on_downgrade() {
        let approved = PluginPermissions {
            shell: true,
            ..Default::default()
        };
        let requested = PluginPermissions {
            shell: false,
            ..Default::default()
        };
        let diff = PluginPermissions::diff(&approved, &requested);
        assert!(!diff.shell_escalation);
    }

    #[test]
    fn diff_empty_approved_all_requested_are_new() {
        let approved = PluginPermissions::default();
        let requested = PluginPermissions {
            network: vec!["a.com".into(), "b.com".into()],
            filesystem: vec!["/data".into()],
            env_vars: vec!["KEY".into()],
            shell: true,
        };
        let diff = PluginPermissions::diff(&approved, &requested);
        assert_eq!(diff.new_network, vec!["a.com", "b.com"]);
        assert_eq!(diff.new_filesystem, vec!["/data"]);
        assert_eq!(diff.new_env_vars, vec!["KEY"]);
        assert!(diff.shell_escalation);
    }

    #[test]
    fn diff_removed_permissions_not_reported() {
        // If requested drops a permission that was approved, it should NOT
        // appear as a new permission (only additions are reported).
        let approved = PluginPermissions {
            network: vec!["old.example.com".into(), "keep.example.com".into()],
            ..Default::default()
        };
        let requested = PluginPermissions {
            network: vec!["keep.example.com".into()],
            ..Default::default()
        };
        let diff = PluginPermissions::diff(&approved, &requested);
        assert!(diff.is_empty());
    }

    #[test]
    fn diff_wildcard_network_is_treated_as_new_entry() {
        // Wildcard "*" is compared as a literal string entry.
        // If the approved set has specific domains but the requested set
        // adds a wildcard, the wildcard is detected as a new entry.
        let approved = PluginPermissions {
            network: vec!["api.example.com".into()],
            ..Default::default()
        };
        let requested = PluginPermissions {
            network: vec!["api.example.com".into(), "*".into()],
            ..Default::default()
        };
        let diff = PluginPermissions::diff(&approved, &requested);
        assert_eq!(diff.new_network, vec!["*"]);
    }

    #[test]
    fn permission_diff_is_empty_default() {
        let diff = PermissionDiff::default();
        assert!(diff.is_empty());
    }

    // ── WEFT-64: legacy .weftos-plugin.toml reader ────────────────

    #[test]
    fn legacy_toml_basic_parse() {
        let toml = r#"
[plugin]
name = "my-plugin"
type = "tool"
version = "0.2.0"
description = "A test"
author = "alice"
license = "MIT"

[compatibility]
weftos_min_version = "0.4.0"
"#;
        let manifest = PluginManifest::from_legacy_toml(toml).unwrap();
        assert_eq!(manifest.name, "my-plugin");
        assert_eq!(manifest.version, "0.2.0");
        assert_eq!(manifest.id, "weftos.plugin.my-plugin");
        assert_eq!(manifest.capabilities, vec![PluginCapability::Tool]);
    }

    #[test]
    fn legacy_toml_channel_type_maps_capability() {
        let toml = r#"
[plugin]
name = "slack-bridge"
type = "channel"
version = "0.1.0"
"#;
        let manifest = PluginManifest::from_legacy_toml(toml).unwrap();
        assert_eq!(manifest.capabilities, vec![PluginCapability::Channel]);
    }

    #[test]
    fn legacy_toml_analyzer_type_maps_pipeline_stage() {
        let toml = r#"
[plugin]
name = "lint-pass"
type = "analyzer"
version = "0.1.0"
"#;
        let manifest = PluginManifest::from_legacy_toml(toml).unwrap();
        assert_eq!(
            manifest.capabilities,
            vec![PluginCapability::PipelineStage]
        );
    }

    #[test]
    fn legacy_toml_unknown_type_falls_back_to_tool() {
        let toml = r#"
[plugin]
name = "weird"
type = "no_such_type"
version = "0.1.0"
"#;
        let manifest = PluginManifest::from_legacy_toml(toml).unwrap();
        assert_eq!(manifest.capabilities, vec![PluginCapability::Tool]);
    }

    #[test]
    fn legacy_toml_missing_plugin_table_fails() {
        let toml = r#"
[compatibility]
weftos_min_version = "0.4.0"
"#;
        let err = PluginManifest::from_legacy_toml(toml).unwrap_err();
        assert!(err.to_string().contains("[plugin]"), "got: {err}");
    }

    #[test]
    fn legacy_toml_missing_name_fails() {
        let toml = r#"
[plugin]
type = "tool"
version = "0.1.0"
"#;
        let err = PluginManifest::from_legacy_toml(toml).unwrap_err();
        assert!(err.to_string().contains("name"), "got: {err}");
    }

    #[test]
    fn legacy_toml_invalid_version_fails() {
        let toml = r#"
[plugin]
name = "bad-version"
type = "tool"
version = "not-semver"
"#;
        let err = PluginManifest::from_legacy_toml(toml).unwrap_err();
        assert!(err.to_string().contains("invalid semver"), "got: {err}");
    }

    #[test]
    fn legacy_toml_handles_comments_and_blank_lines() {
        let toml = r#"
# top comment

[plugin]
# inline section comment
name = "commenter"   # trailing comment
type = "tool"
version = "1.0.0"
"#;
        let manifest = PluginManifest::from_legacy_toml(toml).unwrap();
        assert_eq!(manifest.name, "commenter");
        assert_eq!(manifest.version, "1.0.0");
    }

    // ── WEFT-556 / SC-10: voice capability tests ────────────────────

    fn cap_voice() -> Vec<PluginCapability> {
        vec![PluginCapability::Voice]
    }

    #[test]
    fn voice_capability_parses_from_manifest_json() {
        let json = serde_json::json!({
            "id": "com.example.voice-plugin",
            "name": "Voice Plugin",
            "version": "0.1.0",
            "capabilities": ["voice"],
            "voice": {
                "read_transcripts": true,
                "dispatch_commands": false,
                "synthesize_audio": true,
                "transcript_topics": ["weftos.voice.transcripts.v1"]
            }
        })
        .to_string();
        let manifest = PluginManifest::from_json(&json).unwrap();
        let voice = manifest.voice.expect("voice section parsed");
        assert!(voice.read_transcripts);
        assert!(!voice.dispatch_commands);
        assert!(voice.synthesize_audio);
        assert_eq!(
            voice.transcript_topics,
            vec!["weftos.voice.transcripts.v1"]
        );
    }

    #[test]
    fn voice_capability_camel_case_aliases() {
        let json = serde_json::json!({
            "id": "com.example.camel",
            "name": "Camel",
            "version": "0.1.0",
            "capabilities": ["voice"],
            "voice": {
                "readTranscripts": true,
                "dispatchCommands": true,
                "synthesizeAudio": false,
                "transcriptTopics": ["t.a"]
            }
        })
        .to_string();
        let manifest = PluginManifest::from_json(&json).unwrap();
        let voice = manifest.voice.unwrap();
        assert!(voice.read_transcripts);
        assert!(voice.dispatch_commands);
        assert!(!voice.synthesize_audio);
        assert_eq!(voice.transcript_topics, vec!["t.a"]);
    }

    #[test]
    fn manifest_without_voice_section_defaults_to_none() {
        let json = serde_json::json!({
            "id": "com.example.no-voice",
            "name": "Quiet",
            "version": "0.1.0",
            "capabilities": ["tool"]
        })
        .to_string();
        let manifest = PluginManifest::from_json(&json).unwrap();
        assert!(manifest.voice.is_none());
    }

    #[test]
    fn voice_capability_is_empty_detects_all_false() {
        let v = VoiceCapability::default();
        assert!(v.is_empty());
        let v = VoiceCapability {
            read_transcripts: true,
            ..Default::default()
        };
        assert!(!v.is_empty());
    }

    #[test]
    fn validate_voice_no_request_succeeds_without_grant() {
        let r = validate_voice_capability("p", &[PluginCapability::Tool], None, None);
        assert!(r.is_ok());
    }

    #[test]
    fn validate_voice_empty_request_succeeds_without_grant() {
        let req = VoiceCapability::default();
        let r = validate_voice_capability("p", &cap_voice(), Some(&req), None);
        assert!(r.is_ok());
    }

    #[test]
    fn validate_voice_request_without_capability_decl_rejects() {
        // The plugin asks for voice perms but didn't list `Voice` in
        // its `capabilities[]` array. That's an internally inconsistent
        // manifest and must be rejected.
        let req = VoiceCapability {
            read_transcripts: true,
            ..Default::default()
        };
        let grants = VoiceGrants {
            read_transcripts: true,
            ..Default::default()
        };
        let err = validate_voice_capability(
            "p",
            &[PluginCapability::Tool],
            Some(&req),
            Some(&grants),
        )
        .expect_err("must reject");
        match err {
            SkillLoadError::VoiceCapabilityNotGranted { plugin, denied } => {
                assert_eq!(plugin, "p");
                assert!(denied.iter().any(|d| d.contains("capabilities[]")));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn validate_voice_request_without_grant_rejects() {
        // Plugin requests dispatch_commands but operator hasn't granted
        // anything (no entry in voice_grants for this plugin).
        let req = VoiceCapability {
            dispatch_commands: true,
            ..Default::default()
        };
        let err = validate_voice_capability("p", &cap_voice(), Some(&req), None)
            .expect_err("must reject");
        match err {
            SkillLoadError::VoiceCapabilityNotGranted { plugin, denied } => {
                assert_eq!(plugin, "p");
                assert_eq!(denied, vec!["voice.dispatch_commands"]);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn validate_voice_dispatch_without_grant_is_the_canonical_failure() {
        // Plugin declares dispatch_commands: true but the operator hasn't
        // granted voice.commands -> reject with VoiceCapabilityNotGranted.
        let req = VoiceCapability {
            read_transcripts: true,
            dispatch_commands: true,
            ..Default::default()
        };
        let grants = VoiceGrants {
            read_transcripts: true,
            // dispatch_commands NOT granted
            ..Default::default()
        };
        let err = validate_voice_capability("p", &cap_voice(), Some(&req), Some(&grants))
            .expect_err("must reject");
        match err {
            SkillLoadError::VoiceCapabilityNotGranted { denied, .. } => {
                assert!(denied.contains(&"voice.dispatch_commands".to_string()));
                // read_transcripts WAS granted, so it must NOT appear:
                assert!(!denied.contains(&"voice.read_transcripts".to_string()));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn validate_voice_full_grant_succeeds() {
        let req = VoiceCapability {
            read_transcripts: true,
            dispatch_commands: true,
            synthesize_audio: true,
            transcript_topics: vec!["t.a".into(), "t.b".into()],
        };
        let grants = VoiceGrants {
            read_transcripts: true,
            dispatch_commands: true,
            synthesize_audio: true,
            transcript_topics: vec!["t.a".into(), "t.b".into(), "t.c".into()],
        };
        let r =
            validate_voice_capability("p", &cap_voice(), Some(&req), Some(&grants));
        assert!(r.is_ok(), "got: {r:?}");
    }

    #[test]
    fn validate_voice_topic_not_whitelisted_rejects() {
        // Operator's allowlist is non-empty but doesn't include the
        // topic the plugin asked for.
        let req = VoiceCapability {
            read_transcripts: true,
            transcript_topics: vec!["weftos.voice.transcripts.v1".into()],
            ..Default::default()
        };
        let grants = VoiceGrants {
            read_transcripts: true,
            transcript_topics: vec!["other.topic".into()],
            ..Default::default()
        };
        let err = validate_voice_capability("p", &cap_voice(), Some(&req), Some(&grants))
            .expect_err("must reject topic");
        match err {
            SkillLoadError::VoiceCapabilityNotGranted { denied, .. } => {
                assert!(denied
                    .iter()
                    .any(|d| d == "voice.transcript_topic:weftos.voice.transcripts.v1"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn validate_voice_empty_grant_topics_means_any_topic_ok() {
        // Operator left transcript_topics empty -> any topic is fine
        // as long as the boolean grants cover the request.
        let req = VoiceCapability {
            read_transcripts: true,
            transcript_topics: vec!["topic.x".into()],
            ..Default::default()
        };
        let grants = VoiceGrants {
            read_transcripts: true,
            transcript_topics: Vec::new(),
            ..Default::default()
        };
        let r =
            validate_voice_capability("p", &cap_voice(), Some(&req), Some(&grants));
        assert!(r.is_ok());
    }

    #[test]
    fn validate_voice_denied_list_is_sorted_and_deduped() {
        let req = VoiceCapability {
            read_transcripts: true,
            dispatch_commands: true,
            synthesize_audio: true,
            ..Default::default()
        };
        let grants = VoiceGrants::default(); // all false
        let err = validate_voice_capability("p", &cap_voice(), Some(&req), Some(&grants))
            .expect_err("must reject");
        match err {
            SkillLoadError::VoiceCapabilityNotGranted { denied, .. } => {
                assert_eq!(
                    denied,
                    vec![
                        "voice.dispatch_commands",
                        "voice.read_transcripts",
                        "voice.synthesize_audio",
                    ]
                );
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn voice_capability_serde_roundtrip() {
        let v = VoiceCapability {
            read_transcripts: true,
            dispatch_commands: false,
            synthesize_audio: true,
            transcript_topics: vec!["t.a".into()],
        };
        let json = serde_json::to_string(&v).unwrap();
        let restored: VoiceCapability = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, v);
    }

    #[test]
    fn legacy_toml_and_canonical_json_yield_same_struct_shape() {
        // Same logical plugin, expressed in both formats.
        let json = serde_json::json!({
            "id": "weftos.plugin.same-plugin",
            "name": "same-plugin",
            "version": "1.0.0",
            "capabilities": ["tool"]
        })
        .to_string();
        let toml = r#"
[plugin]
name = "same-plugin"
type = "tool"
version = "1.0.0"
"#;
        let from_json = PluginManifest::from_json(&json).unwrap();
        let from_toml = PluginManifest::from_legacy_toml(toml).unwrap();

        assert_eq!(from_json.id, from_toml.id);
        assert_eq!(from_json.name, from_toml.name);
        assert_eq!(from_json.version, from_toml.version);
        assert_eq!(from_json.capabilities, from_toml.capabilities);
    }
}
