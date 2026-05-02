//! Claude-based task delegator.
//!
//! [`ClaudeDelegator`] sends multi-turn requests to the Anthropic Messages
//! API, executes tool calls via a caller-provided executor, and returns the
//! final text response.
//!
//! # Protocol
//!
//! The delegator uses the Anthropic Messages API (not the OpenAI-compatible
//! endpoint) because tool use requires the native format. Tool schemas
//! arrive in OpenAI function-calling format and are converted on the fly
//! via [`super::schema::openai_to_anthropic`].
//!
//! # Feature gate
//!
//! This module lives inside the `delegate`-gated `delegation` module.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde_json::Value;
use tracing::{debug, warn};

use clawft_types::delegation::{DelegationConfig, DelegationTarget};

use super::schema;

/// Errors specific to the Claude delegation subsystem.
#[derive(Debug, thiserror::Error)]
pub enum DelegationError {
    /// The HTTP request to the Claude API failed.
    #[error("http error: {0}")]
    Http(String),

    /// The API returned a non-2xx status.
    #[error("api error ({status}): {body}")]
    Api { status: u16, body: String },

    /// The response body could not be parsed.
    #[error("invalid response: {0}")]
    InvalidResponse(String),

    /// The delegation loop exceeded the configured max_turns.
    #[error("max turns ({0}) exceeded")]
    MaxTurnsExceeded(u32),

    /// A tool execution failed during delegation.
    #[error("tool execution failed: {0}")]
    ToolExecFailed(String),

    /// The subprocess exited with a non-zero status.
    #[error("subprocess failed (exit code {exit_code}): {stderr}")]
    SubprocessFailed { exit_code: i32, stderr: String },

    /// stdout produced output that could not be parsed as expected.
    #[error("output parse failed: {parse_error}")]
    OutputParseFailed {
        raw_output: String,
        parse_error: String,
    },

    /// The delegation exceeded the configured timeout.
    #[error("delegation timed out after {elapsed:?}")]
    Timeout { elapsed: Duration },

    /// The delegation was cancelled (user abort, agent shutdown).
    #[error("delegation cancelled")]
    Cancelled,

    /// All fallback targets exhausted (Flow -> Claude -> Local).
    #[error("all delegation targets exhausted")]
    FallbackExhausted {
        attempts: Vec<(DelegationTarget, String)>,
    },
}

/// Result alias for delegation operations.
pub type Result<T> = std::result::Result<T, DelegationError>;

/// Default base URL for the Anthropic API.
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// A delegator that sends tasks to the Anthropic Messages API with tool use.
///
/// The delegator maintains a `reqwest::Client` and configuration state.
/// Each call to [`delegate`](ClaudeDelegator::delegate) executes a
/// multi-turn loop: send user message, receive assistant response, execute
/// any tool calls, feed results back, repeat until `stop_reason == "end_turn"`
/// or the turn limit is hit.
pub struct ClaudeDelegator {
    client: reqwest::Client,
    api_key: String,
    model: String,
    max_turns: u32,
    max_tokens: u32,
    excluded_tools: Vec<String>,
    base_url: String,
}

impl ClaudeDelegator {
    /// Create a new delegator from config and API key.
    ///
    /// Returns `None` if the API key is empty, allowing callers to
    /// gracefully degrade when credentials are absent.
    pub fn new(config: &DelegationConfig, api_key: String) -> Option<Self> {
        if api_key.is_empty() {
            return None;
        }
        Some(Self {
            client: reqwest::Client::new(),
            api_key,
            model: config.claude_model.clone(),
            max_turns: config.max_turns,
            max_tokens: config.max_tokens,
            excluded_tools: config.excluded_tools.clone(),
            base_url: DEFAULT_BASE_URL.to_string(),
        })
    }

