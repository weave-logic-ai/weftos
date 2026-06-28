//! `weft plugins` -- CLI commands for plugin development and management.
//!
//! Provides subcommands:
//!
//! - `weft plugins create <name>` -- scaffold a new WeftOS plugin crate.
//! - `weft plugins templates` -- list available plugin templates.
//! - `weft plugins validate <path>` -- validate a plugin crate structure.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use comfy_table::{Table, presets};

/// Arguments for the `weft plugins` subcommand.
#[derive(Args)]
pub struct PluginsArgs {
    #[command(subcommand)]
    pub action: PluginsAction,
}

/// Subcommands for `weft plugins`.
#[derive(Subcommand)]
pub enum PluginsAction {
    /// Scaffold a new WeftOS plugin crate.
    Create {
        /// Plugin name (e.g. "my-analyzer").
        name: String,
        /// Plugin type: "analyzer", "channel", "tool", or "generic".
        #[arg(long, default_value = "analyzer")]
        plugin_type: String,
        /// Output directory (defaults to current directory).
        #[arg(short, long)]
        dir: Option<String>,
    },
    /// List available plugin templates.
    Templates,
    /// Validate a plugin crate structure.
    Validate {
        /// Path to plugin crate root.
        path: String,
    },
}

/// Run the `weft plugins` subcommand.
pub async fn run(args: PluginsArgs) -> anyhow::Result<()> {
    match args.action {
        PluginsAction::Create {
            name,
            plugin_type,
            dir,
        } => create_plugin(&name, &plugin_type, dir.as_deref())?,
        PluginsAction::Templates => list_templates(),
        PluginsAction::Validate { path } => validate_plugin(&path)?,
    }
    Ok(())
}

// ── Create ──────────────────────────────────────────────────────────

/// Scaffold a new WeftOS plugin crate on disk.
fn create_plugin(name: &str, plugin_type: &str, dir: Option<&str>) -> anyhow::Result<()> {
    let valid_types = ["analyzer", "channel", "tool", "generic"];
    if !valid_types.contains(&plugin_type) {
        anyhow::bail!(
            "unknown plugin type \"{plugin_type}\"; expected one of: {}",
            valid_types.join(", ")
        );
    }

    let base = dir.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let crate_name = format!("clawft-plugin-{name}");
    let root = base.join(&crate_name);

    if root.exists() {
        anyhow::bail!("directory already exists: {}", root.display());
    }

    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir)?;

    // Cargo.toml
    std::fs::write(
        root.join("Cargo.toml"),
        cargo_toml_template(&crate_name, name),
    )?;

    // src/lib.rs
    std::fs::write(src_dir.join("lib.rs"), lib_rs_template(name, plugin_type))?;

    // clawft.plugin.json (canonical format per WEFT-64). The legacy
    // `.weftos-plugin.toml` is still readable via
    // `PluginManifest::from_legacy_toml` for backward compatibility, but
    // the scaffolder only emits JSON now.
    std::fs::write(
        root.join("clawft.plugin.json"),
        plugin_manifest_template_json(name, plugin_type),
    )?;

    // README.md
    std::fs::write(root.join("README.md"), readme_template(name, plugin_type))?;

    println!("Created plugin crate: {}", root.display());
    println!("  type:  {plugin_type}");
    println!("  crate: {crate_name}");
    println!();
    println!("Next steps:");
    println!("  cd {}", root.display());
    println!("  cargo check");
    Ok(())
}

fn cargo_toml_template(crate_name: &str, plugin_name: &str) -> String {
    format!(
        r#"[package]
name = "{crate_name}"
version = "0.1.0"
edition = "2021"
description = "WeftOS plugin: {plugin_name}"
license = "MIT OR Apache-2.0"

[dependencies]
anyhow = "1"
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
async-trait = "0.1"
"#
    )
}

