//! Placement-aware base-URL resolution for [`ProviderRouter`](crate::ProviderRouter)
//! (card mesh-placement-19; ADR-101 section 5).
//!
//! A [`PlacementResolver`] answers "where is the inference role `R` served
//! right now?" with a base URL. The router consults it per request through
//! [`PlacedProvider`], behind a TTL cache that is invalidated on failure
//! or by the caller (for example on a mesh peer event).
//!
//! Precedence is decided by the caller when it builds the router: a
//! provider whose base URL was set by env or `[kernel.llm]` is simply not
//! given a role, so it is never placed. The configured `base_url` stays the
//! fallback whenever the resolver has no answer.
//!
//! The resolver is untrusted for destination: a resolved URL is used only
//! if it is plain `http` to a loopback host. A placed role on another node
//! is reached through the node's loopback proxy, so the resolver never
//! makes this crate send a request (or an API key) to an arbitrary host.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::config::LlmProviderConfig;
use crate::error::{ProviderError, Result};
use crate::openai_compat::OpenAiCompatProvider;
use crate::provider::Provider;
use crate::types::{ChatRequest, ChatResponse, StreamChunk};

/// Default lifetime of a cached resolution.
pub const DEFAULT_TTL: Duration = Duration::from_secs(5);

/// Resolves an inference role to the base URL (`http://127.0.0.1:PORT/v1`)
/// that currently serves it. Cheap and synchronous: implementations answer
/// from a snapshot they keep fresh themselves.
pub trait PlacementResolver: Send + Sync {
    /// Base URL for `role`, or `None` when placement has no answer (layer
    /// disabled, role unknown, nothing healthy). `None` means fall back.
    fn resolve_base_url(&self, role: &str) -> Option<String>;
}

/// TTL cache in front of a resolver. Negative answers are cached too, so a
/// dead placement layer is not asked on every request.
pub struct CachedResolver {
    inner: Arc<dyn PlacementResolver>,
    ttl: Duration,
    cache: Mutex<HashMap<String, (Instant, Option<String>)>>,
}

impl CachedResolver {
    /// Wrap `inner` with a cache of `ttl`.
    pub fn new(inner: Arc<dyn PlacementResolver>, ttl: Duration) -> Self {
        Self {
            inner,
            ttl,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Forget the cached answer for `role` (the instance moved or failed).
    pub fn invalidate(&self, role: &str) {
        if let Ok(mut c) = self.cache.lock() {
            c.remove(role);
        }
    }

    /// Forget every cached answer (a peer or advertisement changed).
    pub fn invalidate_all(&self) {
        if let Ok(mut c) = self.cache.lock() {
            c.clear();
        }
    }
}

impl PlacementResolver for CachedResolver {
    fn resolve_base_url(&self, role: &str) -> Option<String> {
        if let Ok(c) = self.cache.lock()
            && let Some((at, ans)) = c.get(role)
            && at.elapsed() < self.ttl
        {
            return ans.clone();
        }
        let ans = self.inner.resolve_base_url(role);
        if let Ok(mut c) = self.cache.lock() {
            c.insert(role.to_string(), (Instant::now(), ans.clone()));
        }
        ans
    }
}

/// True when `url` is `http://<loopback host>[:port][/path]`.
pub fn is_loopback_http(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    let host = if let Some(h) = authority.strip_prefix('[') {
        h.split(']').next().unwrap_or("")
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// A provider whose endpoint follows placement, falling back to its
/// configured `base_url`.
pub struct PlacedProvider {
    role: String,
    config: LlmProviderConfig,
    fallback: OpenAiCompatProvider,
    resolver: Arc<CachedResolver>,
    placed: Mutex<HashMap<String, Arc<OpenAiCompatProvider>>>,
}

impl PlacedProvider {
    /// Provider for `config` that resolves `role` through `resolver`.
    pub fn new(role: String, config: LlmProviderConfig, resolver: Arc<CachedResolver>) -> Self {
        Self {
            role,
            fallback: OpenAiCompatProvider::new(config.clone()),
            config,
            resolver,
            placed: Mutex::new(HashMap::new()),
        }
    }

    /// The role this provider resolves.
    pub fn role(&self) -> &str {
        &self.role
    }

    /// The provider to use right now and the placed URL it was built for
    /// (`None`: the static fallback).
    fn current(&self) -> (Option<String>, Option<Arc<OpenAiCompatProvider>>) {
        let Some(url) = self
            .resolver
            .resolve_base_url(&self.role)
            .filter(|u| is_loopback_http(u))
        else {
            return (None, None);
        };
        let Ok(mut map) = self.placed.lock() else {
            return (None, None);
        };
        let p = map
            .entry(url.clone())
            .or_insert_with(|| {
                let mut cfg = self.config.clone();
                cfg.base_url = url.clone();
                Arc::new(OpenAiCompatProvider::new(cfg))
            })
            .clone();
        (Some(url), Some(p))
    }

    /// Failures that say the placed endpoint is gone or broken: connect and
    /// timeout errors, and 502, 503 or 504 (which includes the loopback
    /// proxy reporting that no instance serves the role). A 4xx, a quota
    /// 429, an auth failure or a malformed answer is the server's real
    /// answer and is never replayed elsewhere.
    fn should_retry(err: &ProviderError) -> bool {
        match err {
            ProviderError::Timeout => true,
            ProviderError::ServerError { status, .. } => matches!(status, 502..=504),
            ProviderError::Http(e) => e.is_connect() || e.is_timeout(),
            _ => false,
        }
    }
}

#[async_trait]
impl Provider for PlacedProvider {
    fn name(&self) -> &str {
        &self.config.name
    }

    async fn complete(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let (url, placed) = self.current();
        let Some(placed) = placed else {
            return self.fallback.complete(request).await;
        };
        match placed.complete(request).await {
            Err(e) if Self::should_retry(&e) => {
                // The instance may have moved: forget the answer and use
                // the configured endpoint for this request.
                self.resolver.invalidate(&self.role);
                if url.as_deref() == Some(self.config.base_url.trim_end_matches('/')) {
                    return Err(e);
                }
                self.fallback.complete(request).await
            }
            other => other,
        }
    }

    async fn complete_stream(
        &self,
        request: &ChatRequest,
        tx: mpsc::Sender<StreamChunk>,
    ) -> Result<()> {
        let (_, placed) = self.current();
        match placed {
            Some(p) => {
                let r = p.complete_stream(request, tx).await;
                if matches!(&r, Err(e) if Self::should_retry(e)) {
                    self.resolver.invalidate(&self.role);
                }
                r
            }
            None => self.fallback.complete_stream(request, tx).await,
        }
    }
}

impl std::fmt::Debug for PlacedProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlacedProvider")
            .field("role", &self.role)
            .field("fallback_base_url", &self.config.base_url)
            .finish()
    }
}

#[cfg(test)]
#[path = "placement_tests.rs"]
mod tests;