    /// Override the base URL (for testing with mock servers).
    #[cfg(test)]
    fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    /// Delegate a task to Claude with a tool-use loop.
    ///
    /// # Arguments
    ///
    /// * `task` - The user-facing task description.
    /// * `tool_schemas` - Tool definitions in **OpenAI** function-calling
    ///   format. They will be converted to Anthropic format internally.
    /// * `tool_executor` - A closure that executes a tool by name and
    ///   returns the result as a string. Called once per tool_use block
    ///   in the assistant response.
    ///
    /// # Returns
    ///
    /// The final text response from the assistant, or a [`DelegationError`].
    pub async fn delegate<F>(
        &self,
        task: &str,
        tool_schemas: &[Value],
        tool_executor: &F,
    ) -> Result<String>
    where
        F: Fn(
                &str,
                Value,
            )
                -> Pin<Box<dyn Future<Output = std::result::Result<String, String>> + Send>>
            + Sync,
    {
        // Convert and filter tool schemas.
        let all_anthropic = schema::openai_to_anthropic(tool_schemas);
        let anthropic_tools: Vec<Value> = all_anthropic
            .into_iter()
            .filter(|tool| {
                let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
                !self.excluded_tools.contains(&name.to_string())
            })
            .collect();

        let mut messages: Vec<Value> = vec![serde_json::json!({
            "role": "user",
            "content": task,
        })];

        let url = format!("{}/v1/messages", self.base_url);

        for turn in 0..self.max_turns {
            debug!(turn, model = %self.model, "delegation turn");

            let body = serde_json::json!({
                "model": self.model,
                "max_tokens": self.max_tokens,
                "messages": messages,
                "tools": anthropic_tools,
            });

            let response = self
                .client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| DelegationError::Http(e.to_string()))?;

            let status = response.status().as_u16();
            if !(200..300).contains(&status) {
                let body_text = response.text().await.unwrap_or_default();
                return Err(DelegationError::Api {
                    status,
                    body: body_text,
                });
            }

            let resp_json: Value = response
                .json()
                .await
                .map_err(|e| DelegationError::InvalidResponse(e.to_string()))?;

            let stop_reason = resp_json
                .get("stop_reason")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let content = resp_json
                .get("content")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            // Append the full assistant response to the conversation.
            messages.push(serde_json::json!({
                "role": "assistant",
                "content": content,
            }));

            // If the model stopped normally (no tool use), extract text and return.
            if stop_reason != "tool_use" {
                let text = extract_text_from_content(&content);
                return Ok(text);
            }

            // Execute each tool_use block and build tool_result messages.
            let mut tool_results: Vec<Value> = Vec::new();
            for block in &content {
                if block.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
                    continue;
                }

                let tool_name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let tool_id = block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let input = block.get("input").cloned().unwrap_or(Value::Null);

                debug!(tool = %tool_name, id = %tool_id, "executing delegated tool call");

                match tool_executor(tool_name, input).await {
                    Ok(result_text) => {
                        tool_results.push(schema::tool_result_block(tool_id, &result_text));
                    }
                    Err(err_text) => {
                        warn!(tool = %tool_name, error = %err_text, "tool execution failed during delegation");
                        tool_results.push(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": tool_id,
                            "is_error": true,
                            "content": err_text,
                        }));
                    }
                }
            }

            // Append tool results as a user message.
            messages.push(serde_json::json!({
                "role": "user",
                "content": tool_results,
            }));

            // Check if this is the last allowed turn.
            if turn + 1 >= self.max_turns {
                return Err(DelegationError::MaxTurnsExceeded(self.max_turns));
            }
        }

        Err(DelegationError::MaxTurnsExceeded(self.max_turns))
    }
}

/// Extract concatenated text blocks from Anthropic content array.
fn extract_text_from_content(content: &[Value]) -> String {
    let mut text = String::new();
    for block in content {
        if block.get("type").and_then(|v| v.as_str()) == Some("text")
            && let Some(t) = block.get("text").and_then(|v| v.as_str())
        {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(t);
        }
    }
    text
}