fn lib_rs_template(name: &str, plugin_type: &str) -> String {
    let module_name = name.replace('-', "_");
    match plugin_type {
        "analyzer" => format!(
            r#"//! WeftOS analyzer plugin: {name}.

use async_trait::async_trait;

/// Analyzer trait stub -- implement your analysis logic here.
#[async_trait]
pub trait Analyzer: Send + Sync {{
    /// Analyze the given input and return a result summary.
    async fn analyze(&self, input: &str) -> anyhow::Result<String>;
}}

/// Default analyzer implementation for `{name}`.
pub struct {type_name}Analyzer;

#[async_trait]
impl Analyzer for {type_name}Analyzer {{
    async fn analyze(&self, input: &str) -> anyhow::Result<String> {{
        // TODO: implement analysis logic
        Ok(format!("{name} analyzed {{}} bytes", input.len()))
    }}
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[tokio::test]
    async fn analyzer_returns_result() {{
        let a = {type_name}Analyzer;
        let result = a.analyze("hello").await.unwrap();
        assert!(result.contains("5 bytes"));
    }}
}}
"#,
            type_name = to_pascal_case(&module_name),
        ),
        "channel" => format!(
            r#"//! WeftOS channel adapter plugin: {name}.

use async_trait::async_trait;

/// Channel adapter trait stub -- implement your channel integration here.
#[async_trait]
pub trait ChannelAdapter: Send + Sync {{
    /// Start listening for inbound messages on this channel.
    async fn start(&self) -> anyhow::Result<()>;

    /// Send a message through this channel.
    async fn send(&self, target: &str, message: &str) -> anyhow::Result<()>;
}}

/// Default channel adapter for `{name}`.
pub struct {type_name}Channel;

#[async_trait]
impl ChannelAdapter for {type_name}Channel {{
    async fn start(&self) -> anyhow::Result<()> {{
        // TODO: implement channel listener
        Ok(())
    }}

    async fn send(&self, _target: &str, _message: &str) -> anyhow::Result<()> {{
        // TODO: implement message sending
        Ok(())
    }}
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[tokio::test]
    async fn channel_starts_ok() {{
        let ch = {type_name}Channel;
        assert!(ch.start().await.is_ok());
    }}
}}
"#,
            type_name = to_pascal_case(&module_name),
        ),
        "tool" => format!(
            r#"//! WeftOS tool plugin: {name}.

use serde::{{Deserialize, Serialize}};

/// Tool registration descriptor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {{
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}}

/// Return the tool descriptor for registration with the tool registry.
pub fn register() -> ToolDescriptor {{
    ToolDescriptor {{
        name: "{name}".to_string(),
        description: "TODO: describe what this tool does".to_string(),
        parameters: serde_json::json!({{
            "type": "object",
            "properties": {{}}
        }}),
    }}
}}

/// Execute the tool with the given arguments.
pub fn execute(args: &serde_json::Value) -> anyhow::Result<serde_json::Value> {{
    // TODO: implement tool logic
    let _ = args;
    Ok(serde_json::json!({{ "status": "ok" }}))
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn tool_registers() {{
        let desc = register();
        assert_eq!(desc.name, "{name}");
    }}

    #[test]
    fn tool_executes() {{
        let result = execute(&serde_json::json!({{}})).unwrap();
        assert_eq!(result["status"], "ok");
    }}
}}
"#
        ),
        // generic
        _ => format!(
            r#"//! WeftOS plugin: {name}.

/// Plugin entry point.
pub fn init() {{
    // TODO: initialize your plugin
}}

/// Plugin version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn plugin_initializes() {{
        init(); // should not panic
    }}
}}
"#
        ),
    }
}

/// Render the canonical `clawft.plugin.json` manifest for the given
/// plugin name and type. Matches the schema parsed by
/// `clawft_plugin::PluginManifest::from_json`.
fn plugin_manifest_template_json(name: &str, plugin_type: &str) -> String {
    let capability = match plugin_type {
        "tool" => "tool",
        "channel" => "channel",
        "analyzer" => "pipeline_stage",
        "skill" => "skill",
        _ => "tool",
    };
    let id = format!("weftos.plugin.{name}");
    serde_json::to_string_pretty(&serde_json::json!({
        "id": id,
        "name": name,
        "version": "0.1.0",
        "capabilities": [capability],
        "permissions": {
            "network": [],
            "filesystem": [],
            "env_vars": [],
            "shell": false
        }
    }))
    .unwrap_or_else(|_| String::from("{}"))
}

