//! OpenAI-compatible transport implementation.
//!
//! Provides two transport variants:
//!
//! - [`OpenAiCompatTransport::new()`] -- Stub that returns an error, used
//!   during early development or when no LLM provider is configured.
//!
//! - [`OpenAiCompatTransport::with_provider()`] -- Wraps any implementation
//!   of [`LlmProvider`] to make real HTTP calls to an LLM endpoint.
//!
//! Once the `clawft-llm` crate exposes its `Provider` trait and
//! `OpenAiCompatProvider`, callers create an adapter implementing
//! [`LlmProvider`] and pass it via `with_provider()`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::debug;

use clawft_types::error::ClawftError;
use clawft_types::provider::{ContentBlock, LlmResponse, StopReason, Usage};

use super::traits::LlmTransport;
#[cfg(feature = "native")]
use super::traits::StreamCallback;
use super::traits::TransportRequest;

/// An abstraction over the underlying LLM HTTP client.
///
/// This trait bridges the gap between the pipeline's `TransportRequest`
/// format and whatever HTTP client library the LLM provider uses. It
/// intentionally uses simple types (strings, JSON values) so that it
/// can be implemented without importing `clawft-llm` types directly.
///
/// Once `clawft-llm` fully exports its `Provider` trait, a blanket
/// adapter will be provided.
///
/// The `async_trait` `?Send` relaxation is applied for the `browser`
/// feature so single-threaded WASM impls (whose underlying `reqwest`
/// types are `!Send`) satisfy the trait. Native impls keep the strict
/// `Send` bound for tokio multi-threaded runtimes.
#[cfg_attr(not(feature = "browser"), async_trait)]
#[cfg_attr(feature = "browser", async_trait(?Send))]
pub trait LlmProvider: Send + Sync {
    /// Execute a chat completion call and return the raw JSON response.
    ///
    /// The response must be a JSON object with at minimum:
    /// - `id` (string): response identifier
    /// - `choices` (array): list of completion choices
    /// - `choices[].message.role` (string): "assistant"
    /// - `choices[].message.content` (string): response text
    /// - `choices[].finish_reason` (string|null): "stop", "tool_calls", "length"
    ///
    /// Tool calls should appear in `choices[].message.tool_calls[]` with:
    /// - `id` (string): tool call identifier
    /// - `function.name` (string): tool name
    /// - `function.arguments` (string): JSON-encoded arguments
    async fn complete(
        &self,
        model: &str,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
        max_tokens: Option<i32>,
        temperature: Option<f64>,
    ) -> Result<serde_json::Value, String>;

    /// Execute a streaming chat completion, sending text deltas to the channel.
    ///
    /// Each string sent is a text delta from the SSE stream. The function
    /// should return the final aggregated JSON response (same format as
    /// `complete()`) after the stream ends.
    ///
    /// The default implementation returns an error indicating streaming is
    /// not supported. Providers that support streaming should override this.
    ///
    /// Only available with the `native` feature (requires tokio channels).
    #[cfg(feature = "native")]
    async fn complete_stream(
        &self,
        _model: &str,
        _messages: &[serde_json::Value],
        _tools: &[serde_json::Value],
        _max_tokens: Option<i32>,
        _temperature: Option<f64>,
        _tx: tokio::sync::mpsc::Sender<String>,
    ) -> Result<serde_json::Value, String> {
        Err("streaming not supported by this provider".into())
    }
}

/// OpenAI-compatible transport for the pipeline.
///
/// Can operate in two modes:
/// - **Stub mode** (default): Returns an error indicating no provider is configured.
/// - **Provider mode**: Delegates to an injected [`LlmProvider`] for real calls.
pub struct OpenAiCompatTransport {
    provider: Option<Arc<dyn LlmProvider>>,
    /// Named providers for multi-provider routing (keyed by provider prefix).
    providers: HashMap<String, Arc<dyn LlmProvider>>,
}

impl OpenAiCompatTransport {
    /// Create a stub transport that returns an error on every call.
    ///
    /// Use this during development or when no LLM provider is available.
    pub fn new() -> Self {
        Self {
            provider: None,
            providers: HashMap::new(),
        }
    }

