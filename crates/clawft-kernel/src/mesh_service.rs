//! Cross-node service registry query protocol (K6.3).
//!
//! Defines request/response types for querying a remote node's
//! ServiceRegistry. Uses the ServiceApi pattern per K5 Symposium D13.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[cfg(feature = "exochain")]
use std::sync::Arc;

/// Request to resolve a service on a remote node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceResolveRequest {
    /// Name of the service to resolve.
    pub service_name: String,
    /// Requesting node ID.
    pub requesting_node: String,
}

/// Response to a service resolution request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceResolveResponse {
    /// Whether the service was found.
    pub found: bool,
    /// Service endpoint info (if found).
    pub endpoint: Option<RemoteServiceEndpoint>,
    /// Error message (if not found).
    pub error: Option<String>,
}

/// Information about a remotely available service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteServiceEndpoint {
    /// Service name.
    pub name: String,
    /// Node hosting the service.
    pub node_id: String,
    /// PID of the process hosting this service.
    pub pid: u64,
    /// Available methods.
    pub methods: Vec<String>,
    /// Service version.
    pub version: String,
    /// Service contract hash (if chain-anchored).
    pub contract_hash: Option<String>,
    /// Additional metadata.
    pub metadata: HashMap<String, String>,
}

/// Cached resolution of a remote service.
#[derive(Debug, Clone)]
pub struct ResolvedService {
    /// Resolved endpoint.
    pub endpoint: RemoteServiceEndpoint,
    /// When this resolution was cached.
    pub resolved_at: std::time::Instant,
    /// Cache TTL.
    pub ttl: std::time::Duration,
}

impl ResolvedService {
    /// Check if this cached resolution has expired.
    pub fn is_expired(&self) -> bool {
        self.resolved_at.elapsed() > self.ttl
    }
}

/// Service resolution cache for remote services.
pub struct ServiceResolutionCache {
    /// Cached resolutions keyed by service name.
    cache: HashMap<String, ResolvedService>,
    /// Negative cache for known-missing services.
    negative_cache: HashMap<String, std::time::Instant>,
    /// TTL for negative cache entries.
    negative_ttl: std::time::Duration,
    /// Optional chain manager for exochain audit logging.
    #[cfg(feature = "exochain")]
    chain_manager: Option<Arc<crate::chain::ChainManager>>,
}

impl ServiceResolutionCache {
    /// Create a new empty cache with a 30-second negative TTL.
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            negative_cache: HashMap::new(),
            negative_ttl: std::time::Duration::from_secs(30),
            #[cfg(feature = "exochain")]
            chain_manager: None,
        }
    }

    /// Attach a chain manager for exochain audit logging.
    #[cfg(feature = "exochain")]
    pub fn set_chain_manager(&mut self, cm: Arc<crate::chain::ChainManager>) {
        self.chain_manager = Some(cm);
    }

    /// Look up a cached service resolution (returns `None` if expired).
    pub fn get(&self, service_name: &str) -> Option<&ResolvedService> {
        self.cache.get(service_name).filter(|r| !r.is_expired())
    }

    /// Check if a service is known-missing (negative cached).
    pub fn is_known_missing(&self, service_name: &str) -> bool {
        self.negative_cache
            .get(service_name)
            .is_some_and(|t| t.elapsed() < self.negative_ttl)
    }

    /// Cache a positive resolution.
    pub fn insert(&mut self, service_name: String, resolved: ResolvedService) {
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "mesh_service",
                crate::chain::EVENT_KIND_MESH_SERVICE_REGISTER,
                Some(serde_json::json!({
                    "service_name": &service_name,
                    "node_id": &resolved.endpoint.node_id,
                    "version": &resolved.endpoint.version,
                })),
            );
        }
        self.negative_cache.remove(&service_name);
        self.cache.insert(service_name, resolved);
    }

    /// Cache a negative (not found) result.
    pub fn insert_negative(&mut self, service_name: String) {
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "mesh_service",
                crate::chain::EVENT_KIND_MESH_SERVICE_DEREGISTER,
                Some(serde_json::json!({
                    "service_name": &service_name,
                    "action": "negative_cache",
                })),
            );
        }
        self.negative_cache
            .insert(service_name, std::time::Instant::now());
    }

    /// Evict all expired entries from both positive and negative caches.
    pub fn evict_expired(&mut self) {
        self.cache.retain(|_, r| !r.is_expired());
        self.negative_cache
            .retain(|_, t| t.elapsed() < self.negative_ttl);
    }

    /// Number of cached positive resolutions (including expired).
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Whether the positive cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

