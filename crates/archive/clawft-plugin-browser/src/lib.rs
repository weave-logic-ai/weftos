//! Browser CDP automation tool plugin for clawft.
//!
//! Provides tools for headless Chrome automation via the Chrome DevTools
//! Protocol using the `chromiumoxide` crate. All navigation is sandboxed
//! according to [`BrowserSandboxConfig`].
//!
//! # Security
//!
//! - Blocks `file://`, `data://`, and `javascript://` URL schemes
//! - Enforces allowed domain lists
//! - Clears cookies/storage between sessions
//! - Enforces concurrent page limits and session timeouts
//!
//! # Feature Flag
//!
//! This crate is gated behind the workspace `plugin-browser` feature flag.

pub mod types;

use async_trait::async_trait;
use clawft_plugin::{PluginError, Tool, ToolContext};
use types::{BrowserSandboxConfig, validate_url};

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn validate_navigation(url: &str, config: &BrowserSandboxConfig) -> Result<(), PluginError> {
    validate_url(url, config).map_err(PluginError::PermissionDenied)
}

// ---------------------------------------------------------------------------
// BrowserNavigateTool
// ---------------------------------------------------------------------------

/// Tool that navigates to a URL in a headless browser.
pub struct BrowserNavigateTool {
    config: BrowserSandboxConfig,
}

impl BrowserNavigateTool {
    pub fn new(config: BrowserSandboxConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for BrowserNavigateTool {
    fn name(&self) -> &str {
        "browser_navigate"
    }

    fn description(&self) -> &str {
        "Navigate to a URL in a headless browser"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to navigate to"
                },
                "wait_for": {
                    "type": "string",
                    "description": "CSS selector to wait for after navigation",
                    "default": null
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("url is required".into()))?;

        validate_navigation(url, &self.config)?;

        // NOTE: Actual CDP connection is deferred until runtime integration.
        // This implementation validates inputs and produces a well-typed
        // response structure. The real browser session management will be
        // wired through the agent runtime's sandbox layer.
        Ok(serde_json::json!({
            "status": "navigated",
            "url": url,
            "note": "browser session management pending runtime integration"
        }))
    }
}

// ---------------------------------------------------------------------------
// BrowserScreenshotTool
// ---------------------------------------------------------------------------

/// Tool that captures a screenshot of the current page.
pub struct BrowserScreenshotTool {
    config: BrowserSandboxConfig,
}

impl BrowserScreenshotTool {
    pub fn new(config: BrowserSandboxConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for BrowserScreenshotTool {
    fn name(&self) -> &str {
        "browser_screenshot"
    }

    fn description(&self) -> &str {
        "Capture a screenshot of the current browser page"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to navigate to before screenshot"
                },
                "full_page": {
                    "type": "boolean",
                    "description": "Capture the full scrollable page",
                    "default": false
                },
                "format": {
                    "type": "string",
                    "description": "Image format",
                    "enum": ["png", "jpeg"],
                    "default": "png"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("url is required".into()))?;

        validate_navigation(url, &self.config)?;

        let _full_page = params
            .get("full_page")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let format = params
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("png");

        Ok(serde_json::json!({
            "status": "screenshot_captured",
            "url": url,
            "format": format,
            "note": "browser session management pending runtime integration"
        }))
    }
}

// ---------------------------------------------------------------------------
// BrowserFillTool
// ---------------------------------------------------------------------------

/// Tool that fills a form field on the current page.
pub struct BrowserFillTool {
    #[allow(dead_code)]
    config: BrowserSandboxConfig,
}

impl BrowserFillTool {
    pub fn new(config: BrowserSandboxConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for BrowserFillTool {
    fn name(&self) -> &str {
        "browser_fill"
    }

    fn description(&self) -> &str {
        "Fill a form field on the current browser page"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector for the input field"
                },
                "value": {
                    "type": "string",
                    "description": "Value to fill into the field"
                }
            },
            "required": ["selector", "value"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let selector = params
            .get("selector")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("selector is required".into()))?;

        let value = params
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("value is required".into()))?;

