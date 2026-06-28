//! LLM provider types and the static provider registry.
//!
//! This module contains:
//! - Response types returned by LLM providers ([`LlmResponse`], [`ContentBlock`], etc.)
//! - The [`ProviderSpec`] metadata struct and static [`PROVIDERS`] registry
//! - Lookup helpers: [`find_by_model`], [`find_gateway`], [`find_by_name`]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── LLM response types ──────────────────────────────────────────────────

/// A complete response from an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    /// Provider-assigned response identifier.
    pub id: String,

    /// Content blocks in the response.
    pub content: Vec<ContentBlock>,

    /// Why the model stopped generating.
    pub stop_reason: StopReason,

    /// Token usage for this request/response pair.
    pub usage: Usage,

    /// Arbitrary provider-specific metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// A single block of content in an LLM response.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// A text block.
    Text {
        /// The text content.
        text: String,
    },
    /// A tool-use request from the model.
    ToolUse {
        /// Tool call identifier (for correlating results).
        id: String,
        /// Name of the tool the model wants to invoke.
        name: String,
        /// JSON arguments to pass to the tool.
        input: serde_json::Value,
    },
}

/// The reason a model stopped generating tokens.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Natural end of response.
    EndTurn,
    /// Hit the `max_tokens` limit.
    MaxTokens,
    /// A stop sequence was encountered.
    StopSequence,
    /// The model wants to use a tool.
    ToolUse,
}

/// Token usage statistics for a single LLM call.
///
/// This is the canonical usage type for the entire workspace. It stores
/// token counts as `u32` (token counts are never negative). The fields
/// use the clawft naming convention (`input_tokens`, `output_tokens`),
/// but serde aliases allow deserialization from the OpenAI naming
/// convention (`prompt_tokens`, `completion_tokens`) as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Usage {
    /// Tokens consumed by the prompt / input.
    ///
    /// Deserializes from either `"input_tokens"` or `"prompt_tokens"`.
    #[serde(alias = "prompt_tokens")]
    pub input_tokens: u32,

    /// Tokens generated in the response.
    ///
    /// Deserializes from either `"output_tokens"` or `"completion_tokens"`.
    #[serde(alias = "completion_tokens")]
    pub output_tokens: u32,

    /// Total tokens used (input + output).
    ///
    /// When deserializing from providers that include `total_tokens`, this
    /// field is populated directly. Otherwise it defaults to 0 and callers
    /// can use [`Usage::total`] to compute it.
    #[serde(default)]
    pub total_tokens: u32,
}

impl Usage {
    /// Returns the total token count.
    ///
    /// If `total_tokens` was populated by the provider, returns that value.
    /// Otherwise computes `input_tokens + output_tokens`.
    pub fn total(&self) -> u32 {
        if self.total_tokens > 0 {
            self.total_tokens
        } else {
            self.input_tokens + self.output_tokens
        }
    }
}

/// A tool-call request extracted from a model response.
///
/// This is a convenience struct for pipeline stages that need to
/// process tool calls without dealing with the full [`ContentBlock`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// Tool call identifier.
    pub id: String,
    /// Name of the tool.
    pub name: String,
    /// JSON arguments.
    pub input: serde_json::Value,
}

// ── Provider registry ────────────────────────────────────────────────────

/// Metadata for a single LLM provider.
///
/// Used for model-name matching, API key detection, and URL prefixing.
/// All string fields are `&'static str` because instances live in the
/// static [`PROVIDERS`] array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSpec {
    /// Config field name (e.g. `"dashscope"`).
    pub name: &'static str,

    /// Model-name keywords for matching (lowercase).
    pub keywords: &'static [&'static str],

    /// Environment variable for the API key (e.g. `"DASHSCOPE_API_KEY"`).
    pub env_key: &'static str,

    /// Human-readable name shown in status output.
    pub display_name: &'static str,

    /// Prefix added to model names for routing (e.g. `"deepseek"` makes
    /// `deepseek-chat` become `deepseek/deepseek-chat`).
    pub litellm_prefix: &'static str,

    /// Do not add prefix when model already starts with one of these.
    pub skip_prefixes: &'static [&'static str],

    /// Routes any model (e.g. OpenRouter, AiHubMix).
    pub is_gateway: bool,

    /// Local deployment (e.g. vLLM).
    pub is_local: bool,

    /// Uses OAuth flow instead of API key.
    pub is_oauth: bool,

    /// Fallback base URL for the provider.
    pub default_api_base: &'static str,

    /// Match `api_key` prefix for auto-detection (e.g. `"sk-or-"`).
    pub detect_by_key_prefix: &'static str,

    /// Match substring in `api_base` URL for auto-detection.
    pub detect_by_base_keyword: &'static str,

    /// Strip `"provider/"` prefix before re-prefixing.
    pub strip_model_prefix: bool,
}

