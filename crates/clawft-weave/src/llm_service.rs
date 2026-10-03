//! `LlmSystemService` — adapter that registers the daemon's
//! [`clawft_service_llm::LlmClient`] with the kernel's
//! [`clawft_kernel::ServiceRegistry`] so the local LLM appears in
//! `kernel.services` and gets the same lifecycle / governance /
//! permissions treatment as cron, containers, hnsw, etc.
//!
//! Why this exists
//! ===============
//!
//! Before this module the LLM client was a private `OnceLock` handle
//! the `agent.chat` and `llm.prompt` paths reached through `daemon_llm()`.
//! It worked, but it never showed up in the Services panel, never got
//! a chain-anchored contract, and `service.start` / `service.stop` /
//! `service.restart` couldn't address it. The user reasonably wanted
//! the local LLM treated as a first-class service.
//!
//! What this gives you
//! ===================
//!
//! - **Visibility**: a row in `kernel.services` (name = `"llm"`,
//!   `ServiceType::Custom("llm")`) so the desktop Services panel and
//!   `weft service list` both see it.
//! - **Lifecycle**: `service.start` flips the existing
//!   `ControlKind::Service / "llm"` flag on, `service.stop` flips it
//!   off. The flag is the same one [`daemon::handle_llm_prompt`]
//!   already consults at the top of every `llm.prompt`, so the
//!   semantics match: "stopped" means "next prompt returns
//!   'service disabled'."
//! - **Health**: `health_check()` short-circuits to `Degraded("disabled")`
//!   when the flag is off, otherwise calls
//!   [`LlmClient::health`] (a `/health` GET against the upstream
//!   llama-server). 200 → `Healthy`, 503 → `Degraded("loading model")`,
//!   transport / 5xx → `Unhealthy(reason)`.
//! - **Governance**: registration goes through
//!   [`ServiceRegistry::register_with_contract`] when the kernel has
//!   exochain wired, which appends a `service.contract.register`
//!   chain event listing the methods this service exposes
//!   (`llm.prompt`). The chain audit then carries the LLM alongside
//!   every other governed kernel service.
//! - **Permissions**: the control flag is the same one the
//!   `control.set_enabled { kind: "service", target: "llm" }` RPC
//!   already toggles, so the existing capability gate keeps governing
//!   who can flip it.
//! - **WEFT-343 runtime swap**: the adapter holds a
//!   [`SharedLlmClient`] so a mid-session env rotation (e.g.
//!   `OPENROUTER_API_KEY`) re-read via `control.set_enabled` is
//!   visible to `health_check` / start logs without re-registering.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use clawft_kernel::{HealthStatus, ServiceType, SystemService};
use clawft_service_llm::{
    LlmClient, LlmConfig, SharedLlmClient, DEFAULT_LLM_MODEL, DEFAULT_LLM_SERVICE_URL,
    DEFAULT_OPENROUTER_BASE_URL, DEFAULT_OPENROUTER_MODEL, LLM_MODEL_ENV, LLM_SERVICE_URL_ENV,
    OPENROUTER_API_KEY_ENV,
};

/// Stable name. Matches `(ControlKind::Service, "llm")` so a
/// `service.{start,stop,restart}` call and a
/// `control.set_enabled {kind:"service", target:"llm"}` call share
/// the same identifier surface.
const LLM_SERVICE_NAME: &str = "llm";

/// Methods this service contract exposes.
///
/// - `llm.prompt` — single-shot completion (no tools)
/// - `llm.models` — WEFT-256 enumeration for the chat panel chip strip
///
/// `agent.chat` / `agent.chat_stream` remain separate Chat-class verbs
/// (not on this service contract) until the agent path is fully
/// mediated through the registry.
pub const LLM_CONTRACT_METHODS: &[&str] = &["llm.prompt", "llm.models"];

/// Outcome of [`resolve_llm_endpoint`]: the config to hand
/// [`LlmClient::new`] plus whether the resolved target is OpenRouter
/// (affects health-probe skip + attribution headers).
#[derive(Debug, Clone)]
pub struct ResolvedLlmEndpoint {
    /// Fully-resolved client config (URL, model, api_key, headers).
    pub config: LlmConfig,
    /// `true` when OpenRouter takeover fired (no explicit URL + key set).
    pub using_openrouter: bool,
    /// Human-readable provenance for the winning URL source.
    pub url_source: &'static str,
    /// Human-readable provenance for the winning model source.
    pub model_source: &'static str,
}

