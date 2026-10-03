//! WEFT-604 — bridge `[kernel.llm]` (+ env) into the CLI agent path.
//!
//! The daemon already resolves URL/model with provenance logging
//! (`clawft-weave::daemon`). Standalone `weft agent` historically ignored
//! `[kernel.llm]` and `LLM_SERVICE_URL`, defaulting to a cloud model that
//! demanded an API key. This module is the shared stamp so both paths
//! share one local-LLM source of truth (ADR-060 / `clawft_types::config`
//! constants).

use clawft_types::config::{
    api_base_to_service_url, service_url_to_api_base, Config, DEFAULT_LOCAL_LLM_API_BASE,
    DEFAULT_LOCAL_LLM_MODEL, DEFAULT_LOCAL_LLM_MODEL_ROUTED, DEFAULT_LOCAL_LLM_SERVICE_URL,
    LLM_MODEL_ENV, LLM_SERVICE_URL_ENV,
};

/// Provenance for the resolved local-LLM endpoint + agent model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalLlmResolution {
    /// Service URL without `/v1` (daemon / service-llm shape).
    pub service_url: String,
    /// OpenAI-compat API base with `/v1` (clawft-llm shape).
    pub api_base: String,
    /// Model name for request bodies / `agents.defaults.model` bare alias.
    pub model: String,
    /// Routed model string stamped onto `agents.defaults.model`.
    pub routed_model: String,
    /// Which layer supplied the URL.
    pub url_source: &'static str,
    /// Which layer supplied the model alias.
    pub model_source: &'static str,
    /// Which layer supplied `agents.defaults.model` before the bridge.
    pub agents_model_source: &'static str,
}

/// Resolve URL + model from env → `[kernel.llm]` → ADR-060 defaults.
///
/// Does **not** mutate `config`. See [`apply_local_llm_bridge`] to stamp.
pub fn resolve_local_llm(config: &Config, agents_model_source: &'static str) -> LocalLlmResolution {
    let cfg_url = config
        .kernel
        .llm
        .as_ref()
        .and_then(|c| c.service_url.clone())
        .filter(|s| !s.is_empty());
    let cfg_model = config
        .kernel
        .llm
        .as_ref()
        .and_then(|c| c.model.clone())
        .filter(|s| !s.is_empty());

    let env_url = std::env::var(LLM_SERVICE_URL_ENV)
        .ok()
        .filter(|s| !s.is_empty());
    let env_model = std::env::var(LLM_MODEL_ENV)
        .ok()
        .filter(|s| !s.is_empty());

    let url_from_env = env_url.is_some();
    let url_from_cfg = cfg_url.is_some();
    let model_from_env = env_model.is_some();
    let model_from_cfg = cfg_model.is_some();

    let service_url = env_url
        .or(cfg_url)
        .unwrap_or_else(|| DEFAULT_LOCAL_LLM_SERVICE_URL.to_string());
    // Normalize: accept either shape from env/config.
    let service_url = api_base_to_service_url(&service_url);
    let api_base = service_url_to_api_base(&service_url);

    let model = env_model
        .or(cfg_model)
        .unwrap_or_else(|| DEFAULT_LOCAL_LLM_MODEL.to_string());
    // Strip a `local/` prefix if the operator put a routed form in kernel.llm.
    let model = model
        .strip_prefix("local/")
        .unwrap_or(model.as_str())
        .to_string();
    let routed_model = if model.contains('/') {
        model.clone()
    } else {
        format!("local/{model}")
    };

    let url_source = if url_from_env {
        "env:LLM_SERVICE_URL"
    } else if url_from_cfg {
        "config:[kernel.llm].service_url"
    } else {
        "default:local-adr060"
    };
    let model_source = if model_from_env {
        "env:LLM_MODEL"
    } else if model_from_cfg {
        "config:[kernel.llm].model"
    } else {
        "default:local-adr060"
    };

    // If agents.defaults.model already points at a non-default/non-legacy
    // value and kernel.llm did not supply a model, keep the agents value
    // as the routed model (operator explicit choice).
    let agents_model = config.agents.defaults.model.clone();
    let use_agents = !model_from_env
        && !model_from_cfg
        && agents_model != DEFAULT_LOCAL_LLM_MODEL_ROUTED
        && agents_model != DEFAULT_LOCAL_LLM_MODEL
        && !clawft_types::config::is_legacy_cloud_default_model(&agents_model);

    let (routed_model, model_source, model) = if use_agents {
        let bare = agents_model
            .rsplit_once('/')
            .map(|(_, m)| m.to_string())
            .unwrap_or_else(|| agents_model.clone());
        (agents_model, agents_model_source, bare)
    } else {
        (routed_model, model_source, model)
    };

    LocalLlmResolution {
        service_url,
        api_base,
        model,
        routed_model,
        url_source,
        model_source,
        agents_model_source,
    }
}