impl ProviderSpec {
    /// Display label: `display_name` if non-empty, otherwise title-cased `name`.
    pub fn label(&self) -> &str {
        if self.display_name.is_empty() {
            self.name
        } else {
            self.display_name
        }
    }
}

/// The provider registry. Order equals match priority (gateways first).
///
/// All 15 providers ported from the Python `nanobot/providers/registry.py`.
pub static PROVIDERS: &[ProviderSpec] = &[
    // === Custom (user-provided OpenAI-compatible endpoint) ===
    ProviderSpec {
        name: "custom",
        keywords: &[],
        env_key: "OPENAI_API_KEY",
        display_name: "Custom",
        litellm_prefix: "openai",
        skip_prefixes: &["openai/"],
        is_gateway: true,
        is_local: false,
        is_oauth: false,
        default_api_base: "",
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        strip_model_prefix: true,
    },
    // === Gateways ===
    ProviderSpec {
        name: "openrouter",
        keywords: &["openrouter"],
        env_key: "OPENROUTER_API_KEY",
        display_name: "OpenRouter",
        litellm_prefix: "openrouter",
        skip_prefixes: &[],
        is_gateway: true,
        is_local: false,
        is_oauth: false,
        default_api_base: "https://openrouter.ai/api/v1",
        detect_by_key_prefix: "sk-or-",
        detect_by_base_keyword: "openrouter",
        strip_model_prefix: false,
    },
    ProviderSpec {
        name: "aihubmix",
        keywords: &["aihubmix"],
        env_key: "OPENAI_API_KEY",
        display_name: "AiHubMix",
        litellm_prefix: "openai",
        skip_prefixes: &[],
        is_gateway: true,
        is_local: false,
        is_oauth: false,
        default_api_base: "https://aihubmix.com/v1",
        detect_by_key_prefix: "",
        detect_by_base_keyword: "aihubmix",
        strip_model_prefix: true,
    },
    // === Standard providers ===
    ProviderSpec {
        name: "anthropic",
        keywords: &["anthropic", "claude"],
        env_key: "ANTHROPIC_API_KEY",
        display_name: "Anthropic",
        litellm_prefix: "",
        skip_prefixes: &[],
        is_gateway: false,
        is_local: false,
        is_oauth: false,
        default_api_base: "",
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        strip_model_prefix: false,
    },
    ProviderSpec {
        name: "openai",
        keywords: &["openai", "gpt"],
        env_key: "OPENAI_API_KEY",
        display_name: "OpenAI",
        litellm_prefix: "",
        skip_prefixes: &[],
        is_gateway: false,
        is_local: false,
        is_oauth: false,
        default_api_base: "",
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        strip_model_prefix: false,
    },
    ProviderSpec {
        name: "openai_codex",
        keywords: &["openai-codex", "codex"],
        env_key: "",
        display_name: "OpenAI Codex",
        litellm_prefix: "",
        skip_prefixes: &[],
        is_gateway: false,
        is_local: false,
        is_oauth: true,
        default_api_base: "https://chatgpt.com/backend-api",
        detect_by_key_prefix: "",
        detect_by_base_keyword: "codex",
        strip_model_prefix: false,
    },
    ProviderSpec {
        name: "deepseek",
        keywords: &["deepseek"],
        env_key: "DEEPSEEK_API_KEY",
        display_name: "DeepSeek",
        litellm_prefix: "deepseek",
        skip_prefixes: &["deepseek/"],
        is_gateway: false,
        is_local: false,
        is_oauth: false,
        default_api_base: "",
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        strip_model_prefix: false,
    },
    ProviderSpec {
        name: "gemini",
        keywords: &["gemini"],
        env_key: "GOOGLE_GEMINI_API_KEY",
        display_name: "Gemini",
        litellm_prefix: "gemini",
        skip_prefixes: &["gemini/"],
        is_gateway: false,
        is_local: false,
        is_oauth: false,
        default_api_base: "",
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        strip_model_prefix: false,
    },
    ProviderSpec {
        name: "zhipu",
        keywords: &["zhipu", "glm", "zai"],
        env_key: "ZAI_API_KEY",
        display_name: "Zhipu AI",
        litellm_prefix: "zai",
        skip_prefixes: &["zhipu/", "zai/", "openrouter/", "hosted_vllm/"],
        is_gateway: false,
        is_local: false,
        is_oauth: false,
        default_api_base: "",
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        strip_model_prefix: false,
    },
    ProviderSpec {
        name: "dashscope",
        keywords: &["qwen", "dashscope"],
        env_key: "DASHSCOPE_API_KEY",
        display_name: "DashScope",
        litellm_prefix: "dashscope",
        skip_prefixes: &["dashscope/", "openrouter/"],
        is_gateway: false,
        is_local: false,
        is_oauth: false,
        default_api_base: "",
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        strip_model_prefix: false,
    },
    ProviderSpec {
        name: "moonshot",
        keywords: &["moonshot", "kimi"],
        env_key: "MOONSHOT_API_KEY",
        display_name: "Moonshot",
        litellm_prefix: "moonshot",
        skip_prefixes: &["moonshot/", "openrouter/"],
        is_gateway: false,
        is_local: false,
        is_oauth: false,
        default_api_base: "https://api.moonshot.ai/v1",
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        strip_model_prefix: false,
    },
    ProviderSpec {
        name: "minimax",
        keywords: &["minimax"],
        env_key: "MINIMAX_API_KEY",
        display_name: "MiniMax",
        litellm_prefix: "minimax",
        skip_prefixes: &["minimax/", "openrouter/"],
        is_gateway: false,
        is_local: false,
        is_oauth: false,
        default_api_base: "https://api.minimax.io/v1",
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        strip_model_prefix: false,
    },
    ProviderSpec {
        name: "vllm",
        keywords: &["vllm"],
        env_key: "HOSTED_VLLM_API_KEY",
        display_name: "vLLM/Local",
        litellm_prefix: "hosted_vllm",
        skip_prefixes: &[],
        is_gateway: false,
        is_local: true,
        is_oauth: false,
        default_api_base: "",
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        strip_model_prefix: false,
    },
    ProviderSpec {
        name: "groq",
        keywords: &["groq"],
        env_key: "GROQ_API_KEY",
        display_name: "Groq",
        litellm_prefix: "groq",
        skip_prefixes: &["groq/"],
        is_gateway: false,
        is_local: false,
        is_oauth: false,
        default_api_base: "",
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        strip_model_prefix: false,
    },
    ProviderSpec {
        name: "xai",
        keywords: &["xai", "grok"],
        env_key: "XAI_API_KEY",
        display_name: "xAI",
        litellm_prefix: "xai",
        skip_prefixes: &["xai/"],
        is_gateway: false,
        is_local: false,
        is_oauth: false,
        default_api_base: "https://api.x.ai/v1",
        detect_by_key_prefix: "xai-",
        detect_by_base_keyword: "x.ai",
        strip_model_prefix: false,
    },
    // === Local / air-gapped providers ===
    ProviderSpec {
        name: "local",
        keywords: &["local"],
        env_key: "LOCAL_LLM_API_KEY",
        display_name: "Local",
        litellm_prefix: "openai",
        skip_prefixes: &["local/"],
        is_gateway: false,
        is_local: true,
        is_oauth: false,
        default_api_base: "http://localhost:11434/v1",
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        strip_model_prefix: true,
    },
    ProviderSpec {
        name: "ollama",
        keywords: &["ollama"],
        env_key: "LOCAL_LLM_API_KEY",
        display_name: "Ollama",
        litellm_prefix: "openai",
        skip_prefixes: &["ollama/", "local/"],
        is_gateway: false,
        is_local: true,
        is_oauth: false,
        default_api_base: "http://localhost:11434/v1",
        detect_by_key_prefix: "",
        detect_by_base_keyword: ":11434",
        strip_model_prefix: true,
    },
    ProviderSpec {
        name: "lmstudio",
        keywords: &["lmstudio", "lm-studio"],
        env_key: "LOCAL_LLM_API_KEY",
        display_name: "LM Studio",
        litellm_prefix: "openai",
        skip_prefixes: &["lmstudio/"],
        is_gateway: false,
        is_local: true,
        is_oauth: false,
        default_api_base: "http://localhost:1234/v1",
        detect_by_key_prefix: "",
        detect_by_base_keyword: ":1234",
        strip_model_prefix: true,
    },
    ProviderSpec {
        name: "llamacpp",
        keywords: &["llamacpp", "llama-cpp", "llama.cpp"],
        env_key: "LOCAL_LLM_API_KEY",
        display_name: "llama.cpp",
        litellm_prefix: "openai",
        skip_prefixes: &["llamacpp/"],
        is_gateway: false,
        is_local: true,
        is_oauth: false,
        default_api_base: "http://localhost:8080/v1",
        detect_by_key_prefix: "",
        detect_by_base_keyword: ":8080",
        strip_model_prefix: true,
    },
];