/// Resolve LLM endpoint from env + optional `[kernel.llm]` config.
///
/// Precedence (first hit wins) — mirrors the daemon boot path so a
/// WEFT-343 refresh after `control.set_enabled` observes the same
/// rules as cold start:
///
/// 1. `LLM_SERVICE_URL` env
/// 2. `[kernel.llm].service_url` in config
/// 3. `OPENROUTER_API_KEY` set AND no URL above → OpenRouter defaults
/// 4. ADR-060 local Hermes defaults
pub fn resolve_llm_endpoint(
    cfg_llm: Option<&clawft_types::config::LlmEndpointConfig>,
) -> ResolvedLlmEndpoint {
    // A project-profile kernel reaches the model through its parent: no URL,
    // no key, no OpenRouter takeover from the environment (ADR-103 Phase 2 F).
    if crate::project_profile::is_project_profile() {
        return crate::project_profile::parent_llm_endpoint(cfg_llm);
    }
    let cfg_llm_url = cfg_llm
        .and_then(|c| c.service_url.clone())
        .filter(|s| !s.is_empty());
    let cfg_llm_model = cfg_llm
        .and_then(|c| c.model.clone())
        .filter(|s| !s.is_empty());

    let api_key_env = std::env::var(OPENROUTER_API_KEY_ENV)
        .ok()
        .filter(|s| !s.is_empty());
    let llm_url_env = std::env::var(LLM_SERVICE_URL_ENV)
        .ok()
        .filter(|s| !s.is_empty());
    let llm_model_env = std::env::var(LLM_MODEL_ENV)
        .ok()
        .filter(|s| !s.is_empty());

    let url_from_env = llm_url_env.is_some();
    let url_from_cfg = cfg_llm_url.is_some();
    let model_from_env = llm_model_env.is_some();
    let model_from_cfg = cfg_llm_model.is_some();

    let llm_url_override = llm_url_env.or(cfg_llm_url);
    let llm_model_override = llm_model_env.or(cfg_llm_model);

    let openrouter_takeover = api_key_env.is_some() && llm_url_override.is_none();

    let (default_url, default_model) = if openrouter_takeover {
        (
            DEFAULT_OPENROUTER_BASE_URL.to_string(),
            DEFAULT_OPENROUTER_MODEL.to_string(),
        )
    } else {
        (
            DEFAULT_LLM_SERVICE_URL.to_string(),
            DEFAULT_LLM_MODEL.to_string(),
        )
    };
    // Placement (ADR-101 section 5): with no explicit URL and no OpenRouter
    // takeover, the `local` provider's role may say where the model is now.
    // A role served on this node resolves to its server directly, so the
    // request path has no extra hop; an explicit setting always wins.
    let placed = (llm_url_override.is_none() && !openrouter_takeover)
        .then(|| clawft_types::placement::roles::resolve_for_provider("local"))
        .flatten();
    let using_placement = placed.is_some();
    let llm_url = llm_url_override.unwrap_or_else(|| placed.unwrap_or(default_url));
    let llm_model = llm_model_override.unwrap_or(default_model);
    let using_openrouter = openrouter_takeover;
    let api_key = if using_openrouter { api_key_env } else { None };

    let url_source = if url_from_env {
        "env:LLM_SERVICE_URL"
    } else if url_from_cfg {
        "config:[kernel.llm].service_url"
    } else if using_openrouter {
        "default:openrouter"
    } else if using_placement {
        "placement:local"
    } else {
        "default:local"
    };
    let model_source = if model_from_env {
        "env:LLM_MODEL"
    } else if model_from_cfg {
        "config:[kernel.llm].model"
    } else if using_openrouter {
        "default:openrouter"
    } else {
        "default:local"
    };

    ResolvedLlmEndpoint {
        config: LlmConfig {
            base_url: llm_url,
            model: llm_model,
            api_key,
            referer: using_openrouter.then(|| "https://github.com/clawft/clawft".to_string()),
            app_title: using_openrouter.then(|| "WeftOS weaver".to_string()),
            ..LlmConfig::default()
        },
        using_openrouter,
        url_source,
        model_source,
    }
}