/// Stamp resolved local-LLM settings into `config` so the CLI agent path
/// and provider registry share the daemon's endpoint.
///
/// - Sets `agents.defaults.model` to the routed form (`local/…`) when the
///   current value is the ADR-060 default, a legacy cloud default, or
///   when `[kernel.llm]` / env supplied a model.
/// - Ensures `providers.local.api_base` points at the resolved endpoint
///   when the operator has not already set one.
pub fn apply_local_llm_bridge(
    config: &mut Config,
    agents_model_source: &'static str,
) -> LocalLlmResolution {
    let resolution = resolve_local_llm(config, agents_model_source);

    let agents_is_defaultish = config.agents.defaults.model == DEFAULT_LOCAL_LLM_MODEL_ROUTED
        || config.agents.defaults.model == DEFAULT_LOCAL_LLM_MODEL
        || clawft_types::config::is_legacy_cloud_default_model(&config.agents.defaults.model)
        || resolution.model_source.starts_with("env:")
        || resolution.model_source.starts_with("config:[kernel.llm]");

    if agents_is_defaultish {
        config.agents.defaults.model = resolution.routed_model.clone();
    }

    // Stamp local api_base when unset so create_adapter_from_config honors
    // the same endpoint the daemon uses.
    let local_base_empty = config
        .providers
        .local
        .api_base
        .as_ref()
        .map(|s| s.is_empty())
        .unwrap_or(true);
    if local_base_empty {
        // Prefer resolved api_base; if service_url was default, this is
        // DEFAULT_LOCAL_LLM_API_BASE.
        let base = if resolution.api_base.is_empty() {
            DEFAULT_LOCAL_LLM_API_BASE.to_string()
        } else {
            resolution.api_base.clone()
        };
        config.providers.local.api_base = Some(base);
    }

    resolution
}

/// May the `local/` provider follow placement (ADR-101 section 5)?
///
/// Precedence is env, then `[kernel.llm]` / `[providers.local]`, then
/// placement, then the ADR-060 constants. Placement therefore applies only
/// when nothing explicit chose the endpoint: no `LLM_SERVICE_URL`, no
/// `[kernel.llm].service_url`, and no `[providers.local].api_base` other
/// than the ADR-060 default (which the bridge stamps itself). Call after
/// [`apply_local_llm_bridge`] with its resolution.
pub fn local_placement_allowed(config: &Config, resolution: &LocalLlmResolution) -> bool {
    resolution.url_source == "default:local-adr060"
        && config
            .providers
            .local
            .api_base
            .as_deref()
            .is_none_or(|b| b.is_empty() || b.trim_end_matches('/') == DEFAULT_LOCAL_LLM_API_BASE)
}