/// Find a standard provider by model-name keyword (case-insensitive).
///
/// Skips gateways and local providers -- those are matched by
/// API key prefix or base URL instead.
pub fn find_by_model(model: &str) -> Option<&'static ProviderSpec> {
    let model_lower = model.to_lowercase();
    PROVIDERS.iter().find(|spec| {
        !spec.is_gateway
            && !spec.is_local
            && spec.keywords.iter().any(|kw| model_lower.contains(kw))
    })
}

/// Detect a gateway or local provider.
///
/// Priority:
/// 1. `provider_name` -- if it maps to a gateway/local spec, use it directly.
/// 2. `api_key` prefix -- e.g. `"sk-or-"` matches OpenRouter.
/// 3. `api_base` keyword -- e.g. `"aihubmix"` in the URL matches AiHubMix.
pub fn find_gateway(
    provider_name: Option<&str>,
    api_key: Option<&str>,
    api_base: Option<&str>,
) -> Option<&'static ProviderSpec> {
    // 1. Direct match by config key
    if let Some(name) = provider_name
        && let Some(spec) = find_by_name(name)
        && (spec.is_gateway || spec.is_local)
    {
        return Some(spec);
    }

    // 2. Auto-detect by api_key prefix / api_base keyword
    for spec in PROVIDERS {
        if !spec.detect_by_key_prefix.is_empty()
            && let Some(key) = api_key
            && key.starts_with(spec.detect_by_key_prefix)
        {
            return Some(spec);
        }
        if !spec.detect_by_base_keyword.is_empty()
            && let Some(base) = api_base
            && base.contains(spec.detect_by_base_keyword)
        {
            return Some(spec);
        }
    }

    None
}