/// Build a fresh [`LlmClient`] from the current env + optional kernel
/// config. Used at boot and on WEFT-343 refresh.
pub fn build_llm_client(
    cfg_llm: Option<&clawft_types::config::LlmEndpointConfig>,
) -> Result<(LlmClient, ResolvedLlmEndpoint), clawft_service_llm::LlmError> {
    let resolved = resolve_llm_endpoint(cfg_llm);
    let client = new_client(resolved.config.clone())?;
    Ok((client, resolved))
}

/// [`LlmClient::new`], except in a project-profile kernel, where the client
/// runs on the parent backend and never opens an HTTP connection itself.
pub fn new_client(config: LlmConfig) -> Result<LlmClient, clawft_service_llm::LlmError> {
    match crate::project_profile::parent_llm_backend() {
        Some(backend) => LlmClient::with_backend(config, backend),
        None => LlmClient::new(config),
    }
}

/// SystemService adapter wrapping the daemon's [`SharedLlmClient`].
///
/// Constructed at boot in `daemon::run` once the client has been
/// successfully built; registered into `k.services()` so it appears
/// in the Services panel. The same `SharedLlmClient` is cached in
/// `DAEMON_LLM` for the existing `daemon_llm()` lookup path — this
/// adapter clones the outer `Arc` so a swap replaces the client for
/// every holder.
pub struct LlmSystemService {
    client: SharedLlmClient,
    /// Mirror of the control-flag the daemon registers at boot:
    /// `control_flags.register(ControlKind::Service, "llm", true)`.
    /// Cloned here so `start` / `stop` / `health_check` can read and
    /// flip it without taking the daemon-control RwLock on every
    /// probe.
    enabled: Arc<AtomicBool>,
}

impl LlmSystemService {
    /// Build a new adapter from the shared LLM handle and the existing
    /// service-control flag. Daemon's responsibility to pass the
    /// same `Arc<AtomicBool>` it registered with `ControlFlags`, so
    /// `service.stop` and `control.set_enabled` are observably the
    /// same toggle.
    pub fn new(client: SharedLlmClient, enabled: Arc<AtomicBool>) -> Self {
        Self { client, enabled }
    }

    /// Borrow the shared handle (tests / diagnostics).
    pub fn shared_client(&self) -> &SharedLlmClient {
        &self.client
    }
}

#[async_trait]
impl SystemService for LlmSystemService {
    fn name(&self) -> &str {
        LLM_SERVICE_NAME
    }

    fn service_type(&self) -> ServiceType {
        ServiceType::Custom("llm".into())
    }

