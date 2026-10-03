//! Prefix-based model-to-provider routing.
//!
//! The [`ProviderRouter`] maps model name prefixes (e.g. "openai/", "anthropic/")
//! to their respective providers, enabling a single entry point for any model.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::config::{self, LlmProviderConfig};
use crate::openai_compat::OpenAiCompatProvider;
use crate::placement::{CachedResolver, PlacedProvider, PlacementResolver};
use crate::provider::Provider;

/// Routes model names to providers based on prefix matching.
///
/// When a model name like "openai/gpt-4o" is requested, the router:
/// 1. Extracts the prefix "openai/"
/// 2. Looks up the provider registered for that prefix
/// 3. Strips the prefix and forwards "gpt-4o" to the provider
///
/// If no prefix matches, the default provider is used.
///
/// # Example
///
/// ```rust,ignore
/// use clawft_llm::ProviderRouter;
///
/// let router = ProviderRouter::with_builtins();
/// let (provider, model) = router.route("anthropic/claude-sonnet-4-5-20250514").unwrap();
/// assert_eq!(provider.name(), "anthropic");
/// assert_eq!(model, "claude-sonnet-4-5-20250514");
/// ```
pub struct ProviderRouter {
    /// Provider instances keyed by provider name.
    providers: HashMap<String, Box<dyn Provider>>,
    /// Mapping of prefix strings to provider names, sorted longest-first
    /// for greedy matching.
    prefix_map: Vec<(String, String)>,
    /// The name of the default provider (used when no prefix matches).
    default_provider: String,
    /// Original configs by provider name (kept to build placed providers).
    configs: HashMap<String, LlmProviderConfig>,
    /// TTL cache over the placement resolver, when one is attached.
    placement: Option<Arc<CachedResolver>>,
}

impl ProviderRouter {
    /// Create a router from a list of provider configurations.
    ///
    /// The first provider in the list becomes the default.
    pub fn from_configs(configs: Vec<LlmProviderConfig>) -> Self {
        let default_provider = configs.first().map(|c| c.name.clone()).unwrap_or_default();

        let mut providers: HashMap<String, Box<dyn Provider>> = HashMap::new();
        let mut prefix_map: Vec<(String, String)> = Vec::new();

        let mut kept: HashMap<String, LlmProviderConfig> = HashMap::new();
        for config in configs {
            let name = config.name.clone();
            kept.insert(name.clone(), config.clone());
            if let Some(ref prefix) = config.model_prefix {
                prefix_map.push((prefix.clone(), name.clone()));
            }
            providers.insert(name, Box::new(OpenAiCompatProvider::new(config)));
        }

        // Sort by prefix length descending for greedy matching
        // (e.g. "openai/o1/" should match before "openai/")
        prefix_map.sort_by_key(|b| std::cmp::Reverse(b.0.len()));

        Self {
            providers,
            prefix_map,
            default_provider,
            configs: kept,
            placement: None,
        }
    }

    /// Attach a [`PlacementResolver`] (ADR-101 section 5). Each
    /// `(provider name, role)` pair makes that provider follow placement:
    /// its endpoint is resolved per request through a TTL cache, and its
    /// configured `base_url` remains the fallback.
    ///
    /// Precedence is the caller's: leave a provider out of `roles` when its
    /// endpoint was set explicitly (env or `[kernel.llm]`), and it is never
    /// placed. Unknown provider names are ignored.
    pub fn with_placement(
        mut self,
        resolver: Arc<dyn PlacementResolver>,
        ttl: Duration,
        roles: &[(&str, &str)],
    ) -> Self {
        let cache = Arc::new(CachedResolver::new(resolver, ttl));
        for (provider, role) in roles {
            if let Some(cfg) = self.configs.get(*provider) {
                let placed = PlacedProvider::new((*role).to_string(), cfg.clone(), cache.clone());
                self.providers.insert((*provider).to_string(), Box::new(placed));
            }
        }
        self.placement = Some(cache);
        self
    }

    /// Drop cached placement answers for `role` (call on a mesh peer or
    /// service-advertisement change). No-op without a resolver.
    pub fn invalidate_placement(&self, role: &str) {
        if let Some(c) = &self.placement {
            c.invalidate(role);
        }
    }

    /// Drop every cached placement answer. No-op without a resolver.
    pub fn invalidate_all_placement(&self) {
        if let Some(c) = &self.placement {
            c.invalidate_all();
        }
    }

