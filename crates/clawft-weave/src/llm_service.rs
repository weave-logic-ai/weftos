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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use clawft_kernel::{HealthStatus, ServiceType, SystemService};
use clawft_service_llm::LlmClient;

/// Stable name. Matches `(ControlKind::Service, "llm")` so a
/// `service.{start,stop,restart}` call and a
/// `control.set_enabled {kind:"service", target:"llm"}` call share
/// the same identifier surface.
const LLM_SERVICE_NAME: &str = "llm";

/// Methods this service contract exposes. Single entry today —
/// `llm.prompt` is the only public RPC. When `agent.chat` graduates
/// off `daemon_llm()` and onto a routed dispatch, this list grows.
pub const LLM_CONTRACT_METHODS: &[&str] = &["llm.prompt"];

/// SystemService adapter wrapping the daemon's [`LlmClient`].
///
/// Constructed at boot in `daemon::run` once the client has been
/// successfully built; registered into `k.services()` so it appears
/// in the Services panel. The same `Arc<LlmClient>` is still cached
/// in `DAEMON_LLM` for the existing `daemon_llm()` lookup path —
/// this adapter owns its own clone so the registry can drop and
/// recreate the service without disturbing in-flight RPC calls.
pub struct LlmSystemService {
    client: Arc<LlmClient>,
    /// Mirror of the control-flag the daemon registers at boot:
    /// `control_flags.register(ControlKind::Service, "llm", true)`.
    /// Cloned here so `start` / `stop` / `health_check` can read and
    /// flip it without taking the daemon-control RwLock on every
    /// probe.
    enabled: Arc<AtomicBool>,
}

impl LlmSystemService {
    /// Build a new adapter from the LLM client and the existing
    /// service-control flag. Daemon's responsibility to pass the
    /// same `Arc<AtomicBool>` it registered with `ControlFlags`, so
    /// `service.stop` and `control.set_enabled` are observably the
    /// same toggle.
    pub fn new(client: Arc<LlmClient>, enabled: Arc<AtomicBool>) -> Self {
        Self { client, enabled }
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
        tracing::info!(
            url = %self.client.config().base_url,
            model = %self.client.config().model,
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
        match self.client.health().await {
            Ok(true) => HealthStatus::Healthy,
            Ok(false) => HealthStatus::Degraded("upstream loading model (503)".into()),
            Err(e) => HealthStatus::Unhealthy(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_service_llm::LlmConfig;

    fn unreachable_client() -> Arc<LlmClient> {
        // Bind to a never-served port so health() returns a transport
        // error deterministically. We don't actually fire HTTP in the
        // synchronous-construction tests below — only in the async
        // `health_check_*` tests that exercise the unhealthy path.
        let cfg = LlmConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            ..LlmConfig::default()
        };
        Arc::new(LlmClient::new(cfg).expect("client builds with bogus url"))
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
}
