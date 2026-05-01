//! Render UI tool for pushing canvas elements to the Live Canvas.
//!
//! Agents call this tool to render, update, or remove UI elements
//! on the canvas. The tool parses the input as a [`CanvasCommand`],
//! validates it, and (when wired with a [`CanvasPublisher`]) publishes
//! the command to the message bus / WebSocket broadcaster on the
//! `canvas` topic so the dashboard `/canvas` route can render it
//! in real time.

use std::sync::Arc;

use async_trait::async_trait;
use clawft_core::tools::registry::{Tool, ToolError};
use clawft_types::canvas::CanvasCommand;
use serde_json::json;
use tracing::{debug, info};

/// Publisher abstraction the render_ui tool uses to broadcast validated
/// [`CanvasCommand`]s to subscribed dashboard clients.
///
/// The gateway wires a concrete implementation backed by the
/// `TopicBroadcaster` so payloads land on the `canvas` topic and reach
/// any browser tab subscribed via WebSocket. Tests pass a mock
/// publisher to assert the tool dispatches correctly without standing
/// up the full broadcaster.
#[cfg_attr(not(feature = "browser"), async_trait)]
#[cfg_attr(feature = "browser", async_trait(?Send))]
pub trait CanvasPublisher: Send + Sync {
    /// Publish a JSON payload to the named topic.
    ///
    /// Implementations should be cheap to clone via `Arc` and must not
    /// block the calling task — the WebSocket broadcaster's `publish`
    /// is `async` and non-blocking.
    async fn publish(&self, topic: &str, message: serde_json::Value);
}

/// Topic name used to deliver canvas commands to dashboard clients.
pub const CANVAS_TOPIC: &str = "canvas";

/// Tool that agents invoke to push UI elements to the canvas.
///
/// Accepts a JSON payload conforming to the [`CanvasCommand`] protocol,
/// validates it, and publishes it to the configured [`CanvasPublisher`]
/// on the [`CANVAS_TOPIC`]. When no publisher is wired the tool still
/// validates input and returns success so existing tests and the
/// browser/WASM build path keep working.
pub struct RenderUiTool {
    publisher: Option<Arc<dyn CanvasPublisher>>,
}

impl RenderUiTool {
    /// Create a new `RenderUiTool` with no publisher wired.
    ///
    /// The tool will still validate input and return success but will
    /// not broadcast commands anywhere. Used by the WASM/browser build
    /// where there is no separate dashboard subscriber to fan out to.
    pub fn new() -> Self {
        Self { publisher: None }
    }

    /// Create a new `RenderUiTool` wired to a [`CanvasPublisher`].
    ///
    /// Validated commands will be published as JSON on
    /// [`CANVAS_TOPIC`] for fan-out to subscribed dashboard clients.
    pub fn with_publisher(publisher: Arc<dyn CanvasPublisher>) -> Self {
        Self {
            publisher: Some(publisher),
        }
    }
}

impl Default for RenderUiTool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(not(feature = "browser"), async_trait)]
#[cfg_attr(feature = "browser", async_trait(?Send))]
impl Tool for RenderUiTool {
    fn name(&self) -> &str {
        "render_ui"
    }