    /// Create a router with all built-in provider configurations.
    pub fn with_builtins() -> Self {
        Self::from_configs(config::builtin_providers())
    }

    /// Route a model name to its provider, returning the provider and the
    /// model name with the prefix stripped.
    ///
    /// Returns `None` if no provider could be found (neither by prefix
    /// nor default).
    pub fn route(&self, model: &str) -> Option<(&dyn Provider, String)> {
        // Check prefix matches first
        for (prefix, provider_name) in &self.prefix_map {
            if model.starts_with(prefix.as_str()) {
                let stripped = model[prefix.len()..].to_string();
                if let Some(provider) = self.providers.get(provider_name) {
                    return Some((provider.as_ref(), stripped));
                }
            }
        }

        // Fall back to default provider (model name unchanged)
        self.providers
            .get(&self.default_provider)
            .map(|p| (p.as_ref(), model.to_string()))
    }

    /// Split a model string into its optional prefix and the bare model name.
    ///
    /// Uses the first `/` as the separator. If no `/` is present, the entire
    /// string is the model name with no prefix.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let (prefix, model) = ProviderRouter::strip_prefix("openai/gpt-4o");
    /// assert_eq!(prefix, Some("openai".to_string()));
    /// assert_eq!(model, "gpt-4o");
    ///
    /// let (prefix, model) = ProviderRouter::strip_prefix("gpt-4o");
    /// assert_eq!(prefix, None);
    /// assert_eq!(model, "gpt-4o");
    /// ```
    pub fn strip_prefix(model: &str) -> (Option<String>, String) {
        if let Some(slash_pos) = model.find('/') {
            let prefix = model[..slash_pos].to_string();
            let rest = model[slash_pos + 1..].to_string();
            (Some(prefix), rest)
        } else {
            (None, model.to_string())
        }
    }

    /// List the names of all registered providers.
    pub fn providers(&self) -> Vec<String> {
        let mut names: Vec<String> = self.providers.keys().cloned().collect();
        names.sort();
        names
    }

    /// Returns the name of the default provider.
    pub fn default_provider(&self) -> &str {
        &self.default_provider
    }

    /// Look up a provider by name directly, bypassing prefix routing.
    pub fn get_provider(&self, name: &str) -> Option<&dyn Provider> {
        self.providers.get(name).map(|p| p.as_ref())
    }
}

