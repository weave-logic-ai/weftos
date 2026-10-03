//! Authentication middleware and bearer-token validation.
//!
//! The kernel daemon is the single token authority (ADR-102 D3/D5). The
//! gateway holds no token store of its own: every bearer is checked through
//! the daemon's `auth.token.validate` RPC, with a short positive-result cache
//! so a burst of requests does not become a burst of daemon calls. Tokens are
//! minted only by `weft token issue` over the local socket, never over HTTP.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;

use super::daemon_facade::DaemonKernelFacade;

/// Longest a positive validation is served from cache (ADR-102 D3). A token
/// revoked through this gateway bypasses the cache at once; one revoked by
/// `weft token revoke` is refused within this window.
pub const POSITIVE_CACHE_TTL: Duration = Duration::from_secs(30);

/// Upper bound on cached validations; the oldest are dropped past this.
const CACHE_MAX_ENTRIES: usize = 1024;

/// Public metadata of a validated token (never the secret).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TokenMeta {
    /// Token id (first 16 hex of the secret's hash).
    pub id: String,
    /// Operator-chosen label.
    pub label: String,
    /// RFC 3339 issue time.
    pub issued_at: String,
    /// RFC 3339 expiry time.
    pub expires_at: String,
}

/// Outcome of checking a bearer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenCheck {
    /// Known, unexpired, not revoked, owner scope.
    Valid(TokenMeta),
    /// Unknown, expired, revoked, or not an owner token.
    Invalid,
    /// The authority (daemon) could not be reached, so nothing can be proven.
    Unavailable,
}

/// Outcome of a revoke request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeOutcome {
    /// The token is now revoked (or already was).
    Revoked,
    /// The authority could not be reached; the token is still live.
    Unavailable,
}

/// Source of truth for gateway bearer tokens.
#[async_trait]
pub trait TokenValidator: Send + Sync {
    /// Check a bearer secret.
    async fn validate(&self, token: &str) -> TokenCheck;
    /// Revoke the token with this id.
    async fn revoke(&self, id: &str) -> RevokeOutcome;
}

/// Validates bearers through the kernel daemon (`auth.token.validate`).
pub struct DaemonTokenValidator {
    facade: Arc<DaemonKernelFacade>,
    ttl: Duration,
    keys: RandomState,
    cache: Mutex<HashMap<u64, (Instant, TokenMeta)>>,
}

impl DaemonTokenValidator {
    /// Validator that reaches the daemon through `facade`.
    pub fn new(facade: Arc<DaemonKernelFacade>) -> Self {
        Self {
            facade,
            ttl: POSITIVE_CACHE_TTL,
            keys: RandomState::new(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Override the positive-cache lifetime (tests).
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Cache key: a keyed 64-bit hash, so secrets are not retained in memory.
    fn key(&self, token: &str) -> u64 {
        self.keys.hash_one(token)
    }

    fn cached(&self, key: u64) -> Option<TokenMeta> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match cache.get(&key) {
            Some((at, meta)) if at.elapsed() < self.ttl && !is_expired(meta) => {
                Some(meta.clone())
            }
            Some(_) => {
                cache.remove(&key);
                None
            }
            None => None,
        }
    }

    fn remember(&self, key: u64, meta: TokenMeta) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= CACHE_MAX_ENTRIES {
            cache.retain(|_, (at, m)| at.elapsed() < self.ttl && !is_expired(m));
        }
        if cache.len() >= CACHE_MAX_ENTRIES
            && let Some(oldest) = cache.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| *k)
        {
            cache.remove(&oldest);
        }
        cache.insert(key, (Instant::now(), meta));
    }
}

fn is_expired(meta: &TokenMeta) -> bool {
    DateTime::parse_from_rfc3339(&meta.expires_at)
        .map(|t| t.with_timezone(&Utc) <= Utc::now())
        .unwrap_or(true)
}

/// Parse the `token` object of an `auth.token.validate` reply. Only owner
/// tokens pass: a project token (ADR-103) is a child kernel's credential for
/// the user daemon, not a gateway operator credential.
fn parse_meta(info: &serde_json::Value) -> Option<TokenMeta> {
    if info.get("scope").and_then(|v| v.as_str()) != Some("owner") {
        return None;
    }
    if info.get("project").is_some_and(|v| !v.is_null()) {
        return None;
    }
    let field = |k: &str| info.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    Some(TokenMeta {
        id: field("id")?,
        label: field("label")?,
        issued_at: field("issued_at")?,
        expires_at: field("expires_at")?,
    })
}

