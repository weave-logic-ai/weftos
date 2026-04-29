//! Static router (Level 0 implementation).
//!
//! Always routes to the configured default provider/model pair.
//! The `update` method is a no-op; adaptive routing is left for
//! Level 1+ implementations.

use async_trait::async_trait;

use clawft_types::config::AgentsConfig;

use super::traits::{ChatRequest, ModelRouter, ResponseOutcome, RoutingDecision, TaskProfile};

/// Level 0 static router that always returns the same provider/model.
///
/// Configured either directly via [`StaticRouter::new`] or from the
/// agent defaults via [`StaticRouter::from_config`].
pub struct StaticRouter {
    default_provider: String,
    default_model: String,
}

impl StaticRouter {
    /// Create a router with an explicit provider and model.
    pub fn new(provider: String, model: String) -> Self {
        Self {
            default_provider: provider,
            default_model: model,
        }
    }

    /// Create a router from the agent defaults in configuration.
    ///
    /// The model string is expected in `"provider/model"` format
    /// (e.g. `"anthropic/claude-opus-4-5"`). If no slash is present,
    /// the provider defaults to `"openai"`.
    pub fn from_config(config: &AgentsConfig) -> Self {
        let model_str = &config.defaults.model;
        let (provider, model) = split_provider_model(model_str);
        Self {
            default_provider: provider,
            default_model: model,
        }
    }

    /// Returns the configured provider name.
    pub fn provider(&self) -> &str {
        &self.default_provider
    }

    /// Returns the configured model name.
    pub fn model(&self) -> &str {
        &self.default_model
    }
}

#[async_trait]
impl ModelRouter for StaticRouter {
    async fn route(&self, _request: &ChatRequest, _profile: &TaskProfile) -> RoutingDecision {
        RoutingDecision {
            provider: self.default_provider.clone(),
            model: self.default_model.clone(),
            reason: "static routing (Level 0)".into(),
            ..Default::default()
        }
    }

    fn update(&self, _decision: &RoutingDecision, _outcome: &ResponseOutcome) {
        // No-op: static router does not learn from outcomes.
    }
}

/// Split a `"provider/model"` string into `(provider, model)`.
///
/// If the string contains no slash, the provider defaults to `"openai"`.
fn split_provider_model(s: &str) -> (String, String) {
    if let Some(idx) = s.find('/') {
        let provider = &s[..idx];
        let model = &s[idx + 1..];
        (provider.to_string(), model.to_string())
    } else {
        ("openai".to_string(), s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::traits::{LlmMessage, TaskType};
    use clawft_types::config::AgentDefaults;

    fn make_request() -> ChatRequest {
        ChatRequest {
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "hello".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![],
            model: None,
            max_tokens: None,
            temperature: None,
            auth_context: None,
            complexity_boost: 0.0,
        }
    }

    fn make_profile() -> TaskProfile {
        TaskProfile {
            task_type: TaskType::Chat,
            complexity: 0.3,
            keywords: vec![],
        }
    }

    #[tokio::test]
    async fn route_returns_configured_values() {
        let router = StaticRouter::new("anthropic".into(), "claude-opus-4-5".into());
        let decision = router.route(&make_request(), &make_profile()).await;
        assert_eq!(decision.provider, "anthropic");
        assert_eq!(decision.model, "claude-opus-4-5");
        assert!(decision.reason.contains("static"));
    }

    #[tokio::test]
    async fn route_ignores_request_content() {
        let router = StaticRouter::new("openai".into(), "gpt-4o".into());
        let req = ChatRequest {
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "very complex code generation task".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![serde_json::json!({"type": "function"})],
            model: Some("different-model".into()),
            max_tokens: Some(9999),
            temperature: Some(0.0),
            auth_context: None,
            complexity_boost: 0.0,
        };
        let profile = TaskProfile {
            task_type: TaskType::CodeGeneration,
            complexity: 0.9,
            keywords: vec!["code".into()],
        };
        let decision = router.route(&req, &profile).await;
        // Static router always returns the configured values regardless of input.
        assert_eq!(decision.provider, "openai");
        assert_eq!(decision.model, "gpt-4o");
    }

    #[test]
    fn from_config_splits_provider_model() {
        let config = AgentsConfig {
            defaults: AgentDefaults {
                model: "anthropic/claude-opus-4-5".into(),
                ..AgentDefaults::default()
            },
            ..AgentsConfig::default()
        };
        let router = StaticRouter::from_config(&config);
        assert_eq!(router.provider(), "anthropic");
        assert_eq!(router.model(), "claude-opus-4-5");
    }

    #[test]
    fn from_config_defaults_provider_to_openai() {
        let config = AgentsConfig {
            defaults: AgentDefaults {
                model: "gpt-4o".into(),
                ..AgentDefaults::default()
            },
            ..AgentsConfig::default()
        };
        let router = StaticRouter::from_config(&config);
        assert_eq!(router.provider(), "openai");
        assert_eq!(router.model(), "gpt-4o");
    }

    #[test]
    fn from_config_default_agents_config() {
        let config = AgentsConfig::default();
        let router = StaticRouter::from_config(&config);
        assert_eq!(router.provider(), "deepseek");
        assert_eq!(router.model(), "deepseek-chat");
    }

    #[test]
    fn split_provider_model_with_slash() {
        let (p, m) = split_provider_model("deepseek/deepseek-chat");
        assert_eq!(p, "deepseek");
        assert_eq!(m, "deepseek-chat");
    }

    #[test]
    fn split_provider_model_without_slash() {
        let (p, m) = split_provider_model("gpt-4o-mini");
        assert_eq!(p, "openai");
        assert_eq!(m, "gpt-4o-mini");
    }

    #[test]
    fn split_provider_model_multiple_slashes() {
        // First slash is used as the split point.
        let (p, m) = split_provider_model("openrouter/anthropic/claude-opus-4-5");
        assert_eq!(p, "openrouter");
        assert_eq!(m, "anthropic/claude-opus-4-5");
    }

    #[test]
    fn update_is_noop() {
        let router = StaticRouter::new("test".into(), "test".into());
        use crate::pipeline::traits::QualityScore;
        let decision = RoutingDecision {
            provider: "test".into(),
            model: "test".into(),
            reason: "test".into(),
            ..Default::default()
        };
        let outcome = ResponseOutcome {
            success: true,
            quality: QualityScore {
                overall: 1.0,
                relevance: 1.0,
                coherence: 1.0,
            },
            latency_ms: 100,
        };
        // Should not panic.
        router.update(&decision, &outcome);
    }

    #[test]
    fn accessor_methods() {
        let router = StaticRouter::new("groq".into(), "llama-3".into());
        assert_eq!(router.provider(), "groq");
        assert_eq!(router.model(), "llama-3");
    }
}
