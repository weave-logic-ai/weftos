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

/// How long an *invalid* result is remembered. Short and bounded: it stops a
/// flood of bad bearers from becoming a flood of daemon calls, while a token
/// issued a moment ago is usable again within this window.
pub const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(3);

/// Upper bound on cached validations (positive and negative each); the
/// oldest are dropped past this.
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
    /// The gateway is rate-limiting its own daemon validate calls; retry.
    Busy,
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

/// Daemon `auth.token.validate` calls allowed per second, process-wide, and
/// the burst on top. Cached results do not count. A flood of *distinct*
/// bearers is the only thing that reaches the daemon, and it is bounded here
/// whatever path it arrives on.
pub const VALIDATE_RATE_PER_SEC: f64 = 50.0;
/// Burst allowance for [`VALIDATE_RATE_PER_SEC`].
pub const VALIDATE_BURST: f64 = 100.0;
/// Most validate calls in flight at once.
const VALIDATE_CONCURRENCY: usize = 16;
/// How long a revoked token id is refused regardless of what the daemon or a
/// racing validation says.
pub const REVOKE_TOMBSTONE_TTL: Duration = Duration::from_secs(30);

/// Token bucket for daemon validate calls.
struct Bucket {
    tokens: f64,
    last: Instant,
    rate: f64,
    burst: f64,
}