#[async_trait]
impl TokenValidator for DaemonTokenValidator {
    async fn validate(&self, token: &str) -> TokenCheck {
        let key = self.key(token);
        if let Some(meta) = self.cached(key) {
            return TokenCheck::Valid(meta);
        }
        // `read` is enough to validate; never send `admin` for this.
        let resp = match self
            .facade
            .auth_call(
                "auth.token.validate",
                serde_json::json!({ "token": token }),
                "read",
            )
            .await
        {
            Ok(r) => r,
            Err(_) => return TokenCheck::Unavailable,
        };
        if !resp.ok {
            tracing::warn!(kind = ?resp.error_kind, "daemon refused auth.token.validate");
            return TokenCheck::Unavailable;
        }
        let result = resp.result.unwrap_or(serde_json::Value::Null);
        if result.get("valid").and_then(|v| v.as_bool()) != Some(true) {
            return TokenCheck::Invalid;
        }
        match result.get("token").and_then(parse_meta) {
            Some(meta) if !is_expired(&meta) => {
                self.remember(key, meta.clone());
                TokenCheck::Valid(meta)
            }
            _ => TokenCheck::Invalid,
        }
    }

    async fn revoke(&self, id: &str) -> RevokeOutcome {
        // Drop cached validations for this id first, so a revoke that races
        // a daemon failure still cannot be served from cache afterwards.
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, (_, m)| m.id != id);
        // Revoking needs `admin`: the gateway is the local owner's process and
        // only ever revokes the id of the bearer that authenticated the call.
        match self
            .facade
            .auth_call("auth.token.revoke", serde_json::json!({ "id": id }), "admin")
            .await
        {
            Ok(resp) if resp.ok => RevokeOutcome::Revoked,
            Ok(resp) => {
                tracing::warn!(kind = ?resp.error_kind, "daemon refused auth.token.revoke");
                RevokeOutcome::Unavailable
            }
            Err(_) => RevokeOutcome::Unavailable,
        }
    }
}

/// In-memory validator for tests and embedders that have no daemon. The
/// gateway never uses it: `weft gateway` always builds a
/// [`DaemonTokenValidator`].
#[derive(Default)]
pub struct MemoryTokenValidator {
    tokens: RwLock<HashMap<String, MemoryEntry>>,
}

struct MemoryEntry {
    id: String,
    created_at: Instant,
    ttl_secs: u64,
    revoked: bool,
}

impl MemoryTokenValidator {
    /// Empty validator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a token valid for `ttl_secs`. `None` if the lock is poisoned.
    pub fn generate_token(&self, ttl_secs: u64) -> Option<String> {
        let token = uuid::Uuid::new_v4().to_string();
        let entry = MemoryEntry {
            id: token.chars().take(16).collect(),
            created_at: Instant::now(),
            ttl_secs,
            revoked: false,
        };
        self.tokens.write().ok()?.insert(token.clone(), entry);
        Some(token)
    }

    /// Revoke by secret (tests). `true` when it was live.
    pub fn revoke_token(&self, token: &str) -> bool {
        let Ok(mut tokens) = self.tokens.write() else {
            return false;
        };
        match tokens.get_mut(token) {
            Some(e) if !e.revoked => {
                e.revoked = true;
                true
            }
            _ => false,
        }
    }
}

#[async_trait]
impl TokenValidator for MemoryTokenValidator {
    async fn validate(&self, token: &str) -> TokenCheck {
        let Ok(tokens) = self.tokens.read() else {
            return TokenCheck::Invalid;
        };
        match tokens.get(token) {
            Some(e) if !e.revoked && e.created_at.elapsed().as_secs() < e.ttl_secs => {
                let issued = Utc::now() - chrono::Duration::seconds(e.created_at.elapsed().as_secs() as i64);
                TokenCheck::Valid(TokenMeta {
                    id: e.id.clone(),
                    label: "memory".into(),
                    issued_at: issued.to_rfc3339(),
                    expires_at: (issued + chrono::Duration::seconds(e.ttl_secs as i64)).to_rfc3339(),
                })
            }
            _ => TokenCheck::Invalid,
        }
    }

    async fn revoke(&self, id: &str) -> RevokeOutcome {
        if let Ok(mut tokens) = self.tokens.write() {
            for e in tokens.values_mut().filter(|e| e.id == id) {
                e.revoked = true;
            }
        }
        RevokeOutcome::Revoked
    }
}

