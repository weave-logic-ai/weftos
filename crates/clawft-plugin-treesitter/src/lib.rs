//! Code analysis via tree-sitter tool plugin for clawft.
//!
//! Provides tools for AST parsing, symbol extraction, complexity metrics,
//! and tree-sitter queries using native tree-sitter grammars.
//!
//! # Language Support
//!
//! Each language is an optional feature. Enable the features you need:
//! - `rust` -- Rust (`.rs`)
//! - `typescript` -- TypeScript (`.ts`, `.tsx`)
//! - `python` -- Python (`.py`)
//! - `javascript` -- JavaScript (`.js`, `.jsx`, `.mjs`)
//!
//! # Native Only
//!
//! This crate uses native tree-sitter grammars (C FFI). There is no
//! WASM variant. This keeps parsing fast and memory-safe under the
//! tree-sitter C runtime.
//!
//! # Feature Flag
//!
//! This crate is gated behind the workspace `plugin-treesitter` feature flag.

pub mod analysis;
pub mod types;

use async_trait::async_trait;
use clawft_plugin::{PluginError, Tool, ToolContext};

use analysis::{
    available_languages, calculate_complexity, extract_symbols, is_language_available,
    parse_source, tree_to_ast,
};
use types::{Language, TreeSitterConfig};

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn parse_language(params: &serde_json::Value) -> Result<Language, PluginError> {
    let lang_str = params
        .get("language")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PluginError::ExecutionFailed("language is required".into()))?;

    let language = Language::parse(lang_str).ok_or_else(|| {
        PluginError::ExecutionFailed(format!("unsupported language: '{lang_str}'"))
    })?;

    if !is_language_available(language) {
        return Err(PluginError::ExecutionFailed(format!(
            "language '{lang_str}' feature not enabled at compile time"
        )));
    }

    Ok(language)
}

fn get_source(
    params: &serde_json::Value,
    config: &TreeSitterConfig,
) -> Result<String, PluginError> {
    let source = params
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PluginError::ExecutionFailed("source is required".into()))?;

    if source.len() > config.max_file_size {
        return Err(PluginError::ResourceExhausted(format!(
            "source exceeds max file size ({} > {} bytes)",
            source.len(),
            config.max_file_size
        )));
    }

    Ok(source.to_string())
}

// ---------------------------------------------------------------------------
// TsParseTool
// ---------------------------------------------------------------------------

/// Tool that parses source code and returns a simplified AST.
pub struct TsParseTool {
    config: TreeSitterConfig,
}

impl TsParseTool {
    pub fn new(config: TreeSitterConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for TsParseTool {
    fn name(&self) -> &str {
        "ts_parse"
    }

    fn description(&self) -> &str {
        "Parse source code and return a simplified AST"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "Source code to parse"
                },
                "language": {
                    "type": "string",
                    "description": "Programming language (rust, typescript, python, javascript)",
                    "enum": ["rust", "typescript", "python", "javascript"]
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum AST depth to return",
                    "default": 10
                }
            },
            "required": ["source", "language"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let language = parse_language(&params)?;
        let source = get_source(&params, &self.config)?;
        let max_depth = params
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.config.max_ast_depth as u64) as usize;

        let tree = parse_source(&source, language).map_err(PluginError::ExecutionFailed)?;
        let ast = tree_to_ast(&tree, max_depth);

        let has_errors = tree.root_node().has_error();

        Ok(serde_json::json!({
            "ast": serde_json::to_value(&ast).map_err(PluginError::from)?,
            "has_errors": has_errors,
            "language": language,
        }))
    }
}

// ---------------------------------------------------------------------------
// TsSymbolsTool
// ---------------------------------------------------------------------------

/// Tool that lists symbols (functions, structs, classes) in source code.
pub struct TsSymbolsTool {
    config: TreeSitterConfig,
}

impl TsSymbolsTool {
    pub fn new(config: TreeSitterConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for TsSymbolsTool {
    fn name(&self) -> &str {
        "ts_symbols"
    }

    fn description(&self) -> &str {
        "List functions, structs, classes, and other symbols in source code"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "Source code to analyze"
                },
                "language": {
                    "type": "string",
                    "description": "Programming language (rust, typescript, python, javascript)",
                    "enum": ["rust", "typescript", "python", "javascript"]
                }
            },
            "required": ["source", "language"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let language = parse_language(&params)?;
        let source = get_source(&params, &self.config)?;

        let tree = parse_source(&source, language).map_err(PluginError::ExecutionFailed)?;
        let symbols = extract_symbols(&tree, &source, language);

        serde_json::to_value(&symbols).map_err(PluginError::from)
    }
}

// ---------------------------------------------------------------------------
// TsComplexityTool
// ---------------------------------------------------------------------------

/// Tool that calculates cyclomatic complexity metrics.
pub struct TsComplexityTool {
    config: TreeSitterConfig,
}

impl TsComplexityTool {
    pub fn new(config: TreeSitterConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for TsComplexityTool {
    fn name(&self) -> &str {
        "ts_complexity"
    }

    fn description(&self) -> &str {
        "Calculate cyclomatic complexity metrics for source code"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "Source code to analyze"
                },
                "language": {
                    "type": "string",
                    "description": "Programming language (rust, typescript, python, javascript)",
                    "enum": ["rust", "typescript", "python", "javascript"]
                }
            },
            "required": ["source", "language"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let language = parse_language(&params)?;
        let source = get_source(&params, &self.config)?;

        let tree = parse_source(&source, language).map_err(PluginError::ExecutionFailed)?;
        let metrics = calculate_complexity(&tree, &source, language);

        serde_json::to_value(&metrics).map_err(PluginError::from)
    }
}

// ---------------------------------------------------------------------------
// TsLanguagesTool
// ---------------------------------------------------------------------------

/// Tool that lists available languages (those with features enabled).
pub struct TsLanguagesTool;

#[async_trait]
impl Tool for TsLanguagesTool {
    fn name(&self) -> &str {
        "ts_languages"
    }

