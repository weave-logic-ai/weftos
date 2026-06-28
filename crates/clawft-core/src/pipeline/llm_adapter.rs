//! Bridge between the pipeline's [`LlmProvider`] trait and `clawft-llm`'s
//! [`Provider`](clawft_llm::Provider) trait.
//!
//! The pipeline system uses [`LlmProvider`] (defined in [`super::transport`]),
//! which operates on raw `serde_json::Value` messages and responses. The
//! `clawft-llm` crate uses typed [`ChatRequest`](clawft_llm::ChatRequest) and
//! [`ChatResponse`](clawft_llm::ChatResponse).
//!
//! This module provides:
//!
//! - [`ClawftLlmAdapter`] -- wraps an `Arc<dyn clawft_llm::Provider>` and
//!   implements [`LlmProvider`], converting between the two type systems.
//!
//! - [`create_adapter_from_config`] -- factory that resolves the right provider
//!   from a [`Config`] and returns it as `Arc<dyn LlmProvider>`.
//!
//! - [`build_live_pipeline`] -- constructs a full [`PipelineRegistry`] wired
//!   with a real LLM transport.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::debug;

use clawft_llm::{
    ChatMessage, ChatRequest as LlmChatRequest, ChatResponse, LlmProviderConfig,
    OpenAiCompatProvider, ProviderRouter,
};
use clawft_types::config::Config;

use super::assembler::TokenBudgetAssembler;
use super::classifier::KeywordClassifier;
use super::cost_tracker::CostTracker;
use super::rate_limiter::RateLimiter;
use super::router::StaticRouter;
use super::tiered_router::TieredRouter;
use super::traits::{ModelRouter, Pipeline, PipelineRegistry};
use super::transport::{LlmProvider, OpenAiCompatTransport};

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

/// Adapts a [`clawft_llm::Provider`] into the pipeline's [`LlmProvider`] trait.
///
/// The adapter handles two conversions on every call:
///
/// 1. **Inbound**: `&[serde_json::Value]` messages are converted to
///    `Vec<ChatMessage>`, and the remaining scalar parameters are packed into a
///    [`LlmChatRequest`].
///
/// 2. **Outbound**: The [`ChatResponse`] is serialized back into OpenAI-format
///    `serde_json::Value` so the existing [`OpenAiCompatTransport`] response
///    parser can consume it.
pub struct ClawftLlmAdapter {
    /// The wrapped clawft-llm provider.
    provider: Arc<dyn clawft_llm::Provider>,
}