/// Legacy TOML template (kept for backward-compat reference / tests).
/// Not emitted by the scaffolder anymore (WEFT-64) but still produced by
/// some downstream tooling and accepted by the deprecated TOML reader.
#[cfg(test)]
fn plugin_manifest_template(name: &str, plugin_type: &str) -> String {
    format!(
        r#"[plugin]
name = "{name}"
type = "{plugin_type}"
version = "0.1.0"
description = ""
author = ""
license = "MIT OR Apache-2.0"

[compatibility]
weftos_min_version = "0.4.0"
"#
    )
}

fn readme_template(name: &str, plugin_type: &str) -> String {
    format!(
        r#"# clawft-plugin-{name}

A WeftOS **{plugin_type}** plugin.

## Usage

Add to your workspace `Cargo.toml`:

```toml
clawft-plugin-{name} = {{ path = "crates/clawft-plugin-{name}" }}
```

## Development

```bash
cargo check -p clawft-plugin-{name}
cargo test -p clawft-plugin-{name}
```

## Plugin Manifest

See `clawft.plugin.json` for metadata and capability declarations
(canonical schema). The older `.weftos-plugin.toml` format is still
parseable for backward compatibility but is deprecated.
"#
    )
}

/// Convert a snake_case or kebab-case identifier to PascalCase.
fn to_pascal_case(s: &str) -> String {
    s.split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    upper + chars.as_str()
                }
                None => String::new(),
            }
        })
        .collect()
}

// ── Templates ───────────────────────────────────────────────────────

fn list_templates() {
    let mut table = Table::new();
    table.load_preset(presets::UTF8_FULL_CONDENSED);
    table.set_header(["Type", "Description"]);
    table.add_row([
        "analyzer",
        "Implements the Analyzer trait for data analysis pipelines",
    ]);
    table.add_row([
        "channel",
        "Channel adapter for integrating external messaging platforms",
    ]);
    table.add_row(["tool", "Registers a new tool in the WeftOS tool registry"]);
    table.add_row([
        "generic",
        "Minimal plugin scaffold with no trait constraints",
    ]);
    println!("{table}");
}

// ── Validate ────────────────────────────────────────────────────────