/// Refuse to serve the authenticated API on a non-loopback address over
/// plain HTTP (ADR-102 security notes).
///
/// A bearer token is owner-equivalent (full REST surface, MCP `full`
/// profile), so on a LAN it is as good as shell access. The gateway has no
/// TLS of its own; a non-loopback bind is accepted only when the operator
/// states that TLS is terminated in front (`allow_plain_http`, from
/// `--dangerously-plain-http` or `gateway.dangerously_plain_http`).
pub fn validate_bind_policy(host: &str, allow_plain_http: bool) -> Result<(), String> {
    match crate::mcp::classify_bind_host(host) {
        crate::mcp::BindKind::Loopback => Ok(()),
        crate::mcp::BindKind::Public if allow_plain_http => Ok(()),
        crate::mcp::BindKind::Public => Err(format!(
            "refusing to serve the API on non-loopback address '{host}' over plain HTTP: \
             a bearer token grants the full API. Terminate TLS in front of the gateway and \
             pass --dangerously-plain-http (or set gateway.dangerously_plain_http), or bind \
             127.0.0.1"
        )),
    }
}

/// Public route allowlist — paths reachable without a token (health probe,
/// OPTIONS preflight). `/api/health` serves a minimal body to anonymous
/// callers and the full status document when a valid bearer is presented
/// (ADR-102 D1).
///
/// Paths are matched against both the full URI (e.g. `/api/health`) and
/// the nest-relative URI (e.g. `/health`) because `route_layer` on a
/// `nest("/api", ...)` sees the inner router's relative path.
const PUBLIC_PATHS: &[&str] = &[
    "/api/health",
    // Nest-relative variant:
    "/health",
];

/// Returns `true` if the given path is on the auth allowlist and should
/// bypass token validation.
pub fn is_public_path(path: &str) -> bool {
    PUBLIC_PATHS.contains(&path)
}

/// Tower middleware that validates Bearer tokens on protected routes.
///
/// Requests to paths in [`PUBLIC_PATHS`] are exempt from authentication,
/// but a valid bearer on them is still recognised so the handler can serve
/// the detailed view. All other `/api/*` routes require a valid Bearer
/// token in the `Authorization` header. A validated token's [`TokenMeta`]
/// is placed in the request extensions.
///
/// CORS preflight requests (`OPTIONS`) are also allowed through so the
/// browser can complete its preflight before retrying with the actual
/// `Authorization` header.
///
/// On rejection the middleware responds with HTTP 401 and a
/// `WWW-Authenticate: Bearer` header, or 503 when the daemon that owns the
/// tokens is down.
///
/// # Usage
///
/// Wired in [`super::build_router`] via `route_layer` on the `/api` nest.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<super::ApiState>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::response::Response> {
    // Always permit CORS preflight; the browser cannot attach Authorization
    // on the preflight OPTIONS request.
    if request.method() == axum::http::Method::OPTIONS {
        return Ok(next.run(request).await);
    }

    let public = is_public_path(request.uri().path());
    match check_request(&state, credentials(&request, false)).await {
        TokenCheck::Valid(meta) => {
            request.extensions_mut().insert(meta);
            Ok(next.run(request).await)
        }
        // Anonymous (or bad-token) callers still get the minimal health view.
        _ if public => Ok(next.run(request).await),
        TokenCheck::Invalid => Err(unauthorized_response()),
        TokenCheck::Unavailable => Err(unavailable_response()),
    }
}

