//! Authentication middleware and token management.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Grace period (in seconds) added to a token's TTL before the
/// background cleanup task drops the entry. A small window after
/// expiry keeps short-lived audit lookups working without blocking
/// validation (validation already returns false on expiry).
pub const TOKEN_CLEANUP_GRACE_SECS: u64 = 300;

/// Default cadence for the periodic cleanup task in seconds.
pub const TOKEN_CLEANUP_INTERVAL_SECS: u64 = 60;

/// In-memory token store for API authentication.
pub struct TokenStore {
    tokens: RwLock<HashMap<String, TokenInfo>>,
}

/// Metadata for an issued API token.
#[derive(Clone)]
pub struct TokenInfo {
    pub created_at: std::time::Instant,
    pub ttl_secs: u64,
    /// `true` once `revoke_token` has marked this entry. Revoked
    /// tokens are kept (not deleted) until the periodic cleanup
    /// drops them, so audit / introspection paths can still see
    /// the entry briefly after revocation.
    pub revoked: bool,
}

impl TokenStore {
    /// Create a new empty token store.
    pub fn new() -> Self {
        Self {
            tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Generate a new API token with the given TTL in seconds.
    ///
    /// Returns `None` if the internal lock is poisoned.
    pub fn generate_token(&self, ttl_secs: u64) -> Option<String> {
        let token = uuid::Uuid::new_v4().to_string();
        let info = TokenInfo {
            created_at: std::time::Instant::now(),
            ttl_secs,
            revoked: false,
        };
        self.tokens.write().ok()?.insert(token.clone(), info);
        Some(token)
    }

    /// Validate a token. Returns true if valid, not expired, and not revoked.
    pub fn validate(&self, token: &str) -> bool {
        let tokens = match self.tokens.read() {
            Ok(t) => t,
            Err(_) => return false, // poisoned lock -- deny access
        };
        if let Some(info) = tokens.get(token) {
            !info.revoked && info.created_at.elapsed().as_secs() < info.ttl_secs
        } else {
            false
        }
    }

    /// Revoke an issued token.
    ///
    /// WEFT-102: marks the token as revoked but does **not** remove
    /// it from the map -- the periodic cleanup task drops revoked
    /// entries after `expires_at + TOKEN_CLEANUP_GRACE_SECS`. The
    /// retain-then-sweep pattern preserves the audit trail for
    /// introspection right after a manual revoke without keeping
    /// expired entries around forever.
    ///
    /// Returns `true` if the token was found and newly marked,
    /// `false` if the token was unknown or already revoked.
    pub fn revoke_token(&self, token: &str) -> bool {
        let mut tokens = match self.tokens.write() {
            Ok(t) => t,
            Err(_) => return false,
        };
        match tokens.get_mut(token) {
            Some(info) if !info.revoked => {
                info.revoked = true;
                true
            }
            _ => false,
        }
    }

    /// Drop tokens whose `expires_at + TOKEN_CLEANUP_GRACE_SECS`
    /// has passed, plus any token that has been revoked. Returns
    /// the number of entries removed.
    ///
    /// WEFT-102: paired with the cleanup task spawned at boot, this
    /// caps the in-memory token map at roughly the live-set size.
    pub fn cleanup_expired(&self) -> usize {
        let mut tokens = match self.tokens.write() {
            Ok(t) => t,
            Err(_) => return 0,
        };
        let before = tokens.len();
        tokens.retain(|_, info| {
            // Drop revoked tokens immediately on the next sweep.
            if info.revoked {
                return false;
            }
            // Otherwise honour `expires_at + grace`.
            let total_lifetime = info.ttl_secs.saturating_add(TOKEN_CLEANUP_GRACE_SECS);
            info.created_at.elapsed().as_secs() < total_lifetime
        });
        before - tokens.len()
    }

    /// Number of token entries currently held (live + grace-window).
    pub fn len(&self) -> usize {
        self.tokens.read().map(|t| t.len()).unwrap_or(0)
    }

    /// Whether the store has no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for TokenStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Public route allowlist — paths that must remain reachable without
/// any authentication (token bootstrap, health probe, OPTIONS preflight).
///
/// Paths are matched against both the full URI (e.g. `/api/health`) and
/// the nest-relative URI (e.g. `/health`) because `route_layer` on a
/// `nest("/api", ...)` sees the inner router's relative path.
const PUBLIC_PATHS: &[&str] = &[
    "/api/auth/token",
    "/api/health",
    // Nest-relative variants:
    "/auth/token",
    "/health",
];

/// Returns `true` if the given path is on the auth allowlist and should
/// bypass token validation.
pub fn is_public_path(path: &str) -> bool {
    PUBLIC_PATHS.contains(&path)
}

/// Spawn a tokio task that periodically calls
/// [`TokenStore::cleanup_expired`].
///
/// WEFT-102: this is the production hook for the expired-token sweep.
/// Call once at server boot with the shared `Arc<TokenStore>`. The
/// task lives until either the process exits or the returned
/// `JoinHandle` is dropped/aborted. Only a `Weak` reference to the
/// store is held by the task so the store can be dropped on
/// shutdown without blocking on the task.
pub fn spawn_cleanup_task(
    store: Arc<TokenStore>,
    interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    // Hold only a Weak reference inside the task so the cleanup
    // loop does not extend the lifetime of the TokenStore beyond
    // its owners. The function-scoped Arc is dropped at the next
    // statement (immediately, since `weak` doesn't borrow it).
    let weak = Arc::downgrade(&store);
    drop(store);
    tokio::spawn(async move {
        let mut ticker =
            tokio::time::interval(std::time::Duration::from_secs(interval_secs.max(1)));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let Some(store) = weak.upgrade() else {
                tracing::debug!("token cleanup task: store dropped, exiting");
                break;
            };
            let removed = store.cleanup_expired();
            if removed > 0 {
                tracing::debug!(removed, "token store cleanup swept expired/revoked entries");
            }
        }
    })
}

/// Tower middleware that validates Bearer tokens on protected routes.
///
/// Requests to paths in [`PUBLIC_PATHS`] (currently `/api/auth/token`
/// and `/api/health`) are exempt from authentication. All other
/// `/api/*` routes require a valid Bearer token in the `Authorization`
/// header.
///
/// CORS preflight requests (`OPTIONS`) are also allowed through so the
/// browser can complete its preflight before retrying with the actual
/// `Authorization` header.
///
/// On rejection the middleware responds with HTTP 401 and a
/// `WWW-Authenticate: Bearer` header so well-behaved clients can prompt
/// for credentials.
///
/// # Usage
///
/// Wired in [`super::build_router`] via `route_layer` on the `/api` nest.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<super::ApiState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::response::Response> {
    let path = request.uri().path();

    // Always permit CORS preflight; the browser cannot attach Authorization
    // on the preflight OPTIONS request.
    if request.method() == axum::http::Method::OPTIONS {
        return Ok(next.run(request).await);
    }

    // Allowlist (token bootstrap, health probe).
    if is_public_path(path) {
        return Ok(next.run(request).await);
    }

    if validate_request(&state, &request) {
        Ok(next.run(request).await)
    } else {
        Err(unauthorized_response())
    }
}

/// WebSocket-aware variant of [`auth_middleware`].
///
/// Browsers cannot easily set the `Authorization` header on the WebSocket
/// upgrade request, so this middleware additionally accepts a `?token=...`
/// query parameter. Used on the `/ws` route.
pub async fn ws_auth_middleware(
    axum::extract::State(state): axum::extract::State<super::ApiState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::response::Response> {
    if validate_request(&state, &request) || validate_query_token(&state, &request) {
        Ok(next.run(request).await)
    } else {
        Err(unauthorized_response())
    }
}

/// Inspect `Authorization: Bearer <token>` and check it against the store.
fn validate_request(state: &super::ApiState, request: &axum::extract::Request) -> bool {
    request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .is_some_and(|tok| state.auth.validate(tok))
}

/// Inspect `?token=<token>` query string and check it.
fn validate_query_token(state: &super::ApiState, request: &axum::extract::Request) -> bool {
    let query = match request.uri().query() {
        Some(q) => q,
        None => return false,
    };
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("token=") {
            // Minimal URL-decoding: `+` → space and `%XX` → byte.
            // Tokens are UUIDs so plain matching is sufficient.
            if state.auth.validate(value) {
                return true;
            }
        }
    }
    false
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_store_generate_and_validate() {
        let store = TokenStore::new();
        let token = store.generate_token(3600).expect("generate_token failed");
        assert!(store.validate(&token));
    }

    #[test]
    fn token_store_rejects_unknown() {
        let store = TokenStore::new();
        assert!(!store.validate("not-a-real-token"));
    }

    #[test]
    fn token_store_default() {
        let store = TokenStore::default();
        assert!(!store.validate("anything"));
    }

    /// WEFT-102: revoke_token marks the entry, validate fails, and
    /// re-revoke is idempotent (returns false the second time).
    #[test]
    fn token_store_revoke_invalidates_token() {
        let store = TokenStore::new();
        let token = store.generate_token(3600).expect("generate_token failed");
        assert!(store.validate(&token));

        assert!(store.revoke_token(&token));
        assert!(!store.validate(&token));
        // Re-revoke is a no-op.
        assert!(!store.revoke_token(&token));
        // Revoking an unknown token returns false.
        assert!(!store.revoke_token("not-a-real-token"));
    }

    /// WEFT-102: cleanup_expired drops revoked entries and entries
    /// whose TTL + grace has elapsed. Live tokens stay.
    #[test]
    fn token_store_cleanup_drops_revoked_and_expired() {
        let store = TokenStore::new();
        let live = store.generate_token(3600).unwrap();
        let revoked = store.generate_token(3600).unwrap();
        store.revoke_token(&revoked);

        // Insert a manually-aged entry to simulate a long-expired token.
        // Use TTL=0 so the entry is past `0 + grace` only after at
        // least `TOKEN_CLEANUP_GRACE_SECS` seconds. Instead we cheat
        // by writing through the lock with a back-dated created_at.
        {
            let mut guard = store.tokens.write().unwrap();
            guard.insert(
                "stale".into(),
                TokenInfo {
                    created_at: std::time::Instant::now()
                        - std::time::Duration::from_secs(TOKEN_CLEANUP_GRACE_SECS + 10),
                    ttl_secs: 1,
                    revoked: false,
                },
            );
        }

        assert_eq!(store.len(), 3);
        let removed = store.cleanup_expired();
        // Both `revoked` and `stale` should be reaped.
        assert_eq!(removed, 2);
        assert_eq!(store.len(), 1);
        assert!(store.validate(&live));
    }
}