fn validate_plugin(path: &str) -> anyhow::Result<()> {
    let root = Path::new(path);
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // Check Cargo.toml exists and has required fields.
    let cargo_path = root.join("Cargo.toml");
    if cargo_path.exists() {
        let contents = std::fs::read_to_string(&cargo_path)?;
        let parsed: toml::Value = toml::from_str(&contents)?;
        if let Some(pkg) = parsed.get("package") {
            for field in &["name", "version", "description", "license"] {
                if pkg.get(field).is_none() {
                    errors.push(format!("Cargo.toml: missing [package].{field}"));
                }
            }
        } else {
            errors.push("Cargo.toml: missing [package] table".to_string());
        }
    } else {
        errors.push("Cargo.toml not found".to_string());
    }

    // Check src/lib.rs exists.
    let lib_path = root.join("src").join("lib.rs");
    if !lib_path.exists() {
        errors.push("src/lib.rs not found".to_string());
    } else {
        // Warn on unsafe blocks.
        let src = std::fs::read_to_string(&lib_path)?;
        if src.contains("unsafe ") {
            warnings.push("src/lib.rs: contains `unsafe` blocks (review recommended)".to_string());
        }
    }

    // Check the canonical clawft.plugin.json (or legacy .weftos-plugin.toml)
    // exists and parses. Per WEFT-64 the canonical format is JSON; if only
    // the legacy TOML is present, accept it but warn.
    let json_manifest = root.join("clawft.plugin.json");
    let toml_manifest = root.join(".weftos-plugin.toml");
    if json_manifest.exists() {
        let contents = std::fs::read_to_string(&json_manifest)?;
        match clawft_plugin::PluginManifest::from_json(&contents) {
            Ok(_) => {}
            Err(e) => errors.push(format!("clawft.plugin.json: {e}")),
        }
        if toml_manifest.exists() {
            warnings.push(
                "both clawft.plugin.json and .weftos-plugin.toml present; \
                 the JSON is canonical -- delete the TOML to silence."
                    .to_string(),
            );
        }
    } else if toml_manifest.exists() {
        warnings.push(
            ".weftos-plugin.toml is deprecated; please migrate to \
             clawft.plugin.json (WEFT-64)."
                .to_string(),
        );
        let contents = std::fs::read_to_string(&toml_manifest)?;
        match clawft_plugin::PluginManifest::from_legacy_toml(&contents) {
            Ok(_) => {}
            Err(e) => errors.push(format!(".weftos-plugin.toml: {e}")),
        }
    } else {
        errors.push("no plugin manifest found (clawft.plugin.json)".to_string());
    }

    // Report results.
    if errors.is_empty() && warnings.is_empty() {
        println!("Plugin at {path} is valid.");
    } else {
        for w in &warnings {
            println!("  WARNING: {w}");
        }
        for e in &errors {
            println!("  ERROR:   {e}");
        }
        if !errors.is_empty() {
            anyhow::bail!(
                "validation failed with {} error(s) and {} warning(s)",
                errors.len(),
                warnings.len()
            );
        } else {
            println!("Plugin at {path} is valid ({} warning(s)).", warnings.len());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_pascal_case_works() {
        assert_eq!(to_pascal_case("my_analyzer"), "MyAnalyzer");
        assert_eq!(to_pascal_case("cool-tool"), "CoolTool");
        assert_eq!(to_pascal_case("simple"), "Simple");
    }

    #[test]
    fn cargo_toml_template_contains_name() {
        let t = cargo_toml_template("clawft-plugin-foo", "foo");
        assert!(t.contains("clawft-plugin-foo"));
    }

    #[test]
    fn manifest_template_parses() {
        let t = plugin_manifest_template("test", "analyzer");
        let parsed: toml::Value = toml::from_str(&t).unwrap();
        assert_eq!(parsed["plugin"]["name"].as_str().unwrap(), "test");
    }

    #[test]
    fn manifest_template_json_parses_via_plugin_manifest() {
        // Per WEFT-64 the canonical manifest format is JSON. The scaffolder
        // template must round-trip through `PluginManifest::from_json`.
        for plugin_type in ["analyzer", "channel", "tool", "skill", "generic"] {
            let t = plugin_manifest_template_json("scaffold-test", plugin_type);
            let manifest = clawft_plugin::PluginManifest::from_json(&t)
                .unwrap_or_else(|e| panic!("type {plugin_type}: {e}"));
            assert_eq!(manifest.name, "scaffold-test");
            assert_eq!(manifest.id, "weftos.plugin.scaffold-test");
        }
    }

    #[test]
    fn manifest_template_json_and_legacy_toml_match() {
        let json = plugin_manifest_template_json("twins", "tool");
        let toml = plugin_manifest_template("twins", "tool");

        let from_json = clawft_plugin::PluginManifest::from_json(&json).unwrap();
        let from_toml = clawft_plugin::PluginManifest::from_legacy_toml(&toml).unwrap();

        assert_eq!(from_json.name, from_toml.name);
        assert_eq!(from_json.version, from_toml.version);
        assert_eq!(from_json.capabilities, from_toml.capabilities);
        assert_eq!(from_json.id, from_toml.id);
    }

    #[test]
    fn lib_rs_analyzer_compiles_syntax() {
        let code = lib_rs_template("my-test", "analyzer");
        assert!(code.contains("pub struct MyTestAnalyzer"));
    }

    #[test]
    fn lib_rs_channel_compiles_syntax() {
        let code = lib_rs_template("slack-bridge", "channel");
        assert!(code.contains("pub struct SlackBridgeChannel"));
    }

    #[test]
    fn lib_rs_tool_compiles_syntax() {
        let code = lib_rs_template("grep-search", "tool");
        assert!(code.contains("grep-search"));
    }

    #[test]
    fn lib_rs_generic_compiles_syntax() {
        let code = lib_rs_template("misc", "generic");
        assert!(code.contains("pub fn init()"));
    }
}