impl Bucket {
    fn take(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * self.rate).min(self.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Positive cache, revocation tombstones and the revoke generation, behind
/// one lock so "was there a revoke since I started?" and "insert" are a
/// single atomic step.
#[derive(Default)]
struct Inner {
    entries: HashMap<u64, (Instant, TokenMeta)>,
    /// Revoked token id -> when it was revoked.
    revoked: HashMap<String, Instant>,
    /// Bumped on every revoke attempt.
    generation: u64,
}

impl Inner {
    fn tombstoned(&mut self, id: &str) -> bool {
        match self.revoked.get(id) {
            Some(at) if at.elapsed() < REVOKE_TOMBSTONE_TTL => true,
            Some(_) => {
                self.revoked.remove(id);
                false
            }
            None => false,
        }
    }
}

/// Validates bearers through the kernel daemon (`auth.token.validate`).
pub struct DaemonTokenValidator {
    facade: Arc<DaemonKernelFacade>,
    ttl: Duration,
    keys: RandomState,
    state: Mutex<Inner>,
    /// Invalid results by keyed hash, for [`NEGATIVE_CACHE_TTL`].
    negative: Mutex<HashMap<u64, Instant>>,
    bucket: Mutex<Bucket>,
    in_flight: tokio::sync::Semaphore,
}

impl DaemonTokenValidator {
    /// Validator that reaches the daemon through `facade`.
    pub fn new(facade: Arc<DaemonKernelFacade>) -> Self {
        Self {
            facade,
            ttl: POSITIVE_CACHE_TTL,
            keys: RandomState::new(),
            state: Mutex::new(Inner::default()),
            negative: Mutex::new(HashMap::new()),
            bucket: Mutex::new(Bucket {
                tokens: VALIDATE_BURST,
                last: Instant::now(),
                rate: VALIDATE_RATE_PER_SEC,
                burst: VALIDATE_BURST,
            }),
            in_flight: tokio::sync::Semaphore::new(VALIDATE_CONCURRENCY),
        }
    }

    /// Override the positive-cache lifetime (tests).
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Override the daemon validate budget (tests).
    pub fn with_validate_budget(self, per_sec: f64, burst: f64) -> Self {
        *self.bucket.lock().unwrap() = Bucket {
            tokens: burst,
            last: Instant::now(),
            rate: per_sec,
            burst,
        };
        self
    }

    /// Cache key: a keyed 64-bit hash, so secrets are not retained in memory.
    fn key(&self, token: &str) -> u64 {
        self.keys.hash_one(token)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn cached(&self, key: u64) -> Option<TokenMeta> {
        let mut st = self.lock();
        let hit = st.entries.get(&key).cloned();
        match hit {
            Some((at, meta))
                if at.elapsed() < self.ttl && !is_expired(&meta) && !st.tombstoned(&meta.id) =>
            {
                Some(meta)
            }
            Some(_) => {
                st.entries.remove(&key);
                None
            }
            None => None,
        }
    }

    fn recently_invalid(&self, key: u64) -> bool {
        let mut neg = self.negative.lock().unwrap_or_else(|e| e.into_inner());
        match neg.get(&key) {
            Some(at) if at.elapsed() < NEGATIVE_CACHE_TTL => true,
            Some(_) => {
                neg.remove(&key);
                false
            }
            None => false,
        }
    }

    fn remember_invalid(&self, key: u64) {
        let mut neg = self.negative.lock().unwrap_or_else(|e| e.into_inner());
        if neg.len() >= CACHE_MAX_ENTRIES {
            neg.retain(|_, at| at.elapsed() < NEGATIVE_CACHE_TTL);
        }
        if neg.len() >= CACHE_MAX_ENTRIES
            && let Some(oldest) = neg.iter().min_by_key(|(_, at)| **at).map(|(k, _)| *k)
        {
            neg.remove(&oldest);
        }
        neg.insert(key, Instant::now());
    }

    #[cfg(test)]
    fn cache_len(&self) -> (usize, usize) {
        (self.lock().entries.len(), self.negative.lock().unwrap().len())
    }

    /// The revoke generation; take it before asking the daemon.
    fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Cache `meta` only if no revoke ran since `generation` was read and its
    /// id is not tombstoned. The compare and the insert share one lock hold
    /// with the revoke's bump. Returns whether it was cached.
    fn remember(&self, key: u64, meta: TokenMeta, generation: u64) -> bool {
        let mut st = self.lock();
        if st.generation != generation || st.tombstoned(&meta.id) {
            return false;
        }
        if st.entries.len() >= CACHE_MAX_ENTRIES {
            let ttl = self.ttl;
            st.entries.retain(|_, (at, m)| at.elapsed() < ttl && !is_expired(m));
        }
        if st.entries.len() >= CACHE_MAX_ENTRIES
            && let Some(oldest) = st.entries.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| *k)
        {
            st.entries.remove(&oldest);
        }
        st.entries.insert(key, (Instant::now(), meta));
        true
    }

    /// Start of a revoke: bump the generation and drop cached entries for
    /// `id`, in one lock hold.
    fn begin_revoke(&self, id: &str) {
        let mut st = self.lock();
        st.generation += 1;
        st.entries.retain(|_, (_, m)| m.id != id);
    }

    /// The daemon confirmed the revoke: refuse `id` for a while no matter
    /// what a racing validation or a stale cache entry says.
    fn finish_revoke(&self, id: &str) {
        let mut st = self.lock();
        if st.revoked.len() >= CACHE_MAX_ENTRIES {
            st.revoked.retain(|_, at| at.elapsed() < REVOKE_TOMBSTONE_TTL);
        }
        st.revoked.insert(id.to_owned(), Instant::now());
        st.entries.retain(|_, (_, m)| m.id != id);
    }

    fn tombstoned(&self, id: &str) -> bool {
        self.lock().tombstoned(id)
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
        if self.recently_invalid(key) {
            return TokenCheck::Invalid;
        }
        // Past the caches this costs a daemon call: bound them globally.
        if !self.bucket.lock().unwrap_or_else(|e| e.into_inner()).take() {
            return TokenCheck::Busy;
        }
        let Ok(_permit) = self.in_flight.acquire().await else {
            return TokenCheck::Unavailable;
        };
        let generation = self.generation();
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
            self.remember_invalid(key);
            return TokenCheck::Invalid;
        }
        match result.get("token").and_then(parse_meta) {
            // A reply that raced a revoke of this very token is not trusted.
            Some(meta) if self.tombstoned(&meta.id) => TokenCheck::Invalid,
            Some(meta) if !is_expired(&meta) => {
                self.remember(key, meta.clone(), generation);
                TokenCheck::Valid(meta)
            }
            _ => {
                self.remember_invalid(key);
                TokenCheck::Invalid
            }
        }
    }

    async fn revoke(&self, id: &str) -> RevokeOutcome {
        self.begin_revoke(id);
        // Revoking needs `admin`: the gateway is the local owner's process and
        // only ever revokes the id of the bearer that authenticated the call.
        match self
            .facade
            .auth_call("auth.token.revoke", serde_json::json!({ "id": id }), "admin")
            .await
        {
            Ok(resp) if resp.ok => {
                self.finish_revoke(id);
                RevokeOutcome::Revoked
            }
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
        TokenCheck::Busy => Err(busy_response()),
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
        TokenCheck::Busy => Err(busy_response()),
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
            .and_then(bearer_token)
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

/// The token in an `Authorization` header value. The scheme is matched
/// case-insensitively (RFC 7235); the token itself is not trimmed beyond the
/// separating whitespace.
fn bearer_token(header: &str) -> Option<&str> {
    let (scheme, rest) = header.split_once(' ')?;
    let token = rest.trim_start();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
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

/// Build a 429: too many token checks are reaching the daemon right now.
fn busy_response() -> axum::response::Response {
    use axum::response::IntoResponse;
    let mut response = (
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        axum::Json(serde_json::json!({ "error": "too many authentication attempts" })),
    )
        .into_response();
    response.headers_mut().insert(
        axum::http::header::RETRY_AFTER,
        axum::http::HeaderValue::from_static("1"),
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
    fn bearer_scheme_is_case_insensitive() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer abc"), Some("abc"));
        assert_eq!(bearer_token("BEARER  abc"), Some("abc"));
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("Bearer"), None);
        assert_eq!(bearer_token("Bearer "), None);
    }

    fn offline_validator() -> DaemonTokenValidator {
        let dir = std::env::temp_dir().join("clawft-auth-unit-no-daemon.sock");
        DaemonTokenValidator::new(Arc::new(DaemonKernelFacade::with_socket(dir)))
    }

    fn meta(id: &str) -> TokenMeta {
        TokenMeta {
            id: id.into(),
            label: "l".into(),
            issued_at: "2026-01-01T00:00:00+00:00".into(),
            expires_at: "2099-01-01T00:00:00+00:00".into(),
        }
    }

    /// Neither cache can grow past its bound, however many bearers arrive.
    #[test]
    fn caches_are_bounded() {
        let v = offline_validator();
        for i in 0..(CACHE_MAX_ENTRIES as u64 + 300) {
            v.remember(i, meta(&i.to_string()), v.generation());
            v.remember_invalid(1_000_000 + i);
        }
        let (pos, neg) = v.cache_len();
        assert_eq!(pos, CACHE_MAX_ENTRIES);
        assert_eq!(neg, CACHE_MAX_ENTRIES);
    }

    /// Forced interleave 1: a validation reads the generation, a revoke
    /// starts, then the validation tries to cache. The compare happens under
    /// the lock the revoke's bump holds, so the insert is refused.
    #[test]
    fn stale_generation_cannot_populate_the_cache() {
        let v = offline_validator();
        let g = v.generation();
        v.begin_revoke("id-a");
        assert!(!v.remember(1, meta("id-a"), g));
        assert_eq!(v.cache_len().0, 0);
        // A fresh read after the revoke began may cache (e.g. another token),
        assert!(v.remember(2, meta("id-b"), v.generation()));
    }

    /// Forced interleave 2: a validation that read the generation *after*
    /// the revoke began (daemon not yet updated) caches; once the daemon
    /// confirms, the tombstone evicts it and refuses it from then on.
    #[test]
    fn tombstone_evicts_and_blocks_a_late_cache_entry() {
        let v = offline_validator();
        v.begin_revoke("id-a");
        assert!(v.remember(1, meta("id-a"), v.generation()), "cached before the daemon confirms");
        assert!(v.cached(1).is_some());
        v.finish_revoke("id-a");
        assert!(v.cached(1).is_none(), "tombstone evicts the entry");
        assert!(!v.remember(1, meta("id-a"), v.generation()), "and refuses a re-insert");
        assert!(v.tombstoned("id-a"));
        assert!(!v.tombstoned("id-other"));
    }

    #[test]
    fn tombstones_lapse() {
        let v = offline_validator();
        v.finish_revoke("id-a");
        v.lock()
            .revoked
            .insert("id-a".into(), Instant::now() - REVOKE_TOMBSTONE_TTL - Duration::from_millis(1));
        assert!(!v.tombstoned("id-a"));
    }

    #[test]
    fn validate_budget_is_a_token_bucket() {
        let v = offline_validator().with_validate_budget(0.0, 3.0);
        let take = || v.bucket.lock().unwrap().take();
        assert!(take() && take() && take());
        assert!(!take(), "burst exhausted, no refill at rate 0");
    }

    #[test]
    fn invalid_results_are_remembered_only_briefly() {
        let v = offline_validator();
        let key = v.key("wft_bad");
        assert!(!v.recently_invalid(key));
        v.remember_invalid(key);
        assert!(v.recently_invalid(key));
        // Age the entry past the TTL.
        v.negative
            .lock()
            .unwrap()
            .insert(key, Instant::now() - NEGATIVE_CACHE_TTL - Duration::from_millis(1));
        assert!(!v.recently_invalid(key));
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