    async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.enabled.store(true, Ordering::SeqCst);
        let guard = self.client.read().await;
        tracing::info!(
            url = %guard.config().base_url,
            model = %guard.config().model,
            "llm service enabled (service.start)",
        );
        Ok(())
    }

    async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.enabled.store(false, Ordering::SeqCst);
        tracing::info!("llm service disabled (service.stop)");
        Ok(())
    }

    async fn health_check(&self) -> HealthStatus {
        if !self.enabled.load(Ordering::SeqCst) {
            return HealthStatus::Degraded("disabled".into());
        }
        let guard = self.client.read().await;
        match guard.health().await {
            Ok(true) => HealthStatus::Healthy,
            Ok(false) => HealthStatus::Degraded("upstream loading model (503)".into()),
            Err(e) => HealthStatus::Unhealthy(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_service_llm::{share_llm_client, LlmConfig};

    fn unreachable_client() -> SharedLlmClient {
        // Bind to a never-served port so health() returns a transport
        // error deterministically. We don't actually fire HTTP in the
        // synchronous-construction tests below — only in the async
        // `health_check_*` tests that exercise the unhealthy path.
        let cfg = LlmConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            ..LlmConfig::default()
        };
        share_llm_client(LlmClient::new(cfg).expect("client builds with bogus url"))
    }

    #[test]
    fn name_and_type_are_stable() {
        let svc = LlmSystemService::new(unreachable_client(), Arc::new(AtomicBool::new(true)));
        assert_eq!(svc.name(), "llm");
        assert_eq!(svc.service_type(), ServiceType::Custom("llm".into()));
    }

    #[tokio::test]
    async fn start_flips_flag_on() {
        let flag = Arc::new(AtomicBool::new(false));
        let svc = LlmSystemService::new(unreachable_client(), Arc::clone(&flag));
        svc.start().await.unwrap();
        assert!(flag.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn stop_flips_flag_off() {
        let flag = Arc::new(AtomicBool::new(true));
        let svc = LlmSystemService::new(unreachable_client(), Arc::clone(&flag));
        svc.stop().await.unwrap();
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn health_when_disabled_is_degraded() {
        let flag = Arc::new(AtomicBool::new(false));
        let svc = LlmSystemService::new(unreachable_client(), flag);
        match svc.health_check().await {
            HealthStatus::Degraded(msg) => assert!(msg.contains("disabled")),
            other => panic!("expected Degraded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn health_when_enabled_but_unreachable_is_unhealthy() {
        let flag = Arc::new(AtomicBool::new(true));
        let svc = LlmSystemService::new(unreachable_client(), flag);
        match svc.health_check().await {
            HealthStatus::Unhealthy(_) => {}
            other => panic!("expected Unhealthy, got {other:?}"),
        }
    }

    #[test]
    fn contract_methods_lists_llm_prompt() {
        assert!(LLM_CONTRACT_METHODS.contains(&"llm.prompt"));
    }

    /// WEFT-343: swapping the shared handle is visible to the service
    /// adapter without reconstructing it.
    #[tokio::test]
    async fn shared_client_swap_is_visible_to_service() {
        let shared = unreachable_client();
        let svc = LlmSystemService::new(Arc::clone(&shared), Arc::new(AtomicBool::new(true)));
        assert_eq!(
            svc.shared_client().read().await.config().base_url,
            "http://127.0.0.1:1"
        );

        let replacement = LlmClient::new(LlmConfig {
            base_url: "http://127.0.0.1:2".to_string(),
            model: "swapped-model".to_string(),
            ..LlmConfig::default()
        })
        .expect("client builds");
        *shared.write().await = replacement;

        let guard = svc.shared_client().read().await;
        assert_eq!(guard.config().base_url, "http://127.0.0.1:2");
        assert_eq!(guard.config().model, "swapped-model");
    }

    /// Serialise env-mutating resolve tests — `set_var` races with
    /// other threads under nextest/cargo-test parallelism.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Snapshot / restore the three env vars resolve reads so ambient
    /// developer shell vars (and sibling tests) cannot poison results.
    struct EnvSnapshot {
        url: Option<String>,
        key: Option<String>,
        model: Option<String>,
    }

    impl EnvSnapshot {
        fn capture() -> Self {
            Self {
                url: std::env::var(LLM_SERVICE_URL_ENV).ok(),
                key: std::env::var(OPENROUTER_API_KEY_ENV).ok(),
                model: std::env::var(LLM_MODEL_ENV).ok(),
            }
        }

        fn restore(self) {
            // SAFETY: held under env_lock by the caller.
            unsafe {
                match self.url {
                    Some(v) => std::env::set_var(LLM_SERVICE_URL_ENV, v),
                    None => std::env::remove_var(LLM_SERVICE_URL_ENV),
                }
                match self.key {
                    Some(v) => std::env::set_var(OPENROUTER_API_KEY_ENV, v),
                    None => std::env::remove_var(OPENROUTER_API_KEY_ENV),
                }
                match self.model {
                    Some(v) => std::env::set_var(LLM_MODEL_ENV, v),
                    None => std::env::remove_var(LLM_MODEL_ENV),
                }
            }
        }
    }

    #[test]
    fn resolve_honors_llm_service_url_env() {
        let _guard = env_lock();
        let snap = EnvSnapshot::capture();
        // SAFETY: held under env_lock; set/remove for isolation.
        unsafe {
            std::env::set_var(LLM_SERVICE_URL_ENV, "http://env-host:9000");
            std::env::remove_var(OPENROUTER_API_KEY_ENV);
            std::env::remove_var(LLM_MODEL_ENV);
        }
        let resolved = resolve_llm_endpoint(None);
        snap.restore();
        assert_eq!(resolved.config.base_url, "http://env-host:9000");
        assert_eq!(resolved.url_source, "env:LLM_SERVICE_URL");
        assert!(!resolved.using_openrouter);
    }

    #[test]
    fn resolve_follows_a_placed_local_role_but_never_beats_explicit_settings() {
        use std::collections::HashMap;
        let _guard = env_lock();
        let snap = EnvSnapshot::capture();
        unsafe {
            std::env::remove_var(LLM_SERVICE_URL_ENV);
            std::env::remove_var(OPENROUTER_API_KEY_ENV);
            std::env::remove_var(LLM_MODEL_ENV);
        }
        let mut providers = HashMap::new();
        providers.insert("local".to_string(), "hermes".to_string());
        clawft_types::placement::roles::install(
            std::sync::Arc::new(|r| (r == "hermes").then(|| "http://127.0.0.1:18090/v1".to_string())),
            providers,
        );
        // Default: placement answers.
        let placed = resolve_llm_endpoint(None);
        assert_eq!(placed.config.base_url, "http://127.0.0.1:18090");
        assert_eq!(placed.url_source, "placement:local");
        // [kernel.llm].service_url beats it.
        let cfg = clawft_types::config::LlmEndpointConfig {
            service_url: Some("http://local:8090".into()),
            model: None,
        };
        let r = resolve_llm_endpoint(Some(&cfg));
        assert_eq!(r.config.base_url, "http://local:8090");
        assert_eq!(r.url_source, "config:[kernel.llm].service_url");
        // So does the env var, and the OpenRouter takeover.
        unsafe { std::env::set_var(LLM_SERVICE_URL_ENV, "http://env-host:9000") };
        assert_eq!(resolve_llm_endpoint(None).url_source, "env:LLM_SERVICE_URL");
        unsafe {
            std::env::remove_var(LLM_SERVICE_URL_ENV);
            std::env::set_var(OPENROUTER_API_KEY_ENV, "sk-test-key");
        }
        assert!(resolve_llm_endpoint(None).using_openrouter);
        // The role stops resolving (placement died): the ADR-060 default again.
        unsafe { std::env::remove_var(OPENROUTER_API_KEY_ENV) };
        clawft_types::placement::roles::clear();
        let back = resolve_llm_endpoint(None);
        assert_eq!(back.url_source, "default:local");
        assert_eq!(back.config.base_url, DEFAULT_LLM_SERVICE_URL);
        snap.restore();
    }

    #[test]
    fn resolve_openrouter_takeover_when_key_set_and_no_url() {
        let _guard = env_lock();
        let snap = EnvSnapshot::capture();
        unsafe {
            std::env::remove_var(LLM_SERVICE_URL_ENV);
            std::env::set_var(OPENROUTER_API_KEY_ENV, "sk-test-key");
            std::env::remove_var(LLM_MODEL_ENV);
        }
        let resolved = resolve_llm_endpoint(None);
        snap.restore();
        assert!(resolved.using_openrouter);
        assert_eq!(resolved.config.base_url, DEFAULT_OPENROUTER_BASE_URL);
        assert_eq!(resolved.config.api_key.as_deref(), Some("sk-test-key"));
    }

    #[test]
    fn resolve_config_url_beats_openrouter_key() {
        let _guard = env_lock();
        let snap = EnvSnapshot::capture();
        unsafe {
            std::env::remove_var(LLM_SERVICE_URL_ENV);
            std::env::set_var(OPENROUTER_API_KEY_ENV, "sk-test-key");
            std::env::remove_var(LLM_MODEL_ENV);
        }
        let cfg = clawft_types::config::LlmEndpointConfig {
            service_url: Some("http://local:8090".into()),
            model: Some("hermes".into()),
        };
        let resolved = resolve_llm_endpoint(Some(&cfg));
        snap.restore();
        assert!(!resolved.using_openrouter);
        assert_eq!(resolved.config.base_url, "http://local:8090");
        assert!(resolved.config.api_key.is_none());
    }
}
