//! Gateway middleware: CSP, CORS (deny-by-default), and per-endpoint rate limit.
//!
//! These middlewares are wired in [`super::build_router`] alongside the
//! authentication middleware in [`super::auth::auth_middleware`]. Together
//! they implement the four security gates required by WEFT-99, WEFT-100,
//! WEFT-101, and WEFT-298:
//!
//! 1. **Auth** — Bearer-token gate on `/api/*` and `/ws` (auth.rs).
//! 2. **CORS** — Deny-by-default origin policy with localhost fallback.
//! 3. **Rate limit** — Per-IP token-bucket per endpoint class.
//! 4. **CSP** — `Content-Security-Policy` header on every response.
//!
//! The implementation deliberately avoids new third-party crates: rate
//! limiting is a small in-process token-bucket map keyed by client IP.
//! CSP and CORS use `tower-http` features already enabled.

use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::ConnectInfo;
use axum::http::{
    HeaderName, HeaderValue, Method, Request, StatusCode,
    header::{AUTHORIZATION, CONTENT_TYPE},
};
use axum::middleware::Next;
use axum::response::Response;
use tower_http::cors::{AllowOrigin, CorsLayer};

/// Best-effort extraction of a peer IP for rate-limit keying.
///
/// Order of preference:
///   1. `ConnectInfo<SocketAddr>` extension (set by
///      `into_make_service_with_connect_info`).
///   2. `X-Forwarded-For` (first hop).
///   3. The literal `"unknown"` so requests without a recognizable
///      origin still bucket together (and therefore still rate-limit
///      against each other).
fn client_ip(req: &Request<axum::body::Body>) -> String {
    if let Some(ConnectInfo(addr)) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
        return addr.ip().to_string();
    }
    if let Some(value) = req.headers().get("x-forwarded-for")
        && let Ok(s) = value.to_str()
        && let Some(first) = s.split(',').next()
    {
        let trimmed = first.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "unknown".to_string()
}

/// Default Content-Security-Policy applied to every gateway response.
///
/// The policy is intentionally tight: only same-origin scripts, with
/// `wasm-unsafe-eval` to allow the browser-side WASM modules to compile;
/// `unsafe-inline` for styles to keep the existing UI working; and
/// websocket connections back to the same origin.
pub const CSP_HEADER_VALUE: &str = "default-src 'self'; \
script-src 'self' 'wasm-unsafe-eval'; \
style-src 'self' 'unsafe-inline'; \
img-src 'self' data:; \
connect-src 'self' ws: wss:; \
frame-ancestors 'none'";

/// Axum `from_fn` middleware that adds the [`CSP_HEADER_VALUE`] header to
/// every outgoing response (including the SPA fallback served by
/// `ServeDir`).
pub async fn csp_middleware(request: Request<axum::body::Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if !headers.contains_key("content-security-policy") {
        headers.insert(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(CSP_HEADER_VALUE),
        );
    }
    response
}

/// Build the CORS layer used by the gateway.
///
/// Behavior:
/// - If `WEFTOS_API_ALLOWED_ORIGINS` is set (comma-separated), only those
///   exact origins are accepted.
/// - Otherwise, only `http://localhost:*` and `http://127.0.0.1:*` are
///   accepted (development default).
/// - The legacy `cors_origins: &[String]` parameter from
///   [`super::serve`] still wins if provided non-empty (for backward
///   compatibility with config files that already set it).
///
/// Methods allowed: GET, POST, PUT, DELETE, PATCH, OPTIONS.
/// Headers allowed: `Authorization`, `Content-Type`, `X-Request-ID`.
/// Max-age: 600 seconds (per browser preflight cache).
pub fn build_cors_layer(config_origins: &[String]) -> CorsLayer {
    let methods = [
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::DELETE,
        Method::PATCH,
        Method::OPTIONS,
    ];
    let allowed_headers = [
        AUTHORIZATION,
        CONTENT_TYPE,
        HeaderName::from_static("x-request-id"),
    ];

    // Resolve origin list.
    //   1. config_origins (if non-empty)
    //   2. WEFTOS_API_ALLOWED_ORIGINS env var
    //   3. localhost / 127.0.0.1 predicate
    let env_origins: Vec<String> = env::var("WEFTOS_API_ALLOWED_ORIGINS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let explicit_origins: Vec<String> = if !config_origins.is_empty() {
        config_origins.to_vec()
    } else {
        env_origins
    };

    let allow_origin = if !explicit_origins.is_empty() {
        let parsed: Vec<HeaderValue> = explicit_origins
            .iter()
            .filter_map(|o| HeaderValue::from_str(o).ok())
            .collect();
        AllowOrigin::list(parsed)
    } else {
        // Localhost-only default.
        AllowOrigin::predicate(|origin: &HeaderValue, _request_parts| {
            let s = match origin.to_str() {
                Ok(s) => s,
                Err(_) => return false,
            };
            is_localhost_origin(s)
        })
    };

    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods(methods)
        .allow_headers(allowed_headers)
        .max_age(Duration::from_secs(600))
}