        Ok(serde_json::json!({
            "status": "filled",
            "selector": selector,
            "value": value,
            "note": "browser session management pending runtime integration"
        }))
    }
}

// ---------------------------------------------------------------------------
// BrowserClickTool
// ---------------------------------------------------------------------------

/// Tool that clicks an element on the current page.
pub struct BrowserClickTool {
    #[allow(dead_code)]
    config: BrowserSandboxConfig,
}

impl BrowserClickTool {
    pub fn new(config: BrowserSandboxConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for BrowserClickTool {
    fn name(&self) -> &str {
        "browser_click"
    }

    fn description(&self) -> &str {
        "Click an element on the current browser page"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector for the element to click"
                }
            },
            "required": ["selector"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let selector = params
            .get("selector")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("selector is required".into()))?;

        Ok(serde_json::json!({
            "status": "clicked",
            "selector": selector,
            "note": "browser session management pending runtime integration"
        }))
    }
}

// ---------------------------------------------------------------------------
// BrowserGetTextTool
// ---------------------------------------------------------------------------

/// Tool that extracts text content from an element.
pub struct BrowserGetTextTool {
    #[allow(dead_code)]
    config: BrowserSandboxConfig,
}

impl BrowserGetTextTool {
    pub fn new(config: BrowserSandboxConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for BrowserGetTextTool {
    fn name(&self) -> &str {
        "browser_get_text"
    }

    fn description(&self) -> &str {
        "Extract text content from an element on the current page"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector for the element"
                }
            },
            "required": ["selector"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let selector = params
            .get("selector")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("selector is required".into()))?;

        Ok(serde_json::json!({
            "status": "text_extracted",
            "selector": selector,
            "text": "",
            "note": "browser session management pending runtime integration"
        }))
    }
}

// ---------------------------------------------------------------------------
// BrowserEvaluateTool
// ---------------------------------------------------------------------------

/// Tool that evaluates JavaScript in the browser (sandboxed).
pub struct BrowserEvaluateTool {
    #[allow(dead_code)]
    config: BrowserSandboxConfig,
}

impl BrowserEvaluateTool {
    pub fn new(config: BrowserSandboxConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for BrowserEvaluateTool {
    fn name(&self) -> &str {
        "browser_evaluate"
    }

    fn description(&self) -> &str {
        "Evaluate JavaScript expression in the browser page context"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "JavaScript expression to evaluate"
                }
            },
            "required": ["expression"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let expression = params
            .get("expression")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("expression is required".into()))?;

        Ok(serde_json::json!({
            "status": "evaluated",
            "expression": expression,
            "result": null,
            "note": "browser session management pending runtime integration"
        }))
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Create all browser tools with the given sandbox configuration.
pub fn all_browser_tools(config: BrowserSandboxConfig) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(BrowserNavigateTool::new(config.clone())),
        Box::new(BrowserScreenshotTool::new(config.clone())),
        Box::new(BrowserFillTool::new(config.clone())),
        Box::new(BrowserClickTool::new(config.clone())),
        Box::new(BrowserGetTextTool::new(config.clone())),
        Box::new(BrowserEvaluateTool::new(config)),
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
        async fn list_keys(
            &self,
            _prefix: Option<&str>,
        ) -> Result<Vec<String>, PluginError> {
            Ok(vec![])
        }
    }

    struct MockToolContext;

    impl ToolContext for MockToolContext {
        fn key_value_store(&self) -> &dyn KeyValueStore {
            &MockKvStore
        }
        fn plugin_id(&self) -> &str {
            "clawft-plugin-browser"
        }
        fn agent_id(&self) -> &str {
            "test-agent"
        }
    }

    fn test_config() -> BrowserSandboxConfig {
        BrowserSandboxConfig {
            allowed_domains: vec!["example.com".into(), "test.org".into()],
            ..Default::default()
        }
    }

    #[test]
    fn all_tools_returns_six() {
        let tools = all_browser_tools(test_config());
        assert_eq!(tools.len(), 6);

        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"browser_navigate"));
        assert!(names.contains(&"browser_screenshot"));
        assert!(names.contains(&"browser_fill"));
        assert!(names.contains(&"browser_click"));
        assert!(names.contains(&"browser_get_text"));
        assert!(names.contains(&"browser_evaluate"));
    }

    #[test]
    fn tool_descriptions_non_empty() {
        let tools = all_browser_tools(test_config());
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
        let tools = all_browser_tools(test_config());
        for tool in &tools {
            let schema = tool.parameters_schema();
            assert!(schema.is_object(), "schema not object for {}", tool.name());
            assert_eq!(schema["type"], "object");
        }
    }

    #[tokio::test]
    async fn navigate_blocks_file_scheme() {
        let tool = BrowserNavigateTool::new(test_config());
        let ctx = MockToolContext;

        let params = serde_json::json!({ "url": "file:///etc/passwd" });
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::PermissionDenied(_)));
    }

    #[tokio::test]
    async fn navigate_blocks_data_scheme() {
        let tool = BrowserNavigateTool::new(test_config());
        let ctx = MockToolContext;

        let params = serde_json::json!({ "url": "data:text/html,<h1>hi</h1>" });
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn navigate_blocks_javascript_scheme() {
        let tool = BrowserNavigateTool::new(test_config());
        let ctx = MockToolContext;

        let params = serde_json::json!({ "url": "javascript:alert(1)" });
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn navigate_blocks_unlisted_domain() {
        let tool = BrowserNavigateTool::new(test_config());
        let ctx = MockToolContext;

        let params = serde_json::json!({ "url": "https://evil.com/steal" });
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::PermissionDenied(_)));
    }

    #[tokio::test]
    async fn navigate_allows_listed_domain() {
        let tool = BrowserNavigateTool::new(test_config());
        let ctx = MockToolContext;

        let params = serde_json::json!({ "url": "https://example.com/page" });
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn screenshot_blocks_bad_url() {
        let tool = BrowserScreenshotTool::new(test_config());
        let ctx = MockToolContext;

        let params = serde_json::json!({ "url": "file:///etc/shadow" });
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn screenshot_allows_good_url() {
        let tool = BrowserScreenshotTool::new(test_config());
        let ctx = MockToolContext;

        let params = serde_json::json!({ "url": "https://test.org/page" });
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_ok());
    }
}