impl std::fmt::Debug for ProviderRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRouter")
            .field("providers", &self.providers.keys().collect::<Vec<_>>())
            .field("prefix_map", &self.prefix_map)
            .field("default_provider", &self.default_provider)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_configs() -> Vec<LlmProviderConfig> {
        vec![
            LlmProviderConfig {
                name: "openai".into(),
                base_url: "https://api.openai.com/v1".into(),
                api_key_env: "OPENAI_API_KEY".into(),
                model_prefix: Some("openai/".into()),
                default_model: Some("gpt-4o".into()),
                headers: HashMap::new(),
                timeout_secs: None,
            },
            LlmProviderConfig {
                name: "anthropic".into(),
                base_url: "https://api.anthropic.com/v1".into(),
                api_key_env: "ANTHROPIC_API_KEY".into(),
                model_prefix: Some("anthropic/".into()),
                default_model: None,
                headers: HashMap::new(),
                timeout_secs: None,
            },
            LlmProviderConfig {
                name: "groq".into(),
                base_url: "https://api.groq.com/openai/v1".into(),
                api_key_env: "GROQ_API_KEY".into(),
                model_prefix: Some("groq/".into()),
                default_model: None,
                headers: HashMap::new(),
                timeout_secs: None,
            },
        ]
    }

    #[test]
    fn route_with_prefix() {
        let router = ProviderRouter::from_configs(test_configs());

        let (provider, model) = router.route("openai/gpt-4o").unwrap();
        assert_eq!(provider.name(), "openai");
        assert_eq!(model, "gpt-4o");

        let (provider, model) = router
            .route("anthropic/claude-sonnet-4-5-20250514")
            .unwrap();
        assert_eq!(provider.name(), "anthropic");
        assert_eq!(model, "claude-sonnet-4-5-20250514");

        let (provider, model) = router.route("groq/llama-3.1-70b").unwrap();
        assert_eq!(provider.name(), "groq");
        assert_eq!(model, "llama-3.1-70b");
    }

    #[test]
    fn route_without_prefix_uses_default() {
        let router = ProviderRouter::from_configs(test_configs());

        let (provider, model) = router.route("gpt-4o").unwrap();
        assert_eq!(provider.name(), "openai"); // first config is default
        assert_eq!(model, "gpt-4o");
    }

    #[test]
    fn route_unknown_prefix_uses_default() {
        let router = ProviderRouter::from_configs(test_configs());

        let (provider, model) = router.route("unknown/model-x").unwrap();
        assert_eq!(provider.name(), "openai"); // falls back to default
        assert_eq!(model, "unknown/model-x"); // full string preserved
    }

    #[test]
    fn default_provider_is_first() {
        let router = ProviderRouter::from_configs(test_configs());
        assert_eq!(router.default_provider(), "openai");
    }

    #[test]
    fn strip_prefix_with_slash() {
        let (prefix, model) = ProviderRouter::strip_prefix("openai/gpt-4o");
        assert_eq!(prefix, Some("openai".to_string()));
        assert_eq!(model, "gpt-4o");
    }

    #[test]
    fn strip_prefix_without_slash() {
        let (prefix, model) = ProviderRouter::strip_prefix("gpt-4o");
        assert_eq!(prefix, None);
        assert_eq!(model, "gpt-4o");
    }

    #[test]
    fn strip_prefix_with_nested_slash() {
        let (prefix, model) = ProviderRouter::strip_prefix("openrouter/meta/llama-3-70b");
        assert_eq!(prefix, Some("openrouter".to_string()));
        assert_eq!(model, "meta/llama-3-70b");
    }

    #[test]
    fn providers_list() {
        let router = ProviderRouter::from_configs(test_configs());
        let providers = router.providers();
        assert_eq!(providers.len(), 3);
        // Sorted alphabetically
        assert_eq!(providers, vec!["anthropic", "groq", "openai"]);
    }

    #[test]
    fn get_provider_by_name() {
        let router = ProviderRouter::from_configs(test_configs());

        let provider = router.get_provider("anthropic").unwrap();
        assert_eq!(provider.name(), "anthropic");

        assert!(router.get_provider("nonexistent").is_none());
    }

    #[test]
    fn empty_configs() {
        let router = ProviderRouter::from_configs(vec![]);
        assert!(router.route("anything").is_none());
        assert!(router.providers().is_empty());
        assert_eq!(router.default_provider(), "");
    }

    #[test]
    fn with_builtins_has_all_providers() {
        let router = ProviderRouter::with_builtins();
        let providers = router.providers();
        assert_eq!(providers.len(), 11);
        assert!(providers.contains(&"openai".to_string()));
        assert!(providers.contains(&"anthropic".to_string()));
        assert!(providers.contains(&"groq".to_string()));
        assert!(providers.contains(&"deepseek".to_string()));
        assert!(providers.contains(&"mistral".to_string()));
        assert!(providers.contains(&"together".to_string()));
        assert!(providers.contains(&"openrouter".to_string()));
        assert!(providers.contains(&"gemini".to_string()));
        assert!(providers.contains(&"xai".to_string()));
        assert!(providers.contains(&"local".to_string()));
        assert!(providers.contains(&"ollama".to_string()));
    }

    #[test]
    fn with_builtins_routes_correctly() {
        let router = ProviderRouter::with_builtins();

        let (p, m) = router.route("openai/gpt-4o").unwrap();
        assert_eq!(p.name(), "openai");
        assert_eq!(m, "gpt-4o");

        let (p, m) = router.route("deepseek/deepseek-chat").unwrap();
        assert_eq!(p.name(), "deepseek");
        assert_eq!(m, "deepseek-chat");
    }

    #[test]
    fn debug_output() {
        let router = ProviderRouter::from_configs(test_configs());
        let debug = format!("{:?}", router);
        assert!(debug.contains("ProviderRouter"));
        assert!(debug.contains("openai"));
    }

    #[test]
    fn prefix_no_provider_without_prefix() {
        // Config with no prefix
        let configs = vec![LlmProviderConfig {
            name: "custom".into(),
            base_url: "https://custom.example.com".into(),
            api_key_env: "CUSTOM_KEY".into(),
            model_prefix: None,
            default_model: None,
            headers: HashMap::new(),
            timeout_secs: None,
        }];
        let router = ProviderRouter::from_configs(configs);
        // Should still work via default fallback
        let (provider, model) = router.route("some-model").unwrap();
        assert_eq!(provider.name(), "custom");
        assert_eq!(model, "some-model");
    }
}