impl ClawftLlmAdapter {
    /// Wrap a provider in the adapter.
    pub fn new(provider: Arc<dyn clawft_llm::Provider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl LlmProvider for ClawftLlmAdapter {
    async fn complete(
        &self,
        model: &str,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
        max_tokens: Option<i32>,
        temperature: Option<f64>,
    ) -> Result<serde_json::Value, String> {
        // -- Inbound conversion: Value messages -> ChatMessage ---------------
        let chat_messages: Vec<ChatMessage> =
            messages.iter().map(convert_value_to_message).collect();

        let request = LlmChatRequest {
            model: model.to_string(),
            messages: chat_messages,
            max_tokens,
            temperature,
            tools: tools.to_vec(),
            stream: None,
        };

        debug!(
            provider = %self.provider.name(),
            model = %model,
            messages = request.messages.len(),
            tools = request.tools.len(),
            "adapter forwarding request to clawft-llm provider"
        );

        // Retry is handled by RetryPolicy<P> wrapping at provider construction
        // time (see create_adapter_from_config). No duplicate retry loop here.
        match self.provider.complete(&request).await {
            Ok(response) => Ok(convert_response_to_value(&response)),
            Err(e) => Err(e.to_string()),
        }
    }

    async fn complete_stream(
        &self,
        model: &str,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
        max_tokens: Option<i32>,
        temperature: Option<f64>,
        tx: mpsc::Sender<String>,
    ) -> Result<serde_json::Value, String> {
        let chat_messages: Vec<ChatMessage> =
            messages.iter().map(convert_value_to_message).collect();

        let request = LlmChatRequest {
            model: model.to_string(),
            messages: chat_messages,
            max_tokens,
            temperature,
            tools: tools.to_vec(),
            stream: Some(true),
        };

        debug!(
            provider = %self.provider.name(),
            model = %model,
            messages = request.messages.len(),
            tools = request.tools.len(),
            "adapter forwarding streaming request to clawft-llm provider"
        );

        // Create an internal channel for StreamChunks from clawft-llm
        let (chunk_tx, mut chunk_rx) = mpsc::channel::<clawft_llm::StreamChunk>(64);

        // Spawn the underlying provider's streaming call
        let provider = Arc::clone(&self.provider);
        let stream_handle =
            tokio::spawn(async move { provider.complete_stream(&request, chunk_tx).await });

        // Forward text deltas to the pipeline's string-based channel
        // and accumulate them for the final response
        let mut full_text = String::new();
        let mut finish_reason = None;
        let mut usage = None;

        while let Some(chunk) = chunk_rx.recv().await {
            match chunk {
                clawft_llm::StreamChunk::TextDelta { text } => {
                    full_text.push_str(&text);
                    if tx.send(text).await.is_err() {
                        // Receiver dropped, stop streaming
                        break;
                    }
                }
                clawft_llm::StreamChunk::Done {
                    finish_reason: fr,
                    usage: u,
                } => {
                    if fr.is_some() {
                        finish_reason = fr;
                    }
                    if u.is_some() {
                        usage = u;
                    }
                }
                clawft_llm::StreamChunk::ToolCallDelta { .. } => {
                    // Tool call deltas are not forwarded as text, but we
                    // could extend this in the future
                }
            }
        }

        // Wait for the stream task to complete
        let _ = stream_handle.await;

        // Build a synthetic ChatResponse from the accumulated text
        let fr = finish_reason.unwrap_or_else(|| "stop".into());
        let llm_usage = usage.map(|u| clawft_llm::Usage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            total_tokens: u.total_tokens,
        });

        let response = ChatResponse {
            id: "stream-response".into(),
            model: model.to_string(),
            choices: vec![clawft_llm::types::Choice {
                index: 0,
                message: ChatMessage::assistant(full_text),
                finish_reason: Some(fr),
            }],
            usage: llm_usage,
        };

        Ok(convert_response_to_value(&response))
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a `serde_json::Value` message into a [`ChatMessage`].
///
/// Extracts `role`, `content`, `tool_call_id`, and `tool_calls` fields.
/// When an assistant message has tool_calls and empty/null content, content
/// is set to `None` so that the API receives `"content": null` instead of
/// `"content": ""` (which Anthropic's endpoint rejects).
fn convert_value_to_message(value: &serde_json::Value) -> ChatMessage {
    let role = value["role"].as_str().unwrap_or("user").to_string();
    let content_str = value["content"].as_str().map(String::from);
    let tool_call_id = value
        .get("tool_call_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let tool_calls: Option<Vec<clawft_llm::types::ToolCall>> = value
        .get("tool_calls")
        .filter(|v| !v.is_null())
        .and_then(|v| {
            serde_json::from_value(v.clone())
                .map_err(|e| {
                    debug!("failed to deserialize tool_calls: {e}");
                    e
                })
                .ok()
        });

    // Use None for content when it's empty/missing and tool_calls are present,
    // so the serialised request sends "content": null (required by Anthropic).
    let content = match content_str {
        Some(s) if s.is_empty() && tool_calls.is_some() => None,
        other => other,
    };

    ChatMessage {
        role,
        content,
        tool_call_id,
        tool_calls,
    }
}

/// Convert a [`ChatResponse`] into an OpenAI-format `serde_json::Value`.
fn convert_response_to_value(response: &ChatResponse) -> serde_json::Value {
    let choices: Vec<serde_json::Value> = response
        .choices
        .iter()
        .map(|c| {
            let mut msg = serde_json::json!({
                "role": c.message.role,
                "content": c.message.content, // Option<String> -> null or "text"
            });
            if let Some(ref tcs) = c.message.tool_calls {
                msg["tool_calls"] = serde_json::to_value(tcs).unwrap_or_default();
            }
            serde_json::json!({
                "index": c.index,
                "message": msg,
                "finish_reason": c.finish_reason,
            })
        })
        .collect();

    let usage = response.usage.as_ref().map(|u| {
        serde_json::json!({
            "prompt_tokens": u.input_tokens,
            "completion_tokens": u.output_tokens,
            "total_tokens": u.total_tokens,
        })
    });

    serde_json::json!({
        "id": response.id,
        "model": response.model,
        "choices": choices,
        "usage": usage,
    })
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Create an [`LlmProvider`] adapter from application configuration.
///
/// Resolution strategy:
///
/// 1. Parse the model prefix from `config.agents.defaults.model` (e.g.
///    `"anthropic/"` from `"anthropic/claude-opus-4-5"`).
/// 2. Find the matching built-in [`LlmProviderConfig`](LlmProviderConfig)
///    from [`clawft_llm::config::builtin_providers()`].
/// 3. If the application config (`config.providers`) has an API key or
///    base URL override for that provider, apply it.
/// 4. Create an [`OpenAiCompatProvider`] and wrap it in a [`ClawftLlmAdapter`].
///
/// Falls back to the first built-in provider (OpenAI) when no prefix matches.
pub fn create_adapter_from_config(config: &Config) -> Arc<dyn LlmProvider> {
    let model = &config.agents.defaults.model;
    let (prefix, _bare_model) = ProviderRouter::strip_prefix(model);

    let builtins = clawft_llm::config::builtin_providers();

    // Find the matching built-in config by provider name prefix.
    let mut provider_config = match &prefix {
        Some(name) => builtins
            .iter()
            .find(|c| c.name == *name)
            .cloned()
            .unwrap_or_else(|| builtins[0].clone()),
        None => builtins[0].clone(),
    };

    // Apply overrides from the application config's providers section.
    apply_config_overrides(&mut provider_config, config, prefix.as_deref());

    debug!(
        provider = %provider_config.name,
        base_url = %provider_config.base_url,
        model = %model,
        "creating LLM adapter from config"
    );

    let provider = OpenAiCompatProvider::new(provider_config);
    // Wrap in RetryPolicy so transient errors (5xx, rate-limit, timeout)
    // are retried with exponential backoff at the provider level.
    let retrying =
        clawft_llm::retry::RetryPolicy::new(provider, clawft_llm::retry::RetryConfig::default());
    Arc::new(ClawftLlmAdapter::new(Arc::new(retrying)))
}

/// Apply API key and base URL overrides from the application config to a
/// built-in provider config.
///
/// The application config stores provider credentials in
/// `config.providers.<name>`, where each entry has `api_key` and optionally
/// `api_base`. If present, these override the built-in defaults.
fn apply_config_overrides(
    llm_config: &mut LlmProviderConfig,
    app_config: &Config,
    provider_name: Option<&str>,
) {
    let name = provider_name.unwrap_or(&llm_config.name);

    // Look up the matching provider in the app config.
    let app_provider = match name {
        "openai" => &app_config.providers.openai,
        "anthropic" => &app_config.providers.anthropic,
        "groq" => &app_config.providers.groq,
        "deepseek" => &app_config.providers.deepseek,
        "openrouter" => &app_config.providers.openrouter,
        "gemini" => &app_config.providers.gemini,
        "xai" => &app_config.providers.xai,
        "mistral" | "together" => return, // supported builtins with no config override
        _ => return,
    };

    // Override base URL if provided.
    if let Some(ref base) = app_provider.api_base
        && !base.is_empty()
    {
        llm_config.base_url = base.clone();
    }

    // Merge extra headers if provided.
    if let Some(ref headers) = app_provider.extra_headers {
        for (k, v) in headers {
            llm_config.headers.insert(k.clone(), v.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// Multi-provider adapter factory
// ---------------------------------------------------------------------------

/// Create an [`LlmProvider`] adapter for a specific provider name.
///
/// Looks up the provider in `clawft_llm::config::builtin_providers()`,
/// applies overrides from the application config (base URL, headers, API key),
/// and returns the wrapped adapter.
fn create_adapter_for_provider(provider_name: &str, config: &Config) -> Arc<dyn LlmProvider> {
    let builtins = clawft_llm::config::builtin_providers();

    let mut provider_config = builtins
        .iter()
        .find(|c| c.name == provider_name)
        .cloned()
        .unwrap_or_else(|| builtins[0].clone());

    apply_config_overrides(&mut provider_config, config, Some(provider_name));

    debug!(
        provider = %provider_config.name,
        base_url = %provider_config.base_url,
        "creating LLM adapter for provider"
    );

    // Check for an explicit API key in the app config.
    let app_api_key = resolve_app_api_key(provider_name, config);

    let provider = if let Some(key) = app_api_key {
        OpenAiCompatProvider::with_api_key(provider_config, key)
    } else {
        OpenAiCompatProvider::new(provider_config)
    };

    Arc::new(ClawftLlmAdapter::new(Arc::new(provider)))
}

/// Resolve an explicit API key from the application config for a provider.
///
/// Returns `Some(key)` if the config has a non-empty `api_key` for the
/// provider, `None` otherwise (the provider will fall back to its env var).
fn resolve_app_api_key(provider_name: &str, config: &Config) -> Option<String> {
    let key = match provider_name {
        "openai" => &config.providers.openai.api_key,
        "anthropic" => &config.providers.anthropic.api_key,
        "groq" => &config.providers.groq.api_key,
        "deepseek" => &config.providers.deepseek.api_key,
        "openrouter" => &config.providers.openrouter.api_key,
        "gemini" => &config.providers.gemini.api_key,
        "xai" => &config.providers.xai.api_key,
        _ => return None,
    };
    if key.is_empty() {
        None
    } else {
        Some(key.expose().to_string())
    }
}

/// Create adapters for all providers referenced in the routing tiers,
/// plus the fallback model and the default model.
///
/// Returns a map from provider prefix (e.g. `"gemini"`) to an adapter.
fn create_adapters_for_tiers(config: &Config) -> HashMap<String, Arc<dyn LlmProvider>> {
    let mut adapters: HashMap<String, Arc<dyn LlmProvider>> = HashMap::new();

    // Collect all unique provider prefixes from tier models.
    for tier in &config.routing.tiers {
        for model_str in &tier.models {
            let (prefix, _) = ProviderRouter::strip_prefix(model_str);
            if let Some(name) = prefix
                && !adapters.contains_key(&name)
            {
                adapters.insert(name.clone(), create_adapter_for_provider(&name, config));
            }
        }
    }

    // Ensure the fallback model's provider is included.
    if let Some(ref fallback_model) = config.routing.fallback_model {
        let (fallback_prefix, _) = ProviderRouter::strip_prefix(fallback_model);
        if let Some(name) = fallback_prefix {
            adapters
                .entry(name.clone())
                .or_insert_with(|| create_adapter_for_provider(&name, config));
        }
    }

    // Ensure the default model's provider is included.
    let (default_prefix, _) = ProviderRouter::strip_prefix(&config.agents.defaults.model);
    if let Some(name) = default_prefix {
        adapters
            .entry(name.clone())
            .or_insert_with(|| create_adapter_for_provider(&name, config));
    }

    debug!(
        providers = ?adapters.keys().collect::<Vec<_>>(),
        "created adapters for tiered routing"
    );

    adapters
}

// ---------------------------------------------------------------------------
// Pipeline construction
// ---------------------------------------------------------------------------

/// Build a pipeline registry backed by a real LLM provider.
///
/// This is the production counterpart of
/// [`build_default_pipeline`](crate::bootstrap::build_default_pipeline).
/// Instead of a stub transport, it uses [`OpenAiCompatTransport::with_provider`]
/// with an adapter created from the application config.
pub fn build_live_pipeline(config: &Config) -> PipelineRegistry {
    let classifier = Arc::new(KeywordClassifier::new());
    let router: Arc<dyn ModelRouter> = build_router(config);

    // Use the context window from the routing tiers (input budget), not
    // max_tokens (output budget). Fall back to 128 000 if no tier specifies one.
    let context_budget = config
        .routing
        .tiers
        .iter()
        .map(|t| t.max_context_tokens)
        .max()
        .unwrap_or(128_000);
    let assembler = Arc::new(TokenBudgetAssembler::new(context_budget));

    // Build transport: multi-provider for tiered routing, single for static.
    let transport: Arc<OpenAiCompatTransport> = if config.routing.mode == "tiered" {
        let adapters = create_adapters_for_tiers(config);
        let fallback = create_adapter_from_config(config);
        Arc::new(OpenAiCompatTransport::with_providers(adapters, fallback))
    } else {
        let adapter = create_adapter_from_config(config);
        Arc::new(OpenAiCompatTransport::with_provider(adapter))
    };

    let scorer = super::build_scorer(&config.pipeline);
    let learner = super::build_learner(&config.pipeline);

    let pipeline = Pipeline {
        classifier,
        router,
        assembler,
        transport,
        scorer,
        learner,
    };

    PipelineRegistry::new(pipeline)
}

/// Build the appropriate router based on `config.routing.mode`.
///
/// - `"tiered"` -> [`TieredRouter`] with optional cost tracking and rate limiting
/// - anything else -> [`StaticRouter`] (Level 0 default)
fn build_router(config: &Config) -> Arc<dyn ModelRouter> {
    if config.routing.mode == "tiered" {
        let routing = config.routing.clone();

        let cost_tracker = Arc::new(CostTracker::new(routing.cost_budgets.reset_hour_utc));
        let rate_limiter = Arc::new(RateLimiter::new(
            routing.rate_limiting.window_seconds,
            routing.rate_limiting.global_rate_limit_rpm,
        ));

        let router = TieredRouter::new(routing)
            .with_cost_tracker(cost_tracker)
            .with_rate_limiter(rate_limiter);

        tracing::info!("pipeline: using TieredRouter (Level 1)");

        Arc::new(router)
    } else {
        tracing::info!("pipeline: using StaticRouter (Level 0)");
        Arc::new(StaticRouter::from_config(&config.agents))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_llm::types::{Choice, FunctionCall, ToolCall, Usage as LlmUsage};
    use clawft_types::config::{AgentDefaults, AgentsConfig};

    fn test_config() -> Config {
        Config {
            agents: AgentsConfig {
                defaults: AgentDefaults {
                    workspace: "~/.clawft/workspace".into(),
                    model: "anthropic/claude-opus-4-5".into(),
                    max_tokens: 4096,
                    temperature: 0.7,
                    max_tool_iterations: 10,
                    memory_window: 50,
                },
                ..AgentsConfig::default()
            },
            ..Config::default()
        }
    }

    // -- convert_value_to_message -------------------------------------------

    #[test]
    fn adapter_converts_messages() {
        let value = serde_json::json!({
            "role": "user",
            "content": "Hello, world!",
        });
        let msg = convert_value_to_message(&value);
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content.as_deref(), Some("Hello, world!"));
        assert!(msg.tool_call_id.is_none());
        assert!(msg.tool_calls.is_none());
    }

    #[test]
    fn adapter_converts_message_with_tool_call_id() {
        let value = serde_json::json!({
            "role": "tool",
            "content": "result data",
            "tool_call_id": "call-123",
        });
        let msg = convert_value_to_message(&value);
        assert_eq!(msg.role, "tool");
        assert_eq!(msg.content.as_deref(), Some("result data"));
        assert_eq!(msg.tool_call_id.as_deref(), Some("call-123"));
    }

    #[test]
    fn adapter_converts_message_defaults() {
        // Missing role and content should fall back to defaults.
        let value = serde_json::json!({});
        let msg = convert_value_to_message(&value);
        assert_eq!(msg.role, "user");
        assert!(msg.content.is_none());
    }

    // -- convert_response_to_value ------------------------------------------

    fn make_text_response() -> ChatResponse {
        ChatResponse {
            id: "resp-1".into(),
            model: "claude-opus-4-5".into(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage::assistant("Hello from the LLM!"),
                finish_reason: Some("stop".into()),
            }],
            usage: Some(LlmUsage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
            }),
        }
    }

    #[test]
    fn adapter_converts_response() {
        let response = make_text_response();
        let value = convert_response_to_value(&response);

        assert_eq!(value["id"], "resp-1");
        assert_eq!(value["model"], "claude-opus-4-5");

        let choice = &value["choices"][0];
        assert_eq!(choice["index"], 0);
        assert_eq!(choice["message"]["role"], "assistant");
        assert_eq!(choice["message"]["content"], "Hello from the LLM!");
        assert_eq!(choice["finish_reason"], "stop");

        let usage = &value["usage"];
        assert_eq!(usage["prompt_tokens"], 10);
        assert_eq!(usage["completion_tokens"], 5);
        assert_eq!(usage["total_tokens"], 15);
    }

    #[test]
    fn adapter_converts_response_no_usage() {
        let response = ChatResponse {
            id: "resp-no-usage".into(),
            model: "test-model".into(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage::assistant("ok"),
                finish_reason: Some("stop".into()),
            }],
            usage: None,
        };
        let value = convert_response_to_value(&response);
        assert!(value["usage"].is_null());
    }

    // -- tool call round-trip -----------------------------------------------

    #[test]
    fn adapter_handles_tool_calls() {
        // Build a response with tool calls.
        let response = ChatResponse {
            id: "resp-tc".into(),
            model: "test-model".into(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_call_id: None,
                    tool_calls: Some(vec![ToolCall {
                        id: "call_abc".into(),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: "get_weather".into(),
                            arguments: r#"{"city":"London"}"#.into(),
                        },
                    }]),
                },
                finish_reason: Some("tool_calls".into()),
            }],
            usage: Some(LlmUsage {
                input_tokens: 15,
                output_tokens: 8,
                total_tokens: 23,
            }),
        };

        let value = convert_response_to_value(&response);

        // Verify tool calls are present in the JSON output.
        let tool_calls = value["choices"][0]["message"]["tool_calls"]
            .as_array()
            .expect("tool_calls should be an array");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "call_abc");
        assert_eq!(tool_calls[0]["function"]["name"], "get_weather");
        assert_eq!(
            tool_calls[0]["function"]["arguments"],
            r#"{"city":"London"}"#
        );

        // Now convert the tool_calls back to a message Value and re-parse.
        let msg_value = &value["choices"][0]["message"];
        let round_tripped = convert_value_to_message(msg_value);
        assert_eq!(round_tripped.role, "assistant");
        let tcs = round_tripped.tool_calls.expect("should have tool_calls");
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].id, "call_abc");
        assert_eq!(tcs[0].function.name, "get_weather");
    }