/// Human-readable one-liner for logs (agent CLI + status).
pub fn format_resolution_log(r: &LocalLlmResolution) -> String {
    format!(
        "url={} ({}) model={} ({}) agents.defaults.model layer={}",
        r.service_url, r.url_source, r.routed_model, r.model_source, r.agents_model_source
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_types::config::{KernelConfig, LlmEndpointConfig};

    #[test]
    fn placement_applies_only_when_nothing_explicit_chose_the_endpoint() {
        let mut config = Config::default();
        let r = apply_local_llm_bridge(&mut config, "default");
        if r.url_source == "default:local-adr060" {
            assert!(local_placement_allowed(&config, &r));
        }
        // Explicit providers.local.api_base wins.
        config.providers.local.api_base = Some("http://127.0.0.1:9100/v1".into());
        let r = resolve_local_llm(&config, "default");
        if r.url_source == "default:local-adr060" {
            assert!(!local_placement_allowed(&config, &r));
        }
        // [kernel.llm].service_url wins.
        let mut config = Config {
            kernel: KernelConfig {
                llm: Some(LlmEndpointConfig {
                    service_url: Some("http://127.0.0.1:9000".into()),
                    model: None,
                }),
                ..KernelConfig::default()
            },
            ..Config::default()
        };
        let r = apply_local_llm_bridge(&mut config, "default");
        assert!(!local_placement_allowed(&config, &r));
        // An env-sourced URL (LLM_SERVICE_URL) wins.
        let mut r = r;
        r.url_source = "env:LLM_SERVICE_URL";
        let config = Config::default();
        assert!(!local_placement_allowed(&config, &r));
    }

    #[test]
    fn defaults_resolve_to_adr060() {
        let config = Config::default();
        let r = resolve_local_llm(&config, "default");
        assert_eq!(r.service_url, DEFAULT_LOCAL_LLM_SERVICE_URL);
        assert_eq!(r.api_base, DEFAULT_LOCAL_LLM_API_BASE);
        assert_eq!(r.model, DEFAULT_LOCAL_LLM_MODEL);
        assert_eq!(r.routed_model, DEFAULT_LOCAL_LLM_MODEL_ROUTED);
        assert_eq!(r.url_source, "default:local-adr060");
    }

    #[test]
    fn kernel_llm_overrides_defaults() {
        let mut config = Config::default();
        config.kernel = KernelConfig {
            llm: Some(LlmEndpointConfig {
                service_url: Some("http://127.0.0.1:9000".into()),
                model: Some("my-model".into()),
            }),
            ..KernelConfig::default()
        };
        let r = resolve_local_llm(&config, "default");
        assert_eq!(r.service_url, "http://127.0.0.1:9000");
        assert_eq!(r.api_base, "http://127.0.0.1:9000/v1");
        assert_eq!(r.model, "my-model");
        assert_eq!(r.routed_model, "local/my-model");
        assert_eq!(r.url_source, "config:[kernel.llm].service_url");
        assert_eq!(r.model_source, "config:[kernel.llm].model");
    }

    #[test]
    fn bridge_stamps_agents_and_local_provider() {
        let mut config = Config::default();
        // Simulate a stale overlay that still has the old cloud default.
        config.agents.defaults.model = "deepseek/deepseek-chat".into();
        let r = apply_local_llm_bridge(&mut config, "workspace:.clawft/config.json");
        assert_eq!(config.agents.defaults.model, DEFAULT_LOCAL_LLM_MODEL_ROUTED);
        assert_eq!(
            config.providers.local.api_base.as_deref(),
            Some(DEFAULT_LOCAL_LLM_API_BASE)
        );
        assert_eq!(r.agents_model_source, "workspace:.clawft/config.json");
    }

    #[test]
    fn bridge_preserves_explicit_non_default_agents_model() {
        let mut config = Config::default();
        config.agents.defaults.model = "anthropic/claude-sonnet-4-5-20250514".into();
        apply_local_llm_bridge(&mut config, "global:weave.toml");
        assert_eq!(
            config.agents.defaults.model,
            "anthropic/claude-sonnet-4-5-20250514"
        );
        // local api_base still stamped for operators who later switch.
        assert_eq!(
            config.providers.local.api_base.as_deref(),
            Some(DEFAULT_LOCAL_LLM_API_BASE)
        );
    }

    #[test]
    fn bridge_respects_existing_local_api_base() {
        let mut config = Config::default();
        config.providers.local.api_base = Some("http://192.168.1.5:8080/v1".into());
        apply_local_llm_bridge(&mut config, "default");
        assert_eq!(
            config.providers.local.api_base.as_deref(),
            Some("http://192.168.1.5:8080/v1")
        );
    }

    #[test]
    #[cfg(feature = "native")]
    fn service_llm_defaults_match_types_constants() {
        // WEFT-604: clawft-service-llm keeps its own string constants (no
        // clawft-types dep); this lockstep test is the CI guard.
        assert_eq!(
            clawft_service_llm::DEFAULT_LLM_SERVICE_URL,
            DEFAULT_LOCAL_LLM_SERVICE_URL
        );
        assert_eq!(
            clawft_service_llm::DEFAULT_LLM_MODEL,
            DEFAULT_LOCAL_LLM_MODEL
        );
        assert_eq!(
            clawft_llm::local_provider::HERMES_SERVING_BASE_URL,
            DEFAULT_LOCAL_LLM_API_BASE
        );
        assert_eq!(
            clawft_llm::local_provider::HERMES_DEFAULT_MODEL,
            DEFAULT_LOCAL_LLM_MODEL
        );
    }
}