    /// Create a transport backed by a real LLM provider.
    ///
    /// The provider will be called for every `complete()` invocation,
    /// and its JSON response will be converted to the pipeline's
    /// [`LlmResponse`] type.
    pub fn with_provider(provider: Arc<dyn LlmProvider>) -> Self {
        Self {
            provider: Some(provider),
            providers: HashMap::new(),
        }
    }

    /// Create a transport backed by multiple named LLM providers.
    ///
    /// The `providers` map is keyed by provider prefix (e.g. `"gemini"`,
    /// `"openrouter"`, `"anthropic"`). When a [`TransportRequest`] arrives,
    /// the transport looks up `request.provider` in this map. If no match
    /// is found, it falls back to `fallback`.
    pub fn with_providers(
        providers: HashMap<String, Arc<dyn LlmProvider>>,
        fallback: Arc<dyn LlmProvider>,
    ) -> Self {
        Self {
            provider: Some(fallback),
            providers,
        }
    }

    /// Returns true if this transport has a configured provider.
    pub fn is_configured(&self) -> bool {
        self.provider.is_some() || !self.providers.is_empty()
    }
}

impl Default for OpenAiCompatTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(not(feature = "browser"), async_trait)]
#[cfg_attr(feature = "browser", async_trait(?Send))]
impl LlmTransport for OpenAiCompatTransport {
    async fn complete(&self, request: &TransportRequest) -> clawft_types::Result<LlmResponse> {
        let provider = self
            .providers
            .get(&request.provider)
            .or(self.provider.as_ref())
            .ok_or_else(|| ClawftError::Provider {
                message: "transport not configured -- call with_provider()".into(),
            })?;

        // Convert pipeline messages to JSON values for the provider.
        // For assistant messages with tool_calls, use null content instead of ""
        // so Anthropic's API correctly associates the tool call IDs.
        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|m| {
                let content_value = if m.content.is_empty() && m.tool_calls.is_some() {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(m.content)
                };
                let mut msg = serde_json::json!({
                    "role": m.role,
                    "content": content_value,
                });
                if let Some(ref id) = m.tool_call_id {
                    msg["tool_call_id"] = serde_json::json!(id);
                }
                if let Some(ref tcs) = m.tool_calls {
                    msg["tool_calls"] = serde_json::json!(tcs);
                }
                msg
            })
            .collect();

        debug!(
            provider = %request.provider,
            model = %request.model,
            messages = messages.len(),
            "sending request via transport"
        );

        // Call the provider
        let raw_response = provider
            .complete(
                &request.model,
                &messages,
                &request.tools,
                request.max_tokens,
                request.temperature,
            )
            .await
            .map_err(|e| ClawftError::Provider { message: e })?;

        // Convert the raw JSON to our LlmResponse
        convert_response(raw_response)
    }

    #[cfg(feature = "native")]
    async fn complete_stream(
        &self,
        request: &TransportRequest,
        mut callback: StreamCallback,
    ) -> clawft_types::Result<LlmResponse> {
        let provider = self
            .providers
            .get(&request.provider)
            .or(self.provider.as_ref())
            .ok_or_else(|| ClawftError::Provider {
                message: "transport not configured -- call with_provider()".into(),
            })?;

        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|m| {
                let content_value = if m.content.is_empty() && m.tool_calls.is_some() {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(m.content)
                };
                let mut msg = serde_json::json!({
                    "role": m.role,
                    "content": content_value,
                });
                if let Some(ref id) = m.tool_call_id {
                    msg["tool_call_id"] = serde_json::json!(id);
                }
                if let Some(ref tcs) = m.tool_calls {
                    msg["tool_calls"] = serde_json::json!(tcs);
                }
                msg
            })
            .collect();

        debug!(
            provider = %request.provider,
            model = %request.model,
            messages = messages.len(),
            "sending streaming request via transport"
        );

        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);

        let model = request.model.clone();
        let tools = request.tools.clone();
        let max_tokens = request.max_tokens;
        let temperature = request.temperature;
        let provider_clone = Arc::clone(provider);

        let stream_handle = tokio::spawn(async move {
            provider_clone
                .complete_stream(&model, &messages, &tools, max_tokens, temperature, tx)
                .await
        });

        let mut full_text = String::new();
        while let Some(text_delta) = rx.recv().await {
            full_text.push_str(&text_delta);
            if !callback(&text_delta) {
                break;
            }
        }

        let stream_result = stream_handle.await.map_err(|e| ClawftError::Provider {
            message: format!("stream task panicked: {e}"),
        })?;

        match stream_result {
            Ok(raw_response) => convert_response(raw_response),
            Err(e) => {
                if !full_text.is_empty() {
                    debug!(
                        "stream ended with error but collected {} chars, returning partial response",
                        full_text.len()
                    );
                    Ok(LlmResponse {
                        id: "stream-partial".into(),
                        content: vec![ContentBlock::Text { text: full_text }],
                        stop_reason: StopReason::EndTurn,
                        usage: Usage {
                            input_tokens: 0,
                            output_tokens: 0,
                            total_tokens: 0,
                        },
                        metadata: HashMap::new(),
                    })
                } else {
                    Err(ClawftError::Provider { message: e })
                }
            }
        }
    }
}