impl Default for ServiceResolutionCache {
    fn default() -> Self {
        Self::new()
    }
}

/// RegistryQueryService exposes local service resolution via ServiceApi pattern.
/// Registered as "registry" service, queryable by remote nodes.
pub struct RegistryQueryService {
    /// Methods exposed by this service.
    methods: Vec<String>,
}

impl RegistryQueryService {
    /// Create a new registry query service with default methods.
    pub fn new() -> Self {
        Self {
            methods: vec!["resolve".into(), "list".into(), "health".into()],
        }
    }

    /// Handle a service query request, dispatching by method name.
    pub fn handle_query(&self, method: &str, params: &serde_json::Value) -> serde_json::Value {
        match method {
            "resolve" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                serde_json::json!({
                    "method": "resolve",
                    "name": name,
                    "status": "query_dispatched"
                })
            }
            "list" => {
                serde_json::json!({
                    "method": "list",
                    "status": "query_dispatched"
                })
            }
            "health" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                serde_json::json!({
                    "method": "health",
                    "name": name,
                    "status": "query_dispatched"
                })
            }
            _ => serde_json::json!({"error": format!("unknown method: {method}")}),
        }
    }

    /// Get the list of exposed methods.
    pub fn methods(&self) -> &[String] {
        &self.methods
    }
}

impl Default for RegistryQueryService {
    fn default() -> Self {
        Self::new()
    }
}

/// Circuit breaker state for a remote node or service.
///
/// Implements the standard closed/open/half-open pattern to prevent
/// cascading failures when a remote node becomes unreachable.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum CircuitState {
    /// Normal operation -- requests flow through.
    Closed { error_count: u32, threshold: u32 },
    /// Too many failures -- requests blocked for cooldown.
    Open {
        opened_at: std::time::Instant,
        cooldown: std::time::Duration,
        threshold: u32,
    },
    /// Testing if service has recovered (one probe allowed).
    HalfOpen { threshold: u32 },
}

impl CircuitState {
    /// Create a new circuit breaker in the closed state.
    pub fn new(threshold: u32) -> Self {
        Self::Closed {
            error_count: 0,
            threshold,
        }
    }

    /// Record a successful call. Resets to closed state.
    pub fn record_success(&mut self) {
        let t = self.threshold();
        *self = Self::Closed {
            error_count: 0,
            threshold: t,
        };
    }

    /// Record a failed call. Returns `true` if the circuit just opened.
    pub fn record_failure(&mut self) -> bool {
        match self {
            Self::Closed {
                error_count,
                threshold,
            } => {
                *error_count += 1;
                if *error_count >= *threshold {
                    let t = *threshold;
                    *self = Self::Open {
                        opened_at: std::time::Instant::now(),
                        cooldown: std::time::Duration::from_secs(30),
                        threshold: t,
                    };
                    return true;
                }
                false
            }
            Self::HalfOpen { threshold } => {
                let t = *threshold;
                *self = Self::Open {
                    opened_at: std::time::Instant::now(),
                    cooldown: std::time::Duration::from_secs(30),
                    threshold: t,
                };
                true
            }
            Self::Open { .. } => false,
        }
    }

    /// Check if requests should be allowed through.
    ///
    /// - **Closed**: always allowed.
    /// - **Open**: allowed only after cooldown expires (transitions to half-open).
    /// - **HalfOpen**: allowed (probe request).
    pub fn is_allowed(&mut self) -> bool {
        match self {
            Self::Closed { .. } => true,
            Self::Open {
                opened_at,
                cooldown,
                threshold,
            } => {
                if opened_at.elapsed() > *cooldown {
                    let t = *threshold;
                    *self = Self::HalfOpen { threshold: t };
                    true
                } else {
                    false
                }
            }
            Self::HalfOpen { .. } => true,
        }
    }

