//! Configuration file discovery and loading.
//!
//! Ports the config discovery algorithm from Python `nanobot/config/loader.py`.
//!
//! The discovery order is:
//! 1. `CLAWFT_CONFIG` environment variable (absolute path).
//! 2. `~/.clawft/config.json`
//! 3. `~/.nanobot/config.json` (legacy fallback).
//! 4. If none found, return an empty JSON object (`{}`).
//!
//! JSON keys are normalized from camelCase to snake_case before returning,
//! matching the Python behavior where Pydantic models use snake_case field names.

use std::path::PathBuf;

use serde_json::Value;

/// Discover the config file path using the fallback chain.
///
/// Returns `None` if no config file exists at any of the candidate locations.
/// The discovery order is:
/// 1. Path from `CLAWFT_CONFIG` environment variable.
/// 2. `~/.clawft/config.json`
/// 3. `~/.nanobot/config.json`
///
/// On native targets, candidate paths are checked for existence using
/// synchronous `Path::exists()`. On non-native targets (WASM), the first
/// candidate path is returned without a filesystem existence check --
/// the caller's async `fs.exists()` handles validation.
pub fn discover_config_path(
    env: &dyn super::env::Environment,
    home_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    // Step 1: Check CLAWFT_CONFIG env var
    if let Some(env_path) = env.get_var("CLAWFT_CONFIG") {
        let path = PathBuf::from(env_path);
        return Some(path);
    }

    // Step 2 & 3: Check home directory paths
    if let Some(home) = home_dir {
        let clawft_path = home.join(".clawft").join("config.json");

        #[cfg(feature = "native")]
        {
            if clawft_path.exists() {
                return Some(clawft_path);
            }

            let nanobot_path = home.join(".nanobot").join("config.json");
            if nanobot_path.exists() {
                return Some(nanobot_path);
            }
        }

        // On non-native (WASM) targets, return the preferred path without
        // synchronous filesystem checks. The caller validates asynchronously.
        #[cfg(not(feature = "native"))]
        {
            return Some(clawft_path);
        }
    }

    None
}

/// Load raw JSON configuration using the discovery algorithm.
///
/// Merges configuration from two sources (project `weave.toml` first,
/// then user JSON config on top):
///
/// 1. `weave.toml` in the current directory (project-level settings)
/// 2. JSON config from the discovery chain:
///    - `CLAWFT_CONFIG` env var
///    - `~/.clawft/config.json`
///    - `~/.nanobot/config.json` (legacy)
///
/// JSON config values override `weave.toml` values where keys collide.
///
/// Returns the merged and key-normalized JSON value. The caller (typically
/// `clawft-types`) deserializes this into a typed `Config` struct.
pub async fn load_config_raw(
    fs: &dyn super::fs::FileSystem,
    env: &dyn super::env::Environment,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    // Layer 1: weave.toml from project root (current directory).
    let mut merged = load_weave_toml(fs).await.unwrap_or_else(|_| {
        Value::Object(serde_json::Map::new())
    });

    // Layer 2: JSON config from discovery chain (overrides weave.toml).
    let home = fs.home_dir();
    let json_path = discover_config_path(env, home);

    if let Some(path) = json_path
        && fs.exists(&path).await {
            tracing::debug!(path = %path.display(), "loading JSON config");
            match fs.read_to_string(&path).await {
                Ok(contents) => {
                    match serde_json::from_str::<Value>(&contents) {
                        Ok(json_value) => {
                            let normalized = normalize_keys(json_value);
                            deep_merge(&mut merged, &normalized);
                        }
                        Err(e) => {
                            tracing::warn!(
                                path = %path.display(),
                                error = %e,
                                "failed to parse JSON config, using weave.toml only"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "failed to read JSON config"
                    );
                }
            }
        }

    // Layer 3: workspace JSON overlay from cwd `.clawft/config.json`.
    // Most-specific wins — when a kernel runs inside a workspace, the
    // workspace's config dictates policy (channel permissions, routing
    // tiers, identity binding, etc.) while the home-dir config remains
    // the fallback for fields the workspace does not redeclare.
    //
    // This restores per-workspace policy that was lost when the loader
    // stopped reading cwd JSON; matches the convention used by `git`,
    // `npm`, etc. (most-specific config wins).
    let workspace_path = PathBuf::from(".clawft").join("config.json");
    if fs.exists(&workspace_path).await {
        tracing::debug!(
            path = %workspace_path.display(),
            "loading workspace config overlay"
        );
        match fs.read_to_string(&workspace_path).await {
            Ok(contents) => match serde_json::from_str::<Value>(&contents) {
                Ok(json_value) => {
                    let normalized = normalize_keys(json_value);
                    deep_merge(&mut merged, &normalized);
                }
                Err(e) => tracing::warn!(
                    path = %workspace_path.display(),
                    error = %e,
                    "failed to parse workspace config; ignoring overlay"
                ),
            },
            Err(e) => tracing::warn!(
                path = %workspace_path.display(),
                error = %e,
                "failed to read workspace config; ignoring overlay"
            ),
        }
    }

    if merged.as_object().is_none_or(|m| m.is_empty()) {
        tracing::info!("no config found (checked weave.toml + JSON), using defaults");
    }

    Ok(merged)
}

/// Load `weave.toml` from the current directory if it exists.
async fn load_weave_toml(
    fs: &dyn super::fs::FileSystem,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let toml_path = PathBuf::from("weave.toml");

    if !fs.exists(&toml_path).await {
        return Ok(Value::Object(serde_json::Map::new()));
    }

    tracing::debug!("loading weave.toml from project root");
    let contents = fs
        .read_to_string(&toml_path)
        .await
        .map_err(|e| format!("failed to read weave.toml: {e}"))?;

    let toml_value: toml::Value = contents.parse()
        .map_err(|e| format!("failed to parse weave.toml: {e}"))?;

    // Convert TOML Value to serde_json Value for uniform handling.
    let json_str = serde_json::to_string(&toml_value)
        .map_err(|e| format!("failed to convert weave.toml to JSON: {e}"))?;
    let json_value: Value = serde_json::from_str(&json_str)
        .map_err(|e| format!("failed to re-parse weave.toml as JSON: {e}"))?;

    Ok(normalize_keys(json_value))
}

/// Deep-merge `overlay` into `base`. Overlay values win on conflict.
/// Objects are merged recursively; non-object values are replaced.
fn deep_merge(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let entry = base_map.entry(key.clone()).or_insert(Value::Null);
                deep_merge(entry, overlay_val);
            }
        }
        (base, overlay) => {
            if !overlay.is_null() {
                *base = overlay.clone();
            }
        }
    }
}