impl std::fmt::Debug for ClaudeDelegator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeDelegator")
            .field("model", &self.model)
            .field("max_turns", &self.max_turns)
            .field("max_tokens", &self.max_tokens)
            .field("api_key", &"***")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ClaudeDelegator::new ────────────────────────────────────────────

    #[test]
    fn new_returns_none_without_api_key() {
        let config = DelegationConfig::default();
        assert!(ClaudeDelegator::new(&config, String::new()).is_none());
    }

    #[test]
    fn new_returns_some_with_api_key() {
        let config = DelegationConfig::default();
        let delegator = ClaudeDelegator::new(&config, "sk-ant-test".into());
        assert!(delegator.is_some());
    }

    #[test]
    fn new_respects_config_fields() {
        let config = DelegationConfig {
            claude_enabled: true,
            claude_model: "custom-model".into(),
            max_turns: 5,
            max_tokens: 2048,
            excluded_tools: vec!["exec_shell".into()],
            ..Default::default()
        };
        let delegator = ClaudeDelegator::new(&config, "key".into()).unwrap();
        assert_eq!(delegator.model, "custom-model");
        assert_eq!(delegator.max_turns, 5);
        assert_eq!(delegator.max_tokens, 2048);
        assert_eq!(delegator.excluded_tools, vec!["exec_shell"]);
    }

    // ── extract_text_from_content ───────────────────────────────────────

    #[test]
    fn extract_text_single_block() {
        let content = vec![serde_json::json!({"type": "text", "text": "hello"})];
        assert_eq!(extract_text_from_content(&content), "hello");
    }

    #[test]
    fn extract_text_multiple_blocks() {
        let content = vec![
            serde_json::json!({"type": "text", "text": "a"}),
            serde_json::json!({"type": "tool_use", "name": "x", "id": "1", "input": {}}),
            serde_json::json!({"type": "text", "text": "b"}),
        ];
        assert_eq!(extract_text_from_content(&content), "a\nb");
    }

    #[test]
    fn extract_text_no_text_blocks() {
        let content =
            vec![serde_json::json!({"type": "tool_use", "name": "x", "id": "1", "input": {}})];
        assert_eq!(extract_text_from_content(&content), "");
    }

    #[test]
    fn extract_text_empty_content() {
        assert_eq!(extract_text_from_content(&[]), "");
    }

    // ── Debug impl ──────────────────────────────────────────────────────

    #[test]
    fn debug_hides_api_key() {
        let config = DelegationConfig::default();
        let delegator = ClaudeDelegator::new(&config, "sk-secret-key-123".into()).unwrap();
        let debug_str = format!("{:?}", delegator);
        assert!(!debug_str.contains("sk-secret-key-123"));
        assert!(debug_str.contains("***"));
    }

    // ── DelegationError display ─────────────────────────────────────────

    #[test]
    fn error_display() {
        let err = DelegationError::Http("connection refused".into());
        assert_eq!(err.to_string(), "http error: connection refused");

        let err = DelegationError::Api {
            status: 429,
            body: "rate limited".into(),
        };
        assert_eq!(err.to_string(), "api error (429): rate limited");

        let err = DelegationError::InvalidResponse("bad json".into());
        assert_eq!(err.to_string(), "invalid response: bad json");

        let err = DelegationError::MaxTurnsExceeded(10);
        assert_eq!(err.to_string(), "max turns (10) exceeded");

        let err = DelegationError::ToolExecFailed("boom".into());
        assert_eq!(err.to_string(), "tool execution failed: boom");
    }

    // ── Excluded tools filtering ────────────────────────────────────────

    #[test]
    fn excluded_tools_are_filtered() {
        let schemas = vec![
            serde_json::json!({
                "type": "function",
                "function": { "name": "read_file", "description": "Read", "parameters": {} }
            }),
            serde_json::json!({
                "type": "function",
                "function": { "name": "exec_shell", "description": "Shell", "parameters": {} }
            }),
        ];

        let all = schema::openai_to_anthropic(&schemas);
        let excluded = ["exec_shell".to_string()];
        let filtered: Vec<Value> = all
            .into_iter()
            .filter(|tool| {
                let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
                !excluded.contains(&name.to_string())
            })
            .collect();

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0]["name"], "read_file");
    }

    // ── Mock HTTP delegation tests ──────────────────────────────────────

    #[tokio::test]
    async fn delegate_text_only_response() {
        let mock_response = serde_json::json!({
            "id": "msg_test",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Here is the answer."}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create_async()
            .await;

        let config = DelegationConfig::default();
        let delegator = ClaudeDelegator::new(&config, "test-key".into())
            .unwrap()
            .with_base_url(server.url());

        let result = delegator
            .delegate("What is 2+2?", &[], &|_name, _input| {
                Box::pin(async { Ok("4".to_string()) })
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Here is the answer.");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn delegate_with_tool_use() {
        let mut server = mockito::Server::new_async().await;

        // First response: tool_use
        let tool_use_response = serde_json::json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Let me check."},
                {
                    "type": "tool_use",
                    "id": "call_1",
                    "name": "read_file",
                    "input": {"path": "/tmp/test.txt"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 15}
        });

        // Second response: final text
        let final_response = serde_json::json!({
            "id": "msg_2",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "The file contains: hello world"}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 20, "output_tokens": 10}
        });

        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(tool_use_response.to_string())
            .create_async()
            .await;

        let mock2 = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(final_response.to_string())
            .create_async()
            .await;

        let config = DelegationConfig::default();
        let delegator = ClaudeDelegator::new(&config, "test-key".into())
            .unwrap()
            .with_base_url(server.url());

        let tool_schemas = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }
            }
        })];

        let result = delegator
            .delegate("Read /tmp/test.txt", &tool_schemas, &|name, _input| {
                let name = name.to_string();
                Box::pin(async move {
                    if name == "read_file" {
                        Ok("hello world".to_string())
                    } else {
                        Err(format!("unknown tool: {name}"))
                    }
                })
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "The file contains: hello world");
        mock.assert_async().await;
        mock2.assert_async().await;
    }

    #[tokio::test]
    async fn delegate_api_error() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(500)
            .with_body("internal server error")
            .create_async()
            .await;

        let config = DelegationConfig::default();
        let delegator = ClaudeDelegator::new(&config, "test-key".into())
            .unwrap()
            .with_base_url(server.url());

        let result = delegator
            .delegate("test", &[], &|_name, _input| {
                Box::pin(async { Ok("ok".to_string()) })
            })
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            DelegationError::Api { status, body } => {
                assert_eq!(status, 500);
                assert!(body.contains("internal server error"));
            }
            other => panic!("expected Api error, got: {other}"),
        }
        mock.assert_async().await;
    }
}