    /// Get the failure threshold.
    fn threshold(&self) -> u32 {
        match self {
            Self::Closed { threshold, .. } => *threshold,
            Self::Open { threshold, .. } => *threshold,
            Self::HalfOpen { threshold } => *threshold,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn make_endpoint(name: &str) -> RemoteServiceEndpoint {
        RemoteServiceEndpoint {
            name: name.to_string(),
            node_id: "node-1".to_string(),
            pid: 42,
            methods: vec!["ping".to_string()],
            version: "1.0.0".to_string(),
            contract_hash: None,
            metadata: HashMap::new(),
        }
    }

    fn make_resolved(name: &str, ttl_secs: u64) -> ResolvedService {
        ResolvedService {
            endpoint: make_endpoint(name),
            resolved_at: Instant::now(),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    #[test]
    fn cache_insert_and_get() {
        let mut cache = ServiceResolutionCache::new();
        cache.insert("auth".to_string(), make_resolved("auth", 60));
        assert!(cache.get("auth").is_some());
        assert_eq!(cache.get("auth").unwrap().endpoint.name, "auth");
    }

    #[test]
    fn cache_expired_returns_none() {
        let mut cache = ServiceResolutionCache::new();
        let mut resolved = make_resolved("old", 0);
        resolved.resolved_at = Instant::now() - Duration::from_secs(1);
        cache.insert("old".to_string(), resolved);
        assert!(cache.get("old").is_none());
    }

    #[test]
    fn cache_missing_key_returns_none() {
        let cache = ServiceResolutionCache::new();
        assert!(cache.get("nope").is_none());
    }

    #[test]
    fn negative_cache() {
        let mut cache = ServiceResolutionCache::new();
        assert!(!cache.is_known_missing("gone"));
        cache.insert_negative("gone".to_string());
        assert!(cache.is_known_missing("gone"));
    }

    #[test]
    fn positive_insert_clears_negative() {
        let mut cache = ServiceResolutionCache::new();
        cache.insert_negative("svc".to_string());
        assert!(cache.is_known_missing("svc"));
        cache.insert("svc".to_string(), make_resolved("svc", 60));
        assert!(!cache.is_known_missing("svc"));
        assert!(cache.get("svc").is_some());
    }

    #[test]
    fn evict_expired_removes_stale() {
        let mut cache = ServiceResolutionCache::new();
        let mut stale = make_resolved("stale", 0);
        stale.resolved_at = Instant::now() - Duration::from_secs(1);
        cache.insert("stale".to_string(), stale);
        cache.insert("fresh".to_string(), make_resolved("fresh", 300));
        cache.evict_expired();
        assert_eq!(cache.len(), 1);
        assert!(cache.get("fresh").is_some());
    }

    #[test]
    fn resolve_request_serde_roundtrip() {
        let req = ServiceResolveRequest {
            service_name: "auth".to_string(),
            requesting_node: "node-5".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let restored: ServiceResolveRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.service_name, "auth");
        assert_eq!(restored.requesting_node, "node-5");
    }

    #[test]
    fn resolve_response_found_serde_roundtrip() {
        let resp = ServiceResolveResponse {
            found: true,
            endpoint: Some(make_endpoint("auth")),
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let restored: ServiceResolveResponse = serde_json::from_str(&json).unwrap();
        assert!(restored.found);
        assert_eq!(restored.endpoint.unwrap().name, "auth");
    }

    #[test]
    fn resolve_response_not_found_serde_roundtrip() {
        let resp = ServiceResolveResponse {
            found: false,
            endpoint: None,
            error: Some("service not registered".to_string()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let restored: ServiceResolveResponse = serde_json::from_str(&json).unwrap();
        assert!(!restored.found);
        assert!(restored.endpoint.is_none());
        assert_eq!(restored.error.unwrap(), "service not registered");
    }

    #[test]
    fn resolved_service_expiry() {
        let fresh = make_resolved("x", 60);
        assert!(!fresh.is_expired());

        let mut stale = make_resolved("y", 0);
        stale.resolved_at = Instant::now() - Duration::from_secs(1);
        assert!(stale.is_expired());
    }

    #[test]
    fn empty_cache() {
        let cache = ServiceResolutionCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    // ── RegistryQueryService tests ──────────────────────────────────

    #[test]
    fn registry_query_service_methods() {
        let svc = RegistryQueryService::new();
        let methods = svc.methods();
        assert_eq!(methods.len(), 3);
        assert!(methods.contains(&"resolve".to_string()));
        assert!(methods.contains(&"list".to_string()));
        assert!(methods.contains(&"health".to_string()));
    }

    #[test]
    fn registry_query_resolve() {
        let svc = RegistryQueryService::new();
        let params = serde_json::json!({"name": "auth"});
        let result = svc.handle_query("resolve", &params);
        assert_eq!(result["method"], "resolve");
        assert_eq!(result["name"], "auth");
        assert_eq!(result["status"], "query_dispatched");
    }

    #[test]
    fn registry_query_list() {
        let svc = RegistryQueryService::new();
        let result = svc.handle_query("list", &serde_json::json!({}));
        assert_eq!(result["method"], "list");
        assert_eq!(result["status"], "query_dispatched");
    }

    #[test]
    fn registry_query_health() {
        let svc = RegistryQueryService::new();
        let params = serde_json::json!({"name": "db"});
        let result = svc.handle_query("health", &params);
        assert_eq!(result["method"], "health");
        assert_eq!(result["name"], "db");
    }

    #[test]
    fn registry_query_unknown_method() {
        let svc = RegistryQueryService::new();
        let result = svc.handle_query("unknown", &serde_json::json!({}));
        assert!(result.get("error").is_some());
    }

    #[test]
    fn registry_query_default() {
        let svc = RegistryQueryService::default();
        assert_eq!(svc.methods().len(), 3);
    }

    // ── CircuitState tests ──────────────────────────────────────────

    #[test]
    fn circuit_starts_closed() {
        let cb = CircuitState::new(3);
        assert!(matches!(
            cb,
            CircuitState::Closed {
                error_count: 0,
                threshold: 3
            }
        ));
    }

    #[test]
    fn circuit_stays_closed_below_threshold() {
        let mut cb = CircuitState::new(3);
        assert!(!cb.record_failure()); // 1
        assert!(!cb.record_failure()); // 2
        assert!(cb.is_allowed());
    }

    #[test]
    fn circuit_opens_at_threshold() {
        let mut cb = CircuitState::new(3);
        assert!(!cb.record_failure()); // 1
        assert!(!cb.record_failure()); // 2
        assert!(cb.record_failure()); // 3 -> opens
        assert!(matches!(cb, CircuitState::Open { .. }));
        assert!(!cb.is_allowed()); // cooldown not elapsed
    }

    #[test]
    fn circuit_success_resets_to_closed() {
        let mut cb = CircuitState::new(3);
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        assert!(matches!(cb, CircuitState::Closed { error_count: 0, .. }));
        assert!(cb.is_allowed());
    }

    #[test]
    fn circuit_open_transitions_to_half_open_after_cooldown() {
        let mut cb = CircuitState::Open {
            opened_at: Instant::now() - Duration::from_secs(60),
            cooldown: Duration::from_secs(30),
            threshold: 3,
        };
        // Cooldown has elapsed, should transition to half-open
        assert!(cb.is_allowed());
        assert!(matches!(cb, CircuitState::HalfOpen { .. }));
    }

    #[test]
    fn circuit_half_open_success_closes() {
        let mut cb = CircuitState::HalfOpen { threshold: 3 };
        assert!(cb.is_allowed());
        cb.record_success();
        assert!(matches!(cb, CircuitState::Closed { error_count: 0, .. }));
    }

    #[test]
    fn circuit_half_open_failure_reopens() {
        let mut cb = CircuitState::HalfOpen { threshold: 3 };
        assert!(cb.record_failure()); // reopens
        assert!(matches!(cb, CircuitState::Open { .. }));
        assert!(!cb.is_allowed()); // fresh cooldown
    }
}