/// Convert camelCase JSON keys to snake_case recursively.
///
/// Processes objects and arrays recursively. Non-object/array values are
/// returned unchanged. This matches the Python `convert_keys()` function
/// from `nanobot/config/loader.py`.
pub fn normalize_keys(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut new_map = serde_json::Map::new();
            for (key, val) in map {
                let snake_key = camel_to_snake(&key);
                new_map.insert(snake_key, normalize_keys(val));
            }
            Value::Object(new_map)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(normalize_keys).collect()),
        other => other,
    }
}

/// Convert a single camelCase string to snake_case.
///
/// Handles consecutive uppercase letters (acronyms) correctly:
/// a run of uppercase letters like `"HTML"` is kept together, with an
/// underscore inserted only before the last uppercase letter if it is
/// followed by a lowercase letter (indicating the start of a new word).
///
/// # Examples
/// ```
/// # use clawft_platform::config_loader::camel_to_snake;
/// assert_eq!(camel_to_snake("camelCase"), "camel_case");
/// assert_eq!(camel_to_snake("systemPrompt"), "system_prompt");
/// assert_eq!(camel_to_snake("already_snake"), "already_snake");
/// assert_eq!(camel_to_snake("HTMLParser"), "html_parser");
/// assert_eq!(camel_to_snake("getHTMLParser"), "get_html_parser");
/// assert_eq!(camel_to_snake("simpleXML"), "simple_xml");
/// ```
pub fn camel_to_snake(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut result = String::with_capacity(name.len() + 4);

    for (i, &ch) in chars.iter().enumerate() {
        if ch.is_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let next = chars.get(i + 1).copied();

            // Insert underscore before:
            // 1. An uppercase letter preceded by a lowercase letter (camelCase boundary)
            // 2. An uppercase letter followed by a lowercase letter, when preceded
            //    by an uppercase letter (end of acronym: "HTMLParser" -> "html_parser")
            if prev.is_lowercase()
                || (prev.is_uppercase() && next.is_some_and(|c| c.is_lowercase()))
            {
                result.push('_');
            }
        }
        result.push(ch.to_ascii_lowercase());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── camel_to_snake tests ──────────────────────────────────────────

    #[test]
    fn test_camel_to_snake_basic() {
        assert_eq!(camel_to_snake("camelCase"), "camel_case");
    }

    #[test]
    fn test_camel_to_snake_multiple_words() {
        assert_eq!(camel_to_snake("systemPrompt"), "system_prompt");
        assert_eq!(camel_to_snake("contextWindow"), "context_window");
        assert_eq!(
            camel_to_snake("memoryConsolidation"),
            "memory_consolidation"
        );
    }

    #[test]
    fn test_camel_to_snake_already_snake() {
        assert_eq!(camel_to_snake("already_snake"), "already_snake");
    }

    #[test]
    fn test_camel_to_snake_single_word() {
        assert_eq!(camel_to_snake("model"), "model");
    }

    #[test]
    fn test_camel_to_snake_all_upper() {
        // Pure acronym stays together (no trailing lowercase to split on)
        assert_eq!(camel_to_snake("HTML"), "html");
    }

    #[test]
    fn test_camel_to_snake_acronym_then_word() {
        // Acronym followed by a word: split before the last uppercase
        assert_eq!(camel_to_snake("HTMLParser"), "html_parser");
        assert_eq!(camel_to_snake("getHTMLParser"), "get_html_parser");
        assert_eq!(camel_to_snake("simpleXML"), "simple_xml");
        assert_eq!(camel_to_snake("XMLHTTPRequest"), "xmlhttp_request");
    }

    #[test]
    fn test_camel_to_snake_leading_upper() {
        // Leading uppercase is just lowered, no underscore
        assert_eq!(camel_to_snake("Config"), "config");
    }

    #[test]
    fn test_camel_to_snake_empty() {
        assert_eq!(camel_to_snake(""), "");
    }

    // ── normalize_keys tests ──────────────────────────────────────────

    #[test]
    fn test_normalize_keys_flat_object() {
        let input = json!({
            "systemPrompt": "hello",
            "contextWindow": 4096
        });
        let expected = json!({
            "system_prompt": "hello",
            "context_window": 4096
        });
        assert_eq!(normalize_keys(input), expected);
    }

    #[test]
    fn test_normalize_keys_nested_object() {
        let input = json!({
            "agentsConfig": {
                "defaultModel": "gpt-4",
                "maxTokens": 1024
            }
        });
        let expected = json!({
            "agents_config": {
                "default_model": "gpt-4",
                "max_tokens": 1024
            }
        });
        assert_eq!(normalize_keys(input), expected);
    }

    #[test]
    fn test_normalize_keys_array() {
        let input = json!([
            {"firstName": "Alice"},
            {"firstName": "Bob"}
        ]);
        let expected = json!([
            {"first_name": "Alice"},
            {"first_name": "Bob"}
        ]);
        assert_eq!(normalize_keys(input), expected);
    }

    #[test]
    fn test_normalize_keys_primitives_unchanged() {
        assert_eq!(normalize_keys(json!(42)), json!(42));
        assert_eq!(normalize_keys(json!("hello")), json!("hello"));
        assert_eq!(normalize_keys(json!(true)), json!(true));
        assert_eq!(normalize_keys(json!(null)), json!(null));
    }

    #[test]
    fn test_normalize_keys_empty_object() {
        assert_eq!(normalize_keys(json!({})), json!({}));
    }

    // ── discover_config_path tests ────────────────────────────────────

    /// A minimal mock environment for config discovery tests.
    struct MockEnv {
        vars: std::collections::HashMap<String, String>,
    }

    impl MockEnv {
        fn new() -> Self {
            Self {
                vars: std::collections::HashMap::new(),
            }
        }

        fn with_var(mut self, key: &str, value: &str) -> Self {
            self.vars.insert(key.to_string(), value.to_string());
            self
        }
    }

    impl super::super::env::Environment for MockEnv {
        fn get_var(&self, name: &str) -> Option<String> {
            self.vars.get(name).cloned()
        }

        fn set_var(&self, _name: &str, _value: &str) {
            // No-op for mock
        }

        fn remove_var(&self, _name: &str) {
            // No-op for mock
        }
    }

    #[test]
    fn test_discover_env_var_takes_precedence() {
        let env = MockEnv::new().with_var("CLAWFT_CONFIG", "/custom/config.json");
        let result = discover_config_path(&env, Some(PathBuf::from("/home/user")));
        assert_eq!(result, Some(PathBuf::from("/custom/config.json")));
    }

    #[test]
    fn test_discover_no_home_no_env() {
        let env = MockEnv::new();
        let result = discover_config_path(&env, None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_discover_home_but_no_files() {
        // When home dir is given but no config files exist on disk,
        // discover returns None (both .clawft and .nanobot paths don't exist).
        let env = MockEnv::new();
        // Use a path that definitely does not contain config files.
        let result = discover_config_path(
            &env,
            Some(PathBuf::from("/tmp/clawft_test_nonexistent_home")),
        );
        assert_eq!(result, None);
    }

    // ── Workspace overlay tests (Layer 3 of load_config_raw) ─────────
    //
    // The minimal MockFs below only implements the methods load_config_raw
    // calls (`exists`, `read_to_string`, `home_dir`). Every other method
    // panics with `unimplemented!()` so an accidental call in a future
    // refactor surfaces immediately instead of silently no-oping.

    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Mutex;

    struct MockFs {
        home: Option<PathBuf>,
        files: Mutex<HashMap<PathBuf, String>>,
    }

    impl MockFs {
        fn new(home: Option<PathBuf>) -> Self {
            Self {
                home,
                files: Mutex::new(HashMap::new()),
            }
        }

        fn with_file(self, path: impl Into<PathBuf>, contents: &str) -> Self {
            self.files
                .lock()
                .unwrap()
                .insert(path.into(), contents.to_string());
            self
        }
    }

    #[async_trait]
    impl super::super::fs::FileSystem for MockFs {
        async fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
            self.files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "not found"))
        }
        async fn write_string(&self, _: &Path, _: &str) -> std::io::Result<()> {
            unimplemented!()
        }
        async fn append_string(&self, _: &Path, _: &str) -> std::io::Result<()> {
            unimplemented!()
        }
        async fn exists(&self, path: &Path) -> bool {
            self.files.lock().unwrap().contains_key(path)
        }
        async fn list_dir(&self, _: &Path) -> std::io::Result<Vec<PathBuf>> {
            unimplemented!()
        }
        async fn create_dir_all(&self, _: &Path) -> std::io::Result<()> {
            unimplemented!()
        }
        async fn remove_file(&self, _: &Path) -> std::io::Result<()> {
            unimplemented!()
        }
        fn home_dir(&self) -> Option<PathBuf> {
            self.home.clone()
        }
    }

    fn workspace_config_path() -> PathBuf {
        PathBuf::from(".clawft").join("config.json")
    }

    // The home-layer (Layer 2) goes through `discover_config_path`,
    // which uses a sync `Path::exists()` against the *real* filesystem
    // — it cannot be exercised with a mock `FileSystem`. These tests
    // therefore focus on the new workspace overlay (Layer 3), which
    // does flow through `fs.exists()` / `fs.read_to_string()` and is
    // mockable. End-to-end behaviour (workspace overrides home with
    // both layers populated) is covered by the live smoke test.

    #[tokio::test]
    async fn workspace_overlay_applied_when_present() {
        // Workspace declares an agent.chat channel at level 2; with
        // no other layer present, the overlay must surface it.
        let workspace = r#"{
            "routing": {
                "permissions": {
                    "channels": { "agent.chat": { "level": 2 } }
                }
            }
        }"#;
        // Home dir set to a path that won't exist on the real fs so
        // discover_config_path returns None (Layer 2 inert).
        let fs = MockFs::new(Some(PathBuf::from("/tmp/clawft_test_no_home_xyz")))
            .with_file(workspace_config_path(), workspace);
        let env = MockEnv::new();
        let merged = load_config_raw(&fs, &env).await.unwrap();
        let lvl = merged
            .pointer("/routing/permissions/channels/agent.chat/level")
            .and_then(Value::as_u64)
            .expect("level present after overlay");
        assert_eq!(lvl, 2);
    }

    #[tokio::test]
    async fn workspace_overlay_skipped_when_absent() {
        // No workspace config + no home config (real fs path missing)
        // = empty merged result, behaviour matches pre-fix.
        let fs = MockFs::new(Some(PathBuf::from("/tmp/clawft_test_no_home_xyz")));
        let env = MockEnv::new();
        let merged = load_config_raw(&fs, &env).await.unwrap();
        assert!(
            merged.as_object().map(|m| m.is_empty()).unwrap_or(true),
            "no layers present should yield empty merged config"
        );
    }

    #[tokio::test]
    async fn workspace_overlay_invalid_json_is_ignored() {
        // Malformed workspace JSON must not abort load_config_raw —
        // the overlay is best-effort. Other layers continue to apply.
        let fs = MockFs::new(Some(PathBuf::from("/tmp/clawft_test_no_home_xyz")))
            .with_file(workspace_config_path(), "{ this is not json");
        let env = MockEnv::new();
        let merged = load_config_raw(&fs, &env).await.unwrap();
        assert!(merged.is_object(), "loader returns ok despite parse failure");
    }

    #[tokio::test]
    async fn workspace_overlay_keys_are_normalized_to_snake_case() {
        // Workspace JSON uses camelCase (matches the existing config
        // file convention); load_config_raw normalizes via
        // `normalize_keys` before merging.
        let workspace = r#"{ "agentDefaults": { "maxTokens": 8192 } }"#;
        let fs = MockFs::new(Some(PathBuf::from("/tmp/clawft_test_no_home_xyz")))
            .with_file(workspace_config_path(), workspace);
        let env = MockEnv::new();
        let merged = load_config_raw(&fs, &env).await.unwrap();
        assert_eq!(
            merged
                .pointer("/agent_defaults/max_tokens")
                .and_then(Value::as_u64),
            Some(8192)
        );
    }
}