    fn description(&self) -> &str {
        "Render a UI element on the Live Canvas. Supports text, buttons, inputs, images, code blocks, tables, and forms."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The canvas command: render, update, remove, reset, or batch",
                    "enum": ["render", "update", "remove", "reset", "batch"]
                },
                "id": {
                    "type": "string",
                    "description": "Element ID (required for render, update, remove)"
                },
                "element": {
                    "type": "object",
                    "description": "The canvas element to render or update",
                    "properties": {
                        "type": {
                            "type": "string",
                            "description": "Element type: text, button, input, image, code, table, form",
                            "enum": ["text", "button", "input", "image", "code", "table", "form"]
                        }
                    }
                },
                "position": {
                    "type": "integer",
                    "description": "Optional position index for render command"
                },
                "commands": {
                    "type": "array",
                    "description": "Array of sub-commands for batch command",
                    "items": { "type": "object" }
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        // Parse the input as a CanvasCommand.
        let command: CanvasCommand = serde_json::from_value(args.clone()).map_err(|e| {
            ToolError::InvalidArgs(format!("invalid canvas command: {e}"))
        })?;

        // Extract the element ID for the response (if applicable).
        // CanvasCommand is non_exhaustive, so we keep a catch-all arm
        // for forward compatibility — newly-added variants will simply
        // not surface an element id in the success payload.
        let element_id = match &command {
            CanvasCommand::Render { id, .. } => Some(id.clone()),
            CanvasCommand::Update { id, .. } => Some(id.clone()),
            CanvasCommand::Remove { id } => Some(id.clone()),
            CanvasCommand::Reset => None,
            CanvasCommand::Batch { commands } => {
                info!(count = commands.len(), "processing batch canvas command");
                None
            }
            _ => None,
        };

        debug!(?command, "render_ui tool invoked");

        // Publish validated command to the WebSocket broadcaster (if
        // wired). The payload mirrors the `CanvasCommand` protocol as
        // tagged JSON plus a `type: canvas_command` discriminator so
        // the dashboard's `/canvas` route can match on the data type.
        if let Some(publisher) = &self.publisher {
            let payload = match serde_json::to_value(&command) {
                Ok(mut v) => {
                    // Inject a type discriminator alongside the
                    // CanvasCommand tagged enum so frontend listeners
                    // (which dispatch on `data.type`) can pick it up.
                    if let Some(obj) = v.as_object_mut() {
                        obj.insert(
                            "type".into(),
                            serde_json::Value::String("canvas_command".into()),
                        );
                    }
                    v
                }
                Err(e) => {
                    return Err(ToolError::ExecutionFailed(format!(
                        "failed to serialize canvas command: {e}"
                    )));
                }
            };
            publisher.publish(CANVAS_TOPIC, payload).await;
            debug!(topic = CANVAS_TOPIC, "canvas command broadcast");
        }

        Ok(json!({
            "status": "rendered",
            "element_id": element_id,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn make_tool() -> RenderUiTool {
        RenderUiTool::new()
    }

    /// Recording publisher used to assert the tool dispatched a command
    /// to the expected topic with the expected payload shape.
    struct RecordingPublisher {
        events: Mutex<Vec<(String, serde_json::Value)>>,
    }

    impl RecordingPublisher {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(Vec::new()),
            })
        }

        fn snapshot(&self) -> Vec<(String, serde_json::Value)> {
            self.events.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CanvasPublisher for RecordingPublisher {
        async fn publish(&self, topic: &str, message: serde_json::Value) {
            self.events
                .lock()
                .unwrap()
                .push((topic.to_string(), message));
        }
    }

    #[test]
    fn name_is_render_ui() {
        assert_eq!(make_tool().name(), "render_ui");
    }

    #[test]
    fn description_not_empty() {
        assert!(!make_tool().description().is_empty());
    }

    #[test]
    fn parameters_has_command_field() {
        let params = make_tool().parameters();
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&json!("command")));
    }

    #[test]
    fn default_impl() {
        let tool = RenderUiTool::default();
        assert_eq!(tool.name(), "render_ui");
    }

    #[tokio::test]
    async fn render_text_element() {
        let result = make_tool()
            .execute(json!({
                "command": "render",
                "id": "el-1",
                "element": { "type": "text", "content": "Hello" }
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "rendered");
        assert_eq!(result["element_id"], "el-1");
    }

    #[tokio::test]
    async fn render_button_element() {
        let result = make_tool()
            .execute(json!({
                "command": "render",
                "id": "btn-1",
                "element": {
                    "type": "button",
                    "label": "Click me",
                    "action": "do_thing"
                }
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "rendered");
        assert_eq!(result["element_id"], "btn-1");
    }

    #[tokio::test]
    async fn update_element() {
        let result = make_tool()
            .execute(json!({
                "command": "update",
                "id": "el-1",
                "element": { "type": "text", "content": "Updated" }
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "rendered");
        assert_eq!(result["element_id"], "el-1");
    }

    #[tokio::test]
    async fn remove_element() {
        let result = make_tool()
            .execute(json!({
                "command": "remove",
                "id": "el-1"
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "rendered");
        assert_eq!(result["element_id"], "el-1");
    }

    #[tokio::test]
    async fn reset_canvas() {
        let result = make_tool()
            .execute(json!({ "command": "reset" }))
            .await
            .unwrap();

        assert_eq!(result["status"], "rendered");
        assert!(result["element_id"].is_null());
    }

    #[tokio::test]
    async fn batch_command() {
        let result = make_tool()
            .execute(json!({
                "command": "batch",
                "commands": [
                    { "command": "reset" },
                    {
                        "command": "render",
                        "id": "el-1",
                        "element": { "type": "text", "content": "Fresh" }
                    }
                ]
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "rendered");
    }

    #[tokio::test]
    async fn invalid_command_returns_error() {
        let err = make_tool()
            .execute(json!({ "command": "invalid_cmd" }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn missing_command_returns_error() {
        let err = make_tool()
            .execute(json!({ "id": "el-1" }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn render_with_position() {
        let result = make_tool()
            .execute(json!({
                "command": "render",
                "id": "el-top",
                "element": { "type": "text", "content": "First" },
                "position": 0
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "rendered");
        assert_eq!(result["element_id"], "el-top");
    }

    #[tokio::test]
    async fn render_code_element() {
        let result = make_tool()
            .execute(json!({
                "command": "render",
                "id": "code-1",
                "element": {
                    "type": "code",
                    "code": "fn main() {}",
                    "language": "rust"
                }
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "rendered");
        assert_eq!(result["element_id"], "code-1");
    }

    #[tokio::test]
    async fn render_table_element() {
        let result = make_tool()
            .execute(json!({
                "command": "render",
                "id": "tbl-1",
                "element": {
                    "type": "table",
                    "headers": ["Name", "Age"],
                    "rows": [["Alice", "30"]]
                }
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "rendered");
        assert_eq!(result["element_id"], "tbl-1");
    }

    #[tokio::test]
    async fn render_form_element() {
        let result = make_tool()
            .execute(json!({
                "command": "render",
                "id": "form-1",
                "element": {
                    "type": "form",
                    "fields": [{
                        "name": "username",
                        "label": "Username",
                        "field_type": "text",
                        "required": true
                    }],
                    "submit_action": "create_user"
                }
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "rendered");
        assert_eq!(result["element_id"], "form-1");
    }

    #[test]
    fn tool_is_object_safe() {
        fn accepts_tool(_t: &dyn Tool) {}
        accepts_tool(&make_tool());
    }

    /// WEFT-306: when a publisher is wired, the tool publishes the
    /// validated command on the `canvas` topic with a
    /// `type: canvas_command` discriminator so the dashboard's
    /// `/canvas` route can render it in real time.
    #[tokio::test]
    async fn render_publishes_to_canvas_topic_when_wired() {
        let publisher = RecordingPublisher::new();
        let tool = RenderUiTool::with_publisher(publisher.clone());

        let result = tool
            .execute(json!({
                "command": "render",
                "id": "el-1",
                "element": { "type": "text", "content": "Hello" }
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "rendered");

        let events = publisher.snapshot();
        assert_eq!(events.len(), 1, "expected one publish call");
        assert_eq!(events[0].0, CANVAS_TOPIC);
        assert_eq!(events[0].1["type"], "canvas_command");
        assert_eq!(events[0].1["command"], "render");
        assert_eq!(events[0].1["id"], "el-1");
    }

    /// WEFT-306: every command kind reaches the broadcaster, including
    /// `reset` (which has no element id).
    #[tokio::test]
    async fn reset_publishes_to_canvas_topic() {
        let publisher = RecordingPublisher::new();
        let tool = RenderUiTool::with_publisher(publisher.clone());

        tool.execute(json!({"command": "reset"})).await.unwrap();

        let events = publisher.snapshot();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, CANVAS_TOPIC);
        assert_eq!(events[0].1["type"], "canvas_command");
        assert_eq!(events[0].1["command"], "reset");
    }

    /// WEFT-306: invalid commands are rejected before any publish
    /// attempt — broadcasting only happens on validated input.
    #[tokio::test]
    async fn invalid_command_does_not_publish() {
        let publisher = RecordingPublisher::new();
        let tool = RenderUiTool::with_publisher(publisher.clone());

        let _ = tool
            .execute(json!({"command": "invalid_cmd"}))
            .await
            .unwrap_err();

        assert!(
            publisher.snapshot().is_empty(),
            "no publish should occur for invalid input"
        );
    }
}