/// Find a provider spec by config field name (e.g. `"dashscope"`).
pub fn find_by_name(name: &str) -> Option<&'static ProviderSpec> {
    PROVIDERS.iter().find(|spec| spec.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_count() {
        assert_eq!(PROVIDERS.len(), 19);
    }

    #[test]
    fn find_anthropic_by_model() {
        let spec = find_by_model("anthropic/claude-opus-4-5").unwrap();
        assert_eq!(spec.name, "anthropic");
    }

    #[test]
    fn find_deepseek_by_model() {
        let spec = find_by_model("deepseek-chat").unwrap();
        assert_eq!(spec.name, "deepseek");
    }

    #[test]
    fn find_by_model_skips_gateways() {
        // "openrouter" is a keyword for the openrouter gateway but
        // find_by_model should skip gateways.
        let spec = find_by_model("openrouter/some-model");
        assert!(spec.is_none());
    }

    #[test]
    fn find_gateway_by_key_prefix() {
        let spec = find_gateway(None, Some("sk-or-abc123"), None).unwrap();
        assert_eq!(spec.name, "openrouter");
    }

    #[test]
    fn find_gateway_by_base_keyword() {
        let spec = find_gateway(None, None, Some("https://aihubmix.com/v1")).unwrap();
        assert_eq!(spec.name, "aihubmix");
    }

    #[test]
    fn find_gateway_by_name() {
        let spec = find_gateway(Some("vllm"), None, None).unwrap();
        assert_eq!(spec.name, "vllm");
        assert!(spec.is_local);
    }

    #[test]
    fn find_by_name_existing() {
        let spec = find_by_name("moonshot").unwrap();
        assert_eq!(spec.display_name, "Moonshot");
        assert_eq!(spec.default_api_base, "https://api.moonshot.ai/v1");
    }

    #[test]
    fn find_by_name_missing() {
        assert!(find_by_name("nonexistent").is_none());
    }

    #[test]
    fn provider_spec_label() {
        let spec = find_by_name("anthropic").unwrap();
        assert_eq!(spec.label(), "Anthropic");

        // custom has a display_name
        let spec = find_by_name("custom").unwrap();
        assert_eq!(spec.label(), "Custom");
    }

    #[test]
    fn openai_codex_is_oauth() {
        let spec = find_by_name("openai_codex").unwrap();
        assert!(spec.is_oauth);
        assert!(spec.env_key.is_empty());
    }

    #[test]
    fn find_local_by_name() {
        let spec = find_by_name("local").unwrap();
        assert!(spec.is_local);
        assert_eq!(spec.display_name, "Local");
        assert_eq!(spec.default_api_base, "http://localhost:11434/v1");
    }

    #[test]
    fn find_ollama_by_name() {
        let spec = find_by_name("ollama").unwrap();
        assert!(spec.is_local);
        assert_eq!(spec.display_name, "Ollama");
        assert_eq!(spec.default_api_base, "http://localhost:11434/v1");
    }

    #[test]
    fn find_lmstudio_by_name() {
        let spec = find_by_name("lmstudio").unwrap();
        assert!(spec.is_local);
        assert_eq!(spec.display_name, "LM Studio");
        assert_eq!(spec.default_api_base, "http://localhost:1234/v1");
    }

    #[test]
    fn find_llamacpp_by_name() {
        let spec = find_by_name("llamacpp").unwrap();
        assert!(spec.is_local);
        assert_eq!(spec.display_name, "llama.cpp");
        assert_eq!(spec.default_api_base, "http://localhost:8080/v1");
    }

    #[test]
    fn find_gateway_detects_local_by_name() {
        let spec = find_gateway(Some("local"), None, None).unwrap();
        assert_eq!(spec.name, "local");
        assert!(spec.is_local);
    }

    #[test]
    fn find_gateway_detects_ollama_by_name() {
        let spec = find_gateway(Some("ollama"), None, None).unwrap();
        assert_eq!(spec.name, "ollama");
        assert!(spec.is_local);
    }

    #[test]
    fn find_gateway_detects_ollama_by_port() {
        let spec = find_gateway(None, None, Some("http://192.168.1.5:11434/v1")).unwrap();
        assert_eq!(spec.name, "ollama");
    }

    #[test]
    fn find_gateway_detects_lmstudio_by_port() {
        let spec = find_gateway(None, None, Some("http://localhost:1234/v1")).unwrap();
        assert_eq!(spec.name, "lmstudio");
    }

    #[test]
    fn all_local_providers_are_local() {
        for name in &["local", "ollama", "lmstudio", "llamacpp", "vllm"] {
            let spec = find_by_name(name).unwrap();
            assert!(spec.is_local, "provider {} should be marked as local", name);
        }
    }

    #[test]
    fn llm_response_serde_roundtrip() {
        let resp = LlmResponse {
            id: "resp-001".into(),
            content: vec![ContentBlock::Text {
                text: "Hello!".into(),
            }],
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
            },
            metadata: HashMap::new(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let restored: LlmResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, "resp-001");
        assert_eq!(restored.stop_reason, StopReason::EndTurn);
        assert_eq!(restored.usage.input_tokens, 10);
        assert_eq!(restored.usage.total(), 15);
    }

    #[test]
    fn usage_total_computed() {
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 0,
        };
        assert_eq!(usage.total(), 15);
    }

    #[test]
    fn usage_total_from_provider() {
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 20, // provider may count differently
        };
        assert_eq!(usage.total(), 20);
    }

    #[test]
    fn usage_deserializes_from_openai_field_names() {
        let json = r#"{"prompt_tokens": 100, "completion_tokens": 50, "total_tokens": 150}"#;
        let usage: Usage = serde_json::from_str(json).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn usage_deserializes_from_canonical_field_names() {
        let json = r#"{"input_tokens": 100, "output_tokens": 50, "total_tokens": 150}"#;
        let usage: Usage = serde_json::from_str(json).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn usage_deserializes_without_total() {
        let json = r#"{"input_tokens": 100, "output_tokens": 50}"#;
        let usage: Usage = serde_json::from_str(json).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.total_tokens, 0);
        assert_eq!(usage.total(), 150);
    }

    #[test]
    fn content_block_tool_use_serde() {
        let block = ContentBlock::ToolUse {
            id: "call-1".into(),
            name: "web_search".into(),
            input: serde_json::json!({"query": "rust"}),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains(r#""type":"tool_use""#));
        let restored: ContentBlock = serde_json::from_str(&json).unwrap();
        match restored {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call-1");
                assert_eq!(name, "web_search");
                assert_eq!(input["query"], "rust");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn stop_reason_serde() {
        let reasons = [
            (StopReason::EndTurn, "\"end_turn\""),
            (StopReason::MaxTokens, "\"max_tokens\""),
            (StopReason::StopSequence, "\"stop_sequence\""),
            (StopReason::ToolUse, "\"tool_use\""),
        ];
        for (reason, expected_json) in &reasons {
            let json = serde_json::to_string(reason).unwrap();
            assert_eq!(&json, expected_json);
            let restored: StopReason = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, *reason);
        }
    }

    #[test]
    fn tool_call_request_serde() {
        let req = ToolCallRequest {
            id: "tc-1".into(),
            name: "exec".into(),
            input: serde_json::json!({"command": "ls"}),
        };
        let json = serde_json::to_string(&req).unwrap();
        let restored: ToolCallRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "exec");
    }
}