    fn description(&self) -> &str {
        "List available tree-sitter languages (compile-time features)"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let langs = available_languages();
        Ok(serde_json::json!({
            "languages": langs,
        }))
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Create all tree-sitter tools with the given configuration.
pub fn all_treesitter_tools(config: TreeSitterConfig) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(TsParseTool::new(config.clone())),
        Box::new(TsSymbolsTool::new(config.clone())),
        Box::new(TsComplexityTool::new(config)),
        Box::new(TsLanguagesTool),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_plugin::KeyValueStore;

    struct MockKvStore;

    #[async_trait]
    impl KeyValueStore for MockKvStore {
        async fn get(&self, _key: &str) -> Result<Option<String>, PluginError> {
            Ok(None)
        }
        async fn set(&self, _key: &str, _value: &str) -> Result<(), PluginError> {
            Ok(())
        }
        async fn delete(&self, _key: &str) -> Result<bool, PluginError> {
            Ok(false)
        }
        async fn list_keys(&self, _prefix: Option<&str>) -> Result<Vec<String>, PluginError> {
            Ok(vec![])
        }
    }

    struct MockToolContext;

    impl ToolContext for MockToolContext {
        fn key_value_store(&self) -> &dyn KeyValueStore {
            &MockKvStore
        }
        fn plugin_id(&self) -> &str {
            "clawft-plugin-treesitter"
        }
        fn agent_id(&self) -> &str {
            "test-agent"
        }
    }

    #[test]
    fn all_tools_returns_four() {
        let tools = all_treesitter_tools(TreeSitterConfig::default());
        assert_eq!(tools.len(), 4);

        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"ts_parse"));
        assert!(names.contains(&"ts_symbols"));
        assert!(names.contains(&"ts_complexity"));
        assert!(names.contains(&"ts_languages"));
    }

    #[test]
    fn tool_descriptions_non_empty() {
        let tools = all_treesitter_tools(TreeSitterConfig::default());
        for tool in &tools {
            assert!(
                !tool.description().is_empty(),
                "empty description for {}",
                tool.name()
            );
        }
    }

    #[test]
    fn tool_schemas_are_objects() {
        let tools = all_treesitter_tools(TreeSitterConfig::default());
        for tool in &tools {
            let schema = tool.parameters_schema();
            assert!(schema.is_object(), "schema not object for {}", tool.name());
            assert_eq!(schema["type"], "object");
        }
    }

    #[tokio::test]
    async fn languages_tool_returns_list() {
        let tool = TsLanguagesTool;
        let ctx = MockToolContext;
        let result = tool.execute(serde_json::json!({}), &ctx).await.unwrap();
        assert!(result.get("languages").is_some());
        assert!(result["languages"].is_array());
    }

    #[tokio::test]
    async fn parse_rejects_too_large_source() {
        let config = TreeSitterConfig {
            max_file_size: 10,
            max_ast_depth: 5,
        };
        let tool = TsParseTool::new(config);
        let ctx = MockToolContext;

        let params = serde_json::json!({
            "source": "a".repeat(100),
            "language": "rust"
        });

        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        // Without the "rust" feature, the language check fails first.
        // With the "rust" feature, the size check triggers ResourceExhausted.
        if cfg!(feature = "rust") {
            let err = result.unwrap_err();
            assert!(
                matches!(err, PluginError::ResourceExhausted(_)),
                "expected ResourceExhausted, got: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn parse_rejects_unknown_language() {
        let tool = TsParseTool::new(TreeSitterConfig::default());
        let ctx = MockToolContext;

        let params = serde_json::json!({
            "source": "hello",
            "language": "cobol"
        });

        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unsupported language"), "unexpected: {err}");
    }

    #[cfg(feature = "rust")]
    mod rust_tool_tests {
        use super::*;

        const RUST_CODE: &str = r#"
fn hello() -> String {
    "hello".to_string()
}

struct Point {
    x: f64,
    y: f64,
}
"#;

        #[tokio::test]
        async fn parse_tool_rust() {
            let tool = TsParseTool::new(TreeSitterConfig::default());
            let ctx = MockToolContext;

            let params = serde_json::json!({
                "source": RUST_CODE,
                "language": "rust"
            });

            let result = tool.execute(params, &ctx).await.unwrap();
            assert_eq!(result["has_errors"], false);
            assert!(result["ast"].is_object());
        }

        #[tokio::test]
        async fn symbols_tool_rust() {
            let tool = TsSymbolsTool::new(TreeSitterConfig::default());
            let ctx = MockToolContext;

            let params = serde_json::json!({
                "source": RUST_CODE,
                "language": "rust"
            });

            let result = tool.execute(params, &ctx).await.unwrap();
            let symbols: Vec<serde_json::Value> = serde_json::from_value(result).unwrap();
            let names: Vec<&str> = symbols.iter().filter_map(|s| s["name"].as_str()).collect();
            assert!(names.contains(&"hello"), "missing 'hello': {names:?}");
            assert!(names.contains(&"Point"), "missing 'Point': {names:?}");
        }

        #[tokio::test]
        async fn complexity_tool_rust() {
            let tool = TsComplexityTool::new(TreeSitterConfig::default());
            let ctx = MockToolContext;

            let params = serde_json::json!({
                "source": RUST_CODE,
                "language": "rust"
            });

            let result = tool.execute(params, &ctx).await.unwrap();
            assert!(result["function_count"].as_u64().unwrap() >= 1);
            assert!(result["total_complexity"].as_u64().unwrap() >= 1);
        }
    }
}