    // -- error propagation --------------------------------------------------

    struct FailingProvider;

    #[async_trait]
    impl clawft_llm::Provider for FailingProvider {
        fn name(&self) -> &str {
            "failing"
        }
        async fn complete(&self, _request: &LlmChatRequest) -> clawft_llm::Result<ChatResponse> {
            Err(clawft_llm::ProviderError::RequestFailed(
                "simulated network failure".into(),
            ))
        }
    }

    #[tokio::test]
    async fn adapter_maps_errors() {
        let adapter = ClawftLlmAdapter::new(Arc::new(FailingProvider));
        let result = adapter.complete("test-model", &[], &[], None, None).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("simulated network failure"),
            "error should propagate: {err}"
        );
    }

    // -- successful round-trip through adapter ------------------------------

    struct EchoProvider;

    #[async_trait]
    impl clawft_llm::Provider for EchoProvider {
        fn name(&self) -> &str {
            "echo"
        }
        async fn complete(&self, request: &LlmChatRequest) -> clawft_llm::Result<ChatResponse> {
            let content = request
                .messages
                .last()
                .map(|m| format!("echo: {}", m.content.as_deref().unwrap_or("")))
                .unwrap_or_else(|| "echo: (empty)".into());
            Ok(ChatResponse {
                id: "echo-resp".into(),
                model: request.model.clone(),
                choices: vec![Choice {
                    index: 0,
                    message: ChatMessage::assistant(content),
                    finish_reason: Some("stop".into()),
                }],
                usage: Some(LlmUsage {
                    input_tokens: 5,
                    output_tokens: 3,
                    total_tokens: 8,
                }),
            })
        }
    }

    #[tokio::test]
    async fn adapter_complete_round_trip() {
        let adapter = ClawftLlmAdapter::new(Arc::new(EchoProvider));

        let messages = vec![serde_json::json!({
            "role": "user",
            "content": "ping",
        })];

        let result = adapter
            .complete("test-model", &messages, &[], Some(100), Some(0.5))
            .await
            .unwrap();

        assert_eq!(result["id"], "echo-resp");
        assert_eq!(result["model"], "test-model");
        assert_eq!(result["choices"][0]["message"]["content"], "echo: ping");
        assert_eq!(result["usage"]["total_tokens"], 8);
    }

    // -- factory tests ------------------------------------------------------

    #[test]
    fn create_adapter_returns_provider() {
        let config = test_config();
        let adapter = create_adapter_from_config(&config);
        // The adapter should be created without panicking.
        // We cannot call complete() without a real API key, but we can
        // verify the Arc is valid by checking it exists.
        let _ = Arc::strong_count(&adapter);
    }

    #[test]
    fn create_adapter_with_default_config() {
        let config = Config::default();
        let adapter = create_adapter_from_config(&config);
        let _ = Arc::strong_count(&adapter);
    }

    #[test]
    fn create_adapter_unknown_prefix() {
        let mut config = test_config();
        config.agents.defaults.model = "unknown-provider/some-model".into();
        // Should fall back to the first built-in (openai) without panicking.
        let adapter = create_adapter_from_config(&config);
        let _ = Arc::strong_count(&adapter);
    }

    #[test]
    fn create_adapter_no_prefix() {
        let mut config = test_config();
        config.agents.defaults.model = "gpt-4o".into();
        // No prefix -- should use the default (openai) provider.
        let adapter = create_adapter_from_config(&config);
        let _ = Arc::strong_count(&adapter);
    }

    // -- build_live_pipeline ------------------------------------------------

    #[test]
    fn build_live_pipeline_is_configured() {
        let config = test_config();
        let _registry = build_live_pipeline(&config);
        // Should not panic; the pipeline is fully wired.
    }

    #[test]
    fn build_live_pipeline_with_defaults() {
        let config = Config::default();
        let _registry = build_live_pipeline(&config);
    }

    // -- apply_config_overrides ---------------------------------------------

    #[test]
    fn overrides_apply_base_url() {
        let mut config = test_config();
        config.providers.anthropic.api_base = Some("https://custom.proxy.com/v1".into());

        let builtins = clawft_llm::config::builtin_providers();
        let mut llm_config = builtins
            .iter()
            .find(|c| c.name == "anthropic")
            .cloned()
            .unwrap();

        apply_config_overrides(&mut llm_config, &config, Some("anthropic"));
        assert_eq!(llm_config.base_url, "https://custom.proxy.com/v1");
    }

    #[test]
    fn overrides_skip_empty_base_url() {
        let config = test_config();

        let builtins = clawft_llm::config::builtin_providers();
        let mut llm_config = builtins
            .iter()
            .find(|c| c.name == "openai")
            .cloned()
            .unwrap();

        let original_url = llm_config.base_url.clone();
        apply_config_overrides(&mut llm_config, &config, Some("openai"));
        // No override was set, so the URL should remain unchanged.
        assert_eq!(llm_config.base_url, original_url);
    }

    #[test]
    fn overrides_merge_headers() {
        let mut config = test_config();
        let mut headers = std::collections::HashMap::new();
        headers.insert("X-Custom".into(), "value".into());
        config.providers.anthropic.extra_headers = Some(headers);

        let builtins = clawft_llm::config::builtin_providers();
        let mut llm_config = builtins
            .iter()
            .find(|c| c.name == "anthropic")
            .cloned()
            .unwrap();

        apply_config_overrides(&mut llm_config, &config, Some("anthropic"));
        // Should have both the original anthropic-version header and the custom one.
        assert_eq!(llm_config.headers.get("X-Custom").unwrap(), "value");
        assert!(llm_config.headers.contains_key("anthropic-version"));
    }

    // -- end-to-end: MockProvider -> ClawftLlmAdapter -> OpenAiCompatTransport -

    /// End-to-end round-trip test that exercises the full adapter-to-transport path.
    ///
    /// 1. Creates a mock `clawft_llm::Provider` (EchoProvider).
    /// 2. Wraps it in `ClawftLlmAdapter`.
    /// 3. Creates `OpenAiCompatTransport::with_provider(adapter)`.
    /// 4. Constructs a `TransportRequest`.
    /// 5. Calls `transport.complete()` and verifies the `LlmResponse`.
    #[tokio::test]
    async fn transport_adapter_round_trip() {
        use crate::pipeline::traits::{LlmMessage, LlmTransport, TransportRequest};
        use clawft_types::provider::{ContentBlock, StopReason};

        // 1. Create mock provider
        let echo_provider: Arc<dyn clawft_llm::Provider> = Arc::new(EchoProvider);

        // 2. Wrap in adapter
        let adapter: Arc<dyn LlmProvider> = Arc::new(ClawftLlmAdapter::new(echo_provider));

        // 3. Create transport with the adapter
        let transport = OpenAiCompatTransport::with_provider(adapter);
        assert!(transport.is_configured());

        // 4. Build a TransportRequest
        let request = TransportRequest {
            provider: "echo".into(),
            model: "echo-model".into(),
            messages: vec![
                LlmMessage {
                    role: "system".into(),
                    content: "You are a test assistant.".into(),
                    tool_call_id: None,
                    tool_calls: None,
                },
                LlmMessage {
                    role: "user".into(),
                    content: "integration test".into(),
                    tool_call_id: None,
                    tool_calls: None,
                },
            ],
            tools: vec![],
            max_tokens: Some(100),
            temperature: Some(0.5),
        };

        // 5. Call complete and verify
        let response = transport
            .complete(&request)
            .await
            .expect("transport round-trip should succeed with mock provider");

        assert_eq!(response.id, "echo-resp");
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert_eq!(response.usage.input_tokens, 5);
        assert_eq!(response.usage.output_tokens, 3);

        // Verify the content block contains the echoed message.
        assert!(!response.content.is_empty());
        match &response.content[0] {
            ContentBlock::Text { text } => {
                assert_eq!(text, "echo: integration test");
            }
            other => panic!("expected Text block, got: {other:?}"),
        }
    }

    /// End-to-end test with a failing provider through the transport layer.
    #[tokio::test]
    async fn transport_adapter_error_propagation() {
        use crate::pipeline::traits::{LlmMessage, LlmTransport, TransportRequest};

        let failing_provider: Arc<dyn clawft_llm::Provider> = Arc::new(FailingProvider);
        let adapter: Arc<dyn LlmProvider> = Arc::new(ClawftLlmAdapter::new(failing_provider));
        let transport = OpenAiCompatTransport::with_provider(adapter);

        let request = TransportRequest {
            provider: "failing".into(),
            model: "test".into(),
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "this should fail".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![],
            max_tokens: None,
            temperature: None,
        };

        let result = transport.complete(&request).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("simulated network failure"),
            "error should propagate through adapter+transport: {err_msg}"
        );
    }

    /// End-to-end test: adapter wraps a provider with tool call responses,
    /// verifying the full chain: Provider -> Adapter -> Transport -> LlmResponse.
    #[tokio::test]
    async fn transport_adapter_tool_call_round_trip() {
        use crate::pipeline::traits::{LlmMessage, LlmTransport, TransportRequest};
        use clawft_types::provider::{ContentBlock, StopReason};

        // Provider that returns tool calls.
        struct ToolCallProvider;

        #[async_trait]
        impl clawft_llm::Provider for ToolCallProvider {
            fn name(&self) -> &str {
                "tool-call-provider"
            }
            async fn complete(
                &self,
                _request: &LlmChatRequest,
            ) -> clawft_llm::Result<ChatResponse> {
                Ok(ChatResponse {
                    id: "tc-resp".into(),
                    model: "test-model".into(),
                    choices: vec![Choice {
                        index: 0,
                        message: ChatMessage {
                            role: "assistant".into(),
                            content: None,
                            tool_call_id: None,
                            tool_calls: Some(vec![ToolCall {
                                id: "call_xyz".into(),
                                call_type: "function".into(),
                                function: FunctionCall {
                                    name: "web_search".into(),
                                    arguments: r#"{"query":"rust lang"}"#.into(),
                                },
                            }]),
                        },
                        finish_reason: Some("tool_calls".into()),
                    }],
                    usage: Some(LlmUsage {
                        input_tokens: 20,
                        output_tokens: 10,
                        total_tokens: 30,
                    }),
                })
            }
        }

        let provider: Arc<dyn clawft_llm::Provider> = Arc::new(ToolCallProvider);
        let adapter: Arc<dyn LlmProvider> = Arc::new(ClawftLlmAdapter::new(provider));
        let transport = OpenAiCompatTransport::with_provider(adapter);

        let request = TransportRequest {
            provider: "tool-call-provider".into(),
            model: "test-model".into(),
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "search for rust".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![serde_json::json!({"type": "function", "name": "web_search"})],
            max_tokens: Some(100),
            temperature: None,
        };

        let response = transport
            .complete(&request)
            .await
            .expect("tool call round-trip should succeed");

        assert_eq!(response.id, "tc-resp");
        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(response.usage.input_tokens, 20);
        assert_eq!(response.usage.output_tokens, 10);

        // Should have a ToolUse content block (empty text is skipped).
        assert_eq!(response.content.len(), 1);
        match &response.content[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call_xyz");
                assert_eq!(name, "web_search");
                assert_eq!(input["query"], "rust lang");
            }
            other => panic!("expected ToolUse block, got: {other:?}"),
        }
    }
}