/// Convert a raw OpenAI-format JSON response to an [`LlmResponse`].
///
/// Handles both text responses and tool-call responses, extracting
/// content blocks, stop reason, and usage statistics.
fn convert_response(resp: serde_json::Value) -> clawft_types::Result<LlmResponse> {
    let id = resp
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let model = resp
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    // Extract the first choice
    let choice = resp
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .ok_or_else(|| ClawftError::Provider {
            message: "response has no choices".into(),
        })?;

    let message = choice.get("message").ok_or_else(|| ClawftError::Provider {
        message: "choice has no message".into(),
    })?;

    let finish_reason = choice
        .get("finish_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("stop");

    // Build content blocks
    let mut content = Vec::new();

    // Extract text content
    if let Some(text) = message.get("content").and_then(|v| v.as_str())
        && !text.is_empty()
    {
        content.push(ContentBlock::Text {
            text: text.to_string(),
        });
    }

    // Extract tool calls
    if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tool_calls {
            let tc_id = tc
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let function = tc.get("function").cloned().unwrap_or_default();
            let name = function
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let arguments = function
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");

            let input: serde_json::Value =
                crate::json_repair::parse_with_repair(arguments).unwrap_or(serde_json::json!({}));

            content.push(ContentBlock::ToolUse {
                id: tc_id,
                name,
                input,
            });
        }
    }

    // Determine stop reason
    let stop_reason = match finish_reason {
        "stop" => StopReason::EndTurn,
        "tool_calls" => StopReason::ToolUse,
        "length" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        _ => StopReason::EndTurn,
    };

    // Extract usage
    let usage_obj = resp.get("usage");
    let usage = Usage {
        input_tokens: usage_obj
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        output_tokens: usage_obj
            .and_then(|u| u.get("completion_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        total_tokens: usage_obj
            .and_then(|u| u.get("total_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
    };

    let mut metadata = HashMap::new();
    metadata.insert("model".into(), serde_json::json!(model));

    Ok(LlmResponse {
        id,
        content,
        stop_reason,
        usage,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::traits::LlmMessage;
    use tokio::sync::mpsc;

    fn make_transport_request() -> TransportRequest {
        TransportRequest {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "hello".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![],
            max_tokens: Some(1024),
            temperature: Some(0.7),
        }
    }

    // -- Stub mode tests --

    #[tokio::test]
    async fn stub_returns_error() {
        let transport = OpenAiCompatTransport::new();
        let result = transport.complete(&make_transport_request()).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("transport not configured"),
            "error should mention not configured: {msg}"
        );
    }

    #[tokio::test]
    async fn stub_returns_provider_error_variant() {
        let transport = OpenAiCompatTransport::new();
        let result = transport.complete(&make_transport_request()).await;
        match result {
            Err(ClawftError::Provider { message }) => {
                assert!(message.contains("with_provider"));
            }
            other => panic!("expected Provider error, got: {other:?}"),
        }
    }

    #[test]
    fn default_trait_impl() {
        let transport = OpenAiCompatTransport::default();
        assert!(!transport.is_configured());
    }

    #[test]
    fn new_is_not_configured() {
        let transport = OpenAiCompatTransport::new();
        assert!(!transport.is_configured());
    }

    // -- Provider mode tests --

    struct MockProvider {
        response: serde_json::Value,
    }

    impl MockProvider {
        fn text_response(text: &str) -> Self {
            Self {
                response: serde_json::json!({
                    "id": "resp-1",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": text
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 10,
                        "completion_tokens": 5,
                        "total_tokens": 15
                    },
                    "model": "test-model"
                }),
            }
        }

        fn tool_call_response() -> Self {
            Self {
                response: serde_json::json!({
                    "id": "resp-2",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "tool_calls": [{
                                "id": "call_abc",
                                "type": "function",
                                "function": {
                                    "name": "get_weather",
                                    "arguments": "{\"city\":\"London\"}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }],
                    "usage": {
                        "prompt_tokens": 15,
                        "completion_tokens": 8,
                        "total_tokens": 23
                    },
                    "model": "test-model"
                }),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        async fn complete(
            &self,
            _model: &str,
            _messages: &[serde_json::Value],
            _tools: &[serde_json::Value],
            _max_tokens: Option<i32>,
            _temperature: Option<f64>,
        ) -> Result<serde_json::Value, String> {
            Ok(self.response.clone())
        }
    }

    struct FailingProvider;

    #[async_trait]
    impl LlmProvider for FailingProvider {
        async fn complete(
            &self,
            _model: &str,
            _messages: &[serde_json::Value],
            _tools: &[serde_json::Value],
            _max_tokens: Option<i32>,
            _temperature: Option<f64>,
        ) -> Result<serde_json::Value, String> {
            Err("mock network failure".into())
        }
    }

    #[test]
    fn with_provider_is_configured() {
        let provider = Arc::new(MockProvider::text_response("hi"));
        let transport = OpenAiCompatTransport::with_provider(provider);
        assert!(transport.is_configured());
    }

    #[tokio::test]
    async fn provider_text_response() {
        let provider = Arc::new(MockProvider::text_response("Hello from LLM!"));
        let transport = OpenAiCompatTransport::with_provider(provider);
        let result = transport.complete(&make_transport_request()).await.unwrap();

        assert_eq!(result.id, "resp-1");
        assert_eq!(result.stop_reason, StopReason::EndTurn);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Hello from LLM!"),
            _ => panic!("expected Text block"),
        }
        assert_eq!(result.usage.input_tokens, 10);
        assert_eq!(result.usage.output_tokens, 5);
    }

    #[tokio::test]
    async fn provider_tool_call_response() {
        let provider = Arc::new(MockProvider::tool_call_response());
        let transport = OpenAiCompatTransport::with_provider(provider);
        let result = transport.complete(&make_transport_request()).await.unwrap();

        assert_eq!(result.id, "resp-2");
        assert_eq!(result.stop_reason, StopReason::ToolUse);
        // Should have tool call block (empty text is skipped)
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call_abc");
                assert_eq!(name, "get_weather");
                assert_eq!(input["city"], "London");
            }
            _ => panic!("expected ToolUse block"),
        }
    }

    #[tokio::test]
    async fn provider_error_propagates() {
        let provider = Arc::new(FailingProvider);
        let transport = OpenAiCompatTransport::with_provider(provider);
        let result = transport.complete(&make_transport_request()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("mock network failure"));
    }

    // -- convert_response tests --

    #[test]
    fn convert_text_response() {
        let resp = serde_json::json!({
            "id": "test-id",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "hello world"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 5,
                "completion_tokens": 3,
                "total_tokens": 8
            },
            "model": "gpt-4o"
        });
        let result = convert_response(resp).unwrap();
        assert_eq!(result.id, "test-id");
        assert_eq!(result.stop_reason, StopReason::EndTurn);
        match &result.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hello world"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn convert_tool_call_response() {
        let resp = serde_json::json!({
            "id": "tc-resp",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {
                            "name": "search",
                            "arguments": "{\"q\": \"rust\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "model": "gpt-4o"
        });
        let result = convert_response(resp).unwrap();
        assert_eq!(result.stop_reason, StopReason::ToolUse);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call-1");
                assert_eq!(name, "search");
                assert_eq!(input["q"], "rust");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn convert_response_no_choices() {
        let resp = serde_json::json!({
            "id": "empty",
            "choices": [],
            "model": "test"
        });
        let result = convert_response(resp);
        assert!(result.is_err());
    }

    #[test]
    fn convert_response_max_tokens_stop() {
        let resp = serde_json::json!({
            "id": "len",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "truncated..."
                },
                "finish_reason": "length"
            }],
            "model": "test"
        });
        let result = convert_response(resp).unwrap();
        assert_eq!(result.stop_reason, StopReason::MaxTokens);
    }

    #[test]
    fn convert_response_missing_usage() {
        let resp = serde_json::json!({
            "id": "no-usage",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "ok"
                },
                "finish_reason": "stop"
            }],
            "model": "test"
        });
        let result = convert_response(resp).unwrap();
        assert_eq!(result.usage.input_tokens, 0);
        assert_eq!(result.usage.output_tokens, 0);
    }

    #[test]
    fn convert_response_mixed_text_and_tools() {
        let resp = serde_json::json!({
            "id": "mixed",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Let me check that.",
                    "tool_calls": [{
                        "id": "call-2",
                        "type": "function",
                        "function": {
                            "name": "lookup",
                            "arguments": "{}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "model": "test"
        });
        let result = convert_response(resp).unwrap();
        // Should have both text and tool use blocks
        assert_eq!(result.content.len(), 2);
        assert!(matches!(&result.content[0], ContentBlock::Text { .. }));
        assert!(matches!(&result.content[1], ContentBlock::ToolUse { .. }));
    }

    #[tokio::test]
    async fn stub_ignores_request_fields() {
        let transport = OpenAiCompatTransport::new();

        let req1 = TransportRequest {
            provider: "anthropic".into(),
            model: "claude-opus-4-5".into(),
            messages: vec![],
            tools: vec![serde_json::json!({"type": "function"})],
            max_tokens: None,
            temperature: None,
        };
        let req2 = make_transport_request();

        let err1 = transport.complete(&req1).await.unwrap_err().to_string();
        let err2 = transport.complete(&req2).await.unwrap_err().to_string();
        assert_eq!(err1, err2);
    }

    #[test]
    fn convert_response_invalid_arguments_json() {
        let resp = serde_json::json!({
            "id": "bad-args",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call-bad",
                        "type": "function",
                        "function": {
                            "name": "test",
                            "arguments": "not valid json"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "model": "test"
        });
        // Should not fail -- invalid arguments default to empty object
        let result = convert_response(resp).unwrap();
        match &result.content[0] {
            ContentBlock::ToolUse { input, .. } => {
                assert_eq!(*input, serde_json::json!({}));
            }
            _ => panic!("expected ToolUse"),
        }
    }

    // ── GAP-17: Tool call parsing edge case tests ─────────────────────

    #[test]
    fn convert_response_missing_tool_use_id() {
        // Some providers may omit the tool call id entirely.
        let resp = serde_json::json!({
            "id": "missing-id",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"/tmp/test.txt\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "model": "test"
        });
        let result = convert_response(resp).unwrap();
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "unknown", "missing id should default to 'unknown'");
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "/tmp/test.txt");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn convert_response_empty_arguments_string() {
        // Provider returns empty string instead of valid JSON for arguments.
        let resp = serde_json::json!({
            "id": "empty-args",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call-empty",
                        "type": "function",
                        "function": {
                            "name": "list_dir",
                            "arguments": ""
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "model": "test"
        });
        let result = convert_response(resp).unwrap();
        match &result.content[0] {
            ContentBlock::ToolUse { name, input, .. } => {
                assert_eq!(name, "list_dir");
                // Empty string is not valid JSON, should default to {}
                assert_eq!(*input, serde_json::json!({}));
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn convert_response_nested_json_arguments() {
        // Arguments with deeply nested JSON objects.
        let resp = serde_json::json!({
            "id": "nested-args",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call-nested",
                        "type": "function",
                        "function": {
                            "name": "complex_tool",
                            "arguments": "{\"config\":{\"nested\":{\"deep\":true}},\"items\":[1,2,3]}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "model": "test"
        });
        let result = convert_response(resp).unwrap();
        match &result.content[0] {
            ContentBlock::ToolUse { name, input, .. } => {
                assert_eq!(name, "complex_tool");
                assert_eq!(input["config"]["nested"]["deep"], true);
                assert_eq!(input["items"], serde_json::json!([1, 2, 3]));
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn convert_response_missing_function_field() {
        // Tool call with missing function field entirely.
        let resp = serde_json::json!({
            "id": "no-func",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call-nofunc",
                        "type": "function"
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "model": "test"
        });
        let result = convert_response(resp).unwrap();
        match &result.content[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call-nofunc");
                assert_eq!(
                    name, "unknown",
                    "missing function.name should default to 'unknown'"
                );
                assert_eq!(*input, serde_json::json!({}));
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn convert_response_multiple_tool_calls() {
        // Response with multiple tool calls in a single message.
        let resp = serde_json::json!({
            "id": "multi-tc",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [
                        {
                            "id": "call-1",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"a.txt\"}"
                            }
                        },
                        {
                            "id": "call-2",
                            "type": "function",
                            "function": {
                                "name": "web_search",
                                "arguments": "{\"query\":\"rust\"}"
                            }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }],
            "model": "test"
        });
        let result = convert_response(resp).unwrap();
        assert_eq!(result.content.len(), 2);
        match &result.content[0] {
            ContentBlock::ToolUse { name, .. } => assert_eq!(name, "read_file"),
            _ => panic!("expected ToolUse"),
        }
        match &result.content[1] {
            ContentBlock::ToolUse { name, .. } => assert_eq!(name, "web_search"),
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn convert_response_null_content_with_tool_calls() {
        // Content is null (not empty string) and there are tool calls.
        let resp = serde_json::json!({
            "id": "null-content",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call-null",
                        "type": "function",
                        "function": {
                            "name": "exec",
                            "arguments": "{\"command\":\"ls\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "model": "test"
        });
        let result = convert_response(resp).unwrap();
        // Null content should not produce a Text block
        assert_eq!(result.content.len(), 1);
        assert!(matches!(&result.content[0], ContentBlock::ToolUse { .. }));
    }

    #[test]
    fn convert_response_missing_message_field() {
        // Choice missing the message field entirely.
        let resp = serde_json::json!({
            "id": "no-msg",
            "choices": [{
                "index": 0,
                "finish_reason": "stop"
            }],
            "model": "test"
        });
        let result = convert_response(resp);
        assert!(result.is_err(), "should fail when message field is missing");
    }

    #[test]
    fn convert_response_unknown_finish_reason() {
        // Unknown finish_reason should default to EndTurn.
        let resp = serde_json::json!({
            "id": "unknown-fr",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "ok"
                },
                "finish_reason": "content_filter"
            }],
            "model": "test"
        });
        let result = convert_response(resp).unwrap();
        assert_eq!(result.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn convert_response_null_finish_reason() {
        // finish_reason is null (streaming incomplete).
        let resp = serde_json::json!({
            "id": "null-fr",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "partial"
                },
                "finish_reason": null
            }],
            "model": "test"
        });
        let result = convert_response(resp).unwrap();
        // null finish_reason should default to "stop" -> EndTurn
        assert_eq!(result.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn convert_response_arguments_as_json_value_not_string() {
        // Some providers send arguments as a JSON object, not a string.
        // Our parser expects a string, so a JSON object should fall through
        // to the empty object default gracefully.
        let resp = serde_json::json!({
            "id": "obj-args",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call-obj",
                        "type": "function",
                        "function": {
                            "name": "tool",
                            "arguments": {"key": "value"}
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "model": "test"
        });
        let result = convert_response(resp).unwrap();
        match &result.content[0] {
            ContentBlock::ToolUse { input, .. } => {
                // arguments.as_str() returns None for a JSON object,
                // so it falls through to the empty default
                assert_eq!(*input, serde_json::json!({}));
            }
            _ => panic!("expected ToolUse"),
        }
    }

    // ── Streaming tests ─────────────────────────────────────────────

    struct StreamingMockProvider {
        text_chunks: Vec<String>,
        final_response: serde_json::Value,
    }

    impl StreamingMockProvider {
        fn text_stream(chunks: &[&str]) -> Self {
            let full_text: String = chunks.iter().copied().collect();
            Self {
                text_chunks: chunks.iter().map(|c| c.to_string()).collect(),
                final_response: serde_json::json!({
                    "id": "stream-resp",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": full_text
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 10,
                        "completion_tokens": 5,
                        "total_tokens": 15
                    },
                    "model": "test-model"
                }),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for StreamingMockProvider {
        async fn complete(
            &self,
            _model: &str,
            _messages: &[serde_json::Value],
            _tools: &[serde_json::Value],
            _max_tokens: Option<i32>,
            _temperature: Option<f64>,
        ) -> Result<serde_json::Value, String> {
            Ok(self.final_response.clone())
        }

        async fn complete_stream(
            &self,
            _model: &str,
            _messages: &[serde_json::Value],
            _tools: &[serde_json::Value],
            _max_tokens: Option<i32>,
            _temperature: Option<f64>,
            tx: mpsc::Sender<String>,
        ) -> Result<serde_json::Value, String> {
            for chunk in &self.text_chunks {
                if tx.send(chunk.clone()).await.is_err() {
                    break;
                }
            }
            Ok(self.final_response.clone())
        }
    }

    #[tokio::test]
    async fn streaming_transport_collects_text() {
        let provider = Arc::new(StreamingMockProvider::text_stream(&[
            "Hello", " ", "world", "!",
        ]));
        let transport = OpenAiCompatTransport::with_provider(provider);

        let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let collected_clone = collected.clone();

        let callback: StreamCallback = Box::new(move |text| {
            collected_clone.lock().unwrap().push(text.to_string());
            true
        });

        let response = transport
            .complete_stream(&make_transport_request(), callback)
            .await
            .unwrap();

        let chunks = collected.lock().unwrap().clone();
        assert_eq!(chunks, vec!["Hello", " ", "world", "!"]);
        assert_eq!(response.id, "stream-resp");
    }

    #[tokio::test]
    async fn streaming_transport_callback_abort() {
        let provider = Arc::new(StreamingMockProvider::text_stream(&[
            "Hello", " ", "world", "!",
        ]));
        let transport = OpenAiCompatTransport::with_provider(provider);

        let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let collected_clone = collected.clone();

        // Callback that aborts after receiving 2 chunks
        let callback: StreamCallback = Box::new(move |text| {
            let mut vec = collected_clone.lock().unwrap();
            vec.push(text.to_string());
            vec.len() < 2
        });

        let _response = transport
            .complete_stream(&make_transport_request(), callback)
            .await
            .unwrap();

        let chunks = collected.lock().unwrap().clone();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "Hello");
        assert_eq!(chunks[1], " ");
    }

    #[tokio::test]
    async fn streaming_fallback_for_non_streaming_provider() {
        // MockProvider does not implement complete_stream (uses default),
        // but the LlmTransport::complete_stream default impl falls back
        // to complete() and sends the full text via callback.
        let provider = Arc::new(MockProvider::text_response("Full response text"));
        let transport = OpenAiCompatTransport::with_provider(provider);

        let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let collected_clone = collected.clone();

        let callback: StreamCallback = Box::new(move |text| {
            collected_clone.lock().unwrap().push(text.to_string());
            true
        });

        // The default complete_stream on LlmTransport calls complete()
        // then fires the callback once with the full text. Since MockProvider
        // does not implement LlmProvider::complete_stream, the
        // OpenAiCompatTransport::complete_stream will use the spawned task
        // which will fail, but we have collected text as fallback.
        // Actually, the MockProvider's default complete_stream returns an error,
        // so the transport should use the fallback path.
        let result = transport
            .complete_stream(&make_transport_request(), callback)
            .await;

        // The LlmProvider default complete_stream returns an error, so the
        // spawned stream task fails. Since no text was collected during
        // streaming, the transport propagates an error. Callers should use
        // complete() directly for providers that do not support streaming.
        assert!(
            result.is_err(),
            "expected error from non-streaming provider, got success"
        );
    }

    #[tokio::test]
    async fn streaming_stub_returns_error() {
        let transport = OpenAiCompatTransport::new();

        let callback: StreamCallback = Box::new(|_| true);

        let result = transport
            .complete_stream(&make_transport_request(), callback)
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("transport not configured"));
    }
}
