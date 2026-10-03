//! Process-wide placement hook for the `local` provider (card
//! mesh-placement-19; ADR-101 section 5).
//!
//! The daemon installs a [`PlacementResolver`] once, with the provider names
//! that may follow placement and the role each maps to. The adapter factory
//! (`pipeline::llm_adapter`) then wraps those providers in a
//! [`PlacedProvider`](clawft_llm::PlacedProvider), but only when nothing
//! explicit chose the endpoint ([`local_placement_allowed`]): env, then
//! `[kernel.llm]` and `[providers.local]`, then placement, then the
//! ADR-060 constants. Nothing is installed by default, so nothing changes
//! unless the operator turned placement on.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use clawft_llm::{CachedResolver, PlacementResolver};
use clawft_types::config::Config;

use crate::local_llm_bridge::{local_placement_allowed, resolve_local_llm};

struct Hook {
    cache: Arc<CachedResolver>,
    roles: HashMap<String, String>,
}

static HOOK: OnceLock<Hook> = OnceLock::new();

/// A role to base-URL lookup.
pub type ResolveFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

struct FnResolver(ResolveFn);

impl PlacementResolver for FnResolver {
    fn resolve_base_url(&self, role: &str) -> Option<String> {
        (self.0)(role)
    }
}

/// Install the hook (first call wins; returns whether this call did).
/// `resolve` answers a role with the base URL that serves it now (or
/// `None`: fall back); `roles` maps a provider name (`local`) to the role
/// it follows.
pub fn install(
    resolve: ResolveFn,
    ttl: Duration,
    roles: HashMap<String, String>,
) -> bool {
    install_resolver(Arc::new(FnResolver(resolve)), ttl, roles)
}

/// [`install`] for a [`PlacementResolver`] object.
pub fn install_resolver(
    resolver: Arc<dyn PlacementResolver>,
    ttl: Duration,
    roles: HashMap<String, String>,
) -> bool {
    HOOK.set(Hook {
        cache: Arc::new(CachedResolver::new(resolver, ttl)),
        roles,
    })
    .is_ok()
}

/// Drop every cached placement answer (a peer or advert changed).
pub fn invalidate_all() {
    if let Some(h) = HOOK.get() {
        h.cache.invalidate_all();
    }
}

/// The role and cache `provider` should follow under `config`, or `None`
/// when no hook is installed, the provider has no role, or an explicit
/// setting chose its endpoint.
pub fn placement_for(provider: &str, config: &Config) -> Option<(String, Arc<CachedResolver>)> {
    let h = HOOK.get()?;
    let role = h.roles.get(provider)?;
    // Only `local` has an endpoint precedence rule today.
    if provider != "local" {
        return None;
    }
    let resolution = resolve_local_llm(config, "placement");
    local_placement_allowed(config, &resolution).then(|| (role.clone(), h.cache.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed() -> ResolveFn {
        Arc::new(|_| Some("http://127.0.0.1:1/v1".into()))
    }

    #[test]
    fn no_hook_no_placement_then_explicit_settings_win() {
        let config = Config::default();
        // Single test body: the hook is process-wide and first install wins.
        assert!(placement_for("local", &config).is_none(), "nothing installed");
        let mut roles = HashMap::new();
        roles.insert("local".to_string(), "hermes".to_string());
        roles.insert("ollama".to_string(), "other".to_string());
        assert!(install(fixed(), Duration::from_secs(5), roles));
        assert!(!install(fixed(), Duration::from_secs(5), HashMap::new()), "first wins");
        assert!(placement_for("openai", &config).is_none(), "no role for the provider");
        assert!(placement_for("ollama", &config).is_none(), "only local has a precedence rule");
        if resolve_local_llm(&config, "t").url_source == "default:local-adr060" {
            assert_eq!(placement_for("local", &config).unwrap().0, "hermes");
        }
        let mut explicit = Config::default();
        explicit.providers.local.api_base = Some("http://127.0.0.1:9100/v1".into());
        assert!(placement_for("local", &explicit).is_none(), "explicit api_base wins");
    }
}