/// WebSocket-aware variant of [`auth_middleware`].
///
/// Browsers cannot easily set the `Authorization` header on the WebSocket
/// upgrade request, so this middleware additionally accepts a `?token=...`
/// query parameter. Used on the `/ws` route.
pub async fn ws_auth_middleware(
    axum::extract::State(state): axum::extract::State<super::ApiState>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::response::Response> {
    match check_request(&state, credentials(&request, true)).await {
        TokenCheck::Valid(meta) => {
            request.extensions_mut().insert(meta);
            Ok(next.run(request).await)
        }
        TokenCheck::Invalid => Err(unauthorized_response()),
        TokenCheck::Unavailable => Err(unavailable_response()),
    }
}

/// Credentials presented on a request, copied out so no borrow of the
/// (non-`Sync`) request body is held across an await.
struct Credentials {
    bearer: Option<String>,
    query: Option<String>,
}

/// Collect the bearer in `Authorization: Bearer <token>` and, when
/// `allow_query`, the `?token=` parameter.
fn credentials(request: &axum::extract::Request, allow_query: bool) -> Credentials {
    Credentials {
        bearer: request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(str::to_owned),
        query: if allow_query { query_token(request) } else { None },
    }
}

/// Validate the presented credentials. A bad header falls through to the
/// query token (WebSocket only); no credential at all is `Invalid`.
async fn check_request(state: &super::ApiState, creds: Credentials) -> TokenCheck {
    if let Some(tok) = &creds.bearer {
        match state.auth.validate(tok).await {
            TokenCheck::Invalid if creds.query.is_some() => {}
            other => return other,
        }
    }
    if let Some(tok) = &creds.query {
        return state.auth.validate(tok).await;
    }
    TokenCheck::Invalid
}

/// The `?token=<token>` query value, if any. Tokens are `wft_` plus hex, so
/// no percent-decoding is needed.
fn query_token(request: &axum::extract::Request) -> Option<String> {
    request
        .uri()
        .query()?
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))
        .map(str::to_owned)
}

/// Build a 401 response with a `WWW-Authenticate: Bearer` header.
fn unauthorized_response() -> axum::response::Response {
    use axum::response::IntoResponse;
    let mut response = axum::http::StatusCode::UNAUTHORIZED.into_response();
    response.headers_mut().insert(
        axum::http::header::WWW_AUTHENTICATE,
        axum::http::HeaderValue::from_static("Bearer"),
    );
    response
}

/// Build a 503: tokens cannot be checked while the daemon is down.
fn unavailable_response() -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(serde_json::json!({
            "error": "daemon unavailable",
            "remedy": "start the daemon: weft kernel start",
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_validator_generate_and_validate() {
        let store = MemoryTokenValidator::new();
        let token = store.generate_token(3600).expect("generate_token failed");
        assert!(matches!(store.validate(&token).await, TokenCheck::Valid(_)));
        assert_eq!(store.validate("not-a-real-token").await, TokenCheck::Invalid);
    }

    #[tokio::test]
    async fn memory_validator_revoke_by_secret_and_id() {
        let store = MemoryTokenValidator::new();
        let token = store.generate_token(3600).unwrap();
        assert!(store.revoke_token(&token));
        assert_eq!(store.validate(&token).await, TokenCheck::Invalid);
        assert!(!store.revoke_token(&token));

        let t2 = store.generate_token(3600).unwrap();
        let TokenCheck::Valid(meta) = store.validate(&t2).await else {
            panic!("live token must validate");
        };
        assert_eq!(store.revoke(&meta.id).await, RevokeOutcome::Revoked);
        assert_eq!(store.validate(&t2).await, TokenCheck::Invalid);
    }

    #[test]
    fn bind_policy_refuses_plain_http_off_loopback() {
        for h in ["127.0.0.1", "localhost", "::1", "[::1]"] {
            assert!(validate_bind_policy(h, false).is_ok(), "{h}");
        }
        for h in ["0.0.0.0", "::", "192.0.2.10", "gateway.example"] {
            let e = validate_bind_policy(h, false).unwrap_err();
            assert!(e.contains("--dangerously-plain-http"), "{h}: {e}");
            assert!(validate_bind_policy(h, true).is_ok(), "{h}");
        }
    }

    #[test]
    fn only_health_is_public() {
        assert!(is_public_path("/api/health"));
        assert!(is_public_path("/health"));
        // The mint route is gone and must never come back as public.
        assert!(!is_public_path("/api/auth/token"));
        assert!(!is_public_path("/auth/token"));
        assert!(!is_public_path("/api/status"));
    }

    #[test]
    fn parse_meta_accepts_owner_only() {
        let owner = serde_json::json!({
            "id": "a", "label": "l", "issued_at": "2026-01-01T00:00:00Z",
            "expires_at": "2099-01-01T00:00:00Z", "scope": "owner", "project": null,
        });
        assert!(parse_meta(&owner).is_some());
        let mut project_scope = owner.clone();
        project_scope["scope"] = "project".into();
        assert!(parse_meta(&project_scope).is_none());
        let mut claim = owner.clone();
        claim["project"] = "01HZXAAAAAAAAAAAAAAAAAAAAA".into();
        assert!(parse_meta(&claim).is_none());
    }
}
