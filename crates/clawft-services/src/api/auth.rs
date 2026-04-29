//! Authentication middleware and token management.

use std::collections::HashMap;
use std::sync::RwLock;

/// In-memory token store for API authentication.
pub struct TokenStore {
    tokens: RwLock<HashMap<String, TokenInfo>>,
}

/// Metadata for an issued API token.
#[derive(Clone)]
pub struct TokenInfo {
    pub created_at: std::time::Instant,
    pub ttl_secs: u64,
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
        };
        self.tokens.write().ok()?.insert(token.clone(), info);
        Some(token)
    }

    /// Validate a token. Returns true if valid and not expired.
    pub fn validate(&self, token: &str) -> bool {
        let tokens = match self.tokens.read() {
            Ok(t) => t,
            Err(_) => return false, // poisoned lock -- deny access
        };
        if let Some(info) = tokens.get(token) {
            info.created_at.elapsed().as_secs() < info.ttl_secs
        } else {
            false
        }
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
}