/// Returns true if `origin` is a localhost-style origin
/// (`http://localhost`, `http://127.0.0.1`, or either with any port).
fn is_localhost_origin(origin: &str) -> bool {
    // Strip the scheme.
    let rest = match origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    {
        Some(r) => r,
        None => return false,
    };
    // Strip the optional port.
    let host = rest.split(':').next().unwrap_or(rest);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

// ─────────────────────────────────────────────────────────────────────────
// Rate limiting
// ─────────────────────────────────────────────────────────────────────────

/// Endpoint class for rate-limit selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RateClass {
    /// Auth & token endpoints (`/api/auth/*`, `/api/token/*`).
    Auth,
    /// Generic API (`/api/*`).
    Api,
    /// WebSocket (`/ws`).
    Ws,
}

impl RateClass {
    /// Per-minute request budget for this class.
    const fn per_minute(self) -> u32 {
        match self {
            RateClass::Auth => 10,
            RateClass::Api => 60,
            RateClass::Ws => 10,
        }
    }

    /// Identify the rate class from a request path. `None` means the
    /// path is not subject to rate limiting (static SPA, health, etc.).
    fn from_path(path: &str) -> Option<Self> {
        if path == "/ws" {
            Some(Self::Ws)
        } else if path.starts_with("/api/auth/") || path.starts_with("/api/token/") {
            Some(Self::Auth)
        } else if path.starts_with("/api/") {
            // Health is exempt: cheap, must always succeed for k8s probes.
            if path == "/api/health" {
                None
            } else {
                Some(Self::Api)
            }
        } else {
            None
        }
    }
}

/// State for the per-IP per-class rate limiter.
///
/// One entry per `(ip, class)` tracks the start of the current 60-second
/// window and the number of requests seen in it. The map is mutex-guarded;
/// contention is negligible because each request only takes the lock for
/// a few microseconds.
#[derive(Default)]
pub struct RateLimitState {
    inner: Mutex<HashMap<(String, RateClass), Bucket>>,
}

#[derive(Clone, Copy)]
struct Bucket {
    window_start: Instant,
    count: u32,
}

impl RateLimitState {
    /// Construct a fresh, empty rate-limit state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if the request is allowed, `false` if it should be
    /// rate limited (HTTP 429).
    fn check(&self, ip: &str, class: RateClass) -> bool {
        let now = Instant::now();
        let mut map = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return true, // poisoned lock — fail open
        };
        let bucket = map.entry((ip.to_string(), class)).or_insert(Bucket {
            window_start: now,
            count: 0,
        });
        if now.duration_since(bucket.window_start) >= Duration::from_secs(60) {
            bucket.window_start = now;
            bucket.count = 0;
        }
        bucket.count += 1;
        bucket.count <= class.per_minute()
    }
}

/// Axum `from_fn_with_state` middleware that enforces the per-IP
/// per-endpoint-class rate limits documented on [`RateClass`].
pub async fn rate_limit_middleware(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<RateLimitState>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let class = match RateClass::from_path(request.uri().path()) {
        Some(c) => c,
        None => return Ok(next.run(request).await),
    };
    let ip = client_ip(&request);
    if state.check(&ip, class) {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::TOO_MANY_REQUESTS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_class_classifies_paths() {
        assert_eq!(RateClass::from_path("/ws"), Some(RateClass::Ws));
        assert_eq!(
            RateClass::from_path("/api/auth/token"),
            Some(RateClass::Auth)
        );
        assert_eq!(
            RateClass::from_path("/api/token/refresh"),
            Some(RateClass::Auth)
        );
        assert_eq!(RateClass::from_path("/api/agents"), Some(RateClass::Api));
        assert_eq!(RateClass::from_path("/api/health"), None);
        assert_eq!(RateClass::from_path("/index.html"), None);
    }

    #[test]
    fn rate_limit_blocks_after_quota() {
        let state = RateLimitState::new();
        // Auth class allows 10 requests.
        for _ in 0..10 {
            assert!(state.check("1.2.3.4", RateClass::Auth));
        }
        assert!(!state.check("1.2.3.4", RateClass::Auth));
    }

    #[test]
    fn rate_limit_separate_ips() {
        let state = RateLimitState::new();
        for _ in 0..10 {
            assert!(state.check("1.1.1.1", RateClass::Auth));
        }
        // Different IP starts fresh.
        assert!(state.check("2.2.2.2", RateClass::Auth));
    }

    #[test]
    fn rate_limit_general_api_allows_60() {
        let state = RateLimitState::new();
        for _ in 0..60 {
            assert!(state.check("9.9.9.9", RateClass::Api));
        }
        assert!(!state.check("9.9.9.9", RateClass::Api));
    }

    #[test]
    fn localhost_origin_predicate() {
        assert!(is_localhost_origin("http://localhost"));
        assert!(is_localhost_origin("http://localhost:3000"));
        assert!(is_localhost_origin("http://127.0.0.1"));
        assert!(is_localhost_origin("http://127.0.0.1:8080"));
        assert!(is_localhost_origin("https://localhost:5173"));
        assert!(!is_localhost_origin("http://example.com"));
        assert!(!is_localhost_origin("https://evil.localhost.com"));
    }
}
