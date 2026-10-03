//! `GET /api/openapi.json`: the gateway's OpenAPI 3.1 description.
//!
//! The document is a hand-maintained static asset (`openapi.json`, embedded
//! at compile time) rather than generated from the handlers, which keeps a
//! macro dependency out of every route. The tests below fail when a route
//! the router serves is missing from the spec, or the spec lists one that is
//! gone, so it cannot drift silently.
//!
//! Access: behind the bearer token like every route except health. The spec
//! holds no secrets, but it maps the whole surface (including admin and
//! shell-adjacent routes), and nothing needs it before a token exists: the
//! playground holds a token before it fetches it.

use axum::{
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};

/// The embedded OpenAPI 3.1 document.
pub const SPEC: &str = include_str!("openapi.json");

/// `GET /api/openapi.json`.
pub async fn openapi_json() -> Response {
    let mut resp = SPEC.into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    resp
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode, header};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::SPEC;
    use crate::api::auth::MemoryTokenValidator;
    use crate::api::build_router;
    use crate::api::mcp_mount::McpMount;
    use crate::api::ws::tests::stub_state;
    use crate::mcp::ToolDefinition;
    use crate::mcp::composite::CompositeToolProvider;
    use crate::mcp::middleware::AuditLog;
    use crate::mcp::provider::{CallToolResult, ToolError, ToolProvider};
    use crate::mcp::server::McpServerShell;

    const METHODS: [&str; 5] = ["get", "post", "put", "delete", "patch"];

    /// Routes that sit at the top level, not under the `/api` nest.
    const TOP_LEVEL: [&str; 4] = ["/ws", "/events", "/custody/witness", "/mcp"];

    fn spec() -> Value {
        serde_json::from_str(SPEC).expect("openapi.json is valid JSON")
    }

    /// `"METHOD /path"` for every operation the spec documents.
    fn spec_ops() -> BTreeSet<String> {
        let spec = spec();
        let mut out = BTreeSet::new();
        for (path, item) in spec["paths"].as_object().unwrap() {
            for (m, _) in item.as_object().unwrap() {
                assert!(METHODS.contains(&m.as_str()), "{path}: unexpected key {m}");
                out.insert(format!("{} {path}", m.to_uppercase()));
            }
        }
        out
    }

    /// `"METHOD /path"` for every `.route("path", get(..).post(..))` in the
    /// API sources. A route statement runs to the next `.route(`, `.merge(`
    /// or end of statement.
    fn source_ops() -> BTreeSet<String> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/api");
        let route = regex::Regex::new(r#"\.route\(\s*"([^"]+)"\s*,"#).unwrap();
        let end = regex::Regex::new(r"\.route\(|\.merge\(|;\s*\n").unwrap();
        let method = regex::Regex::new(r"\b(get|post|put|delete|patch)\(").unwrap();
        let mut out = BTreeSet::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            // `/playground` serves an HTML page, not an API operation.
            if path.file_name().is_some_and(|n| n == "playground.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let code = text.split("#[cfg(test)]").next().unwrap();
            for m in route.captures_iter(code) {
                let whole = m.get(0).unwrap();
                let rest = &code[whole.end()..];
                let seg = end.find(rest).map_or(rest, |e| &rest[..e.start()]);
                let p = &m[1];
                let full = if TOP_LEVEL.contains(&p) {
                    p.to_owned()
                } else {
                    format!("/api{p}")
                };
                let found: Vec<_> = method.captures_iter(seg).collect();
                assert!(!found.is_empty(), "{}: no method found for {p}", path.display());
                for f in found {
                    out.insert(format!("{} {full}", f[1].to_uppercase()));
                }
            }
        }
        out
    }

    #[test]
    fn spec_is_openapi_3_1_with_bearer_security() {
        let s = spec();
        assert_eq!(s["openapi"], "3.1.0");
        assert_eq!(s["components"]["securitySchemes"]["bearerAuth"]["scheme"], "bearer");
        assert_eq!(s["security"], json!([{ "bearerAuth": [] }]));
    }

    /// Only health may be called without a token.
    #[test]
    fn only_health_opts_out_of_security() {
        let s = spec();
        for (path, item) in s["paths"].as_object().unwrap() {
            for (m, op) in item.as_object().unwrap() {
                let opts_out = op.get("security").is_some();
                let is_health = path == "/api/health" && m == "get";
                assert_eq!(opts_out, is_health, "{m} {path}");
            }
        }
    }

    /// No served route is missing, and nothing documented is stale.
    #[test]
    fn spec_matches_the_routes_in_source() {
        let (spec, src) = (spec_ops(), source_ops());
        let missing: Vec<_> = src.difference(&spec).collect();
        let stale: Vec<_> = spec.difference(&src).collect();
        assert!(
            missing.is_empty() && stale.is_empty(),
            "openapi.json out of sync with the routes in src/api.\n  served but not in the spec: {missing:?}\n  in the spec but not served: {stale:?}\n(top-level, non-/api routes are listed in TOP_LEVEL in this test)"
        );
    }

    struct Echo;
    #[async_trait]
    impl ToolProvider for Echo {
        fn namespace(&self) -> &str {
            ""
        }
        fn list_tools(&self) -> Vec<ToolDefinition> {
            vec![]
        }
        async fn call_tool(&self, _: &str, _: Value) -> Result<CallToolResult, ToolError> {
            Ok(CallToolResult::text("ok"))
        }
    }

    fn full_app() -> (axum::Router, Arc<MemoryTokenValidator>) {
        let auth = Arc::new(MemoryTokenValidator::new());
        let mut state = stub_state();
        state.auth = auth.clone();
        let mut composite = CompositeToolProvider::new();
        composite.register(Box::new(Echo));
        let audit = AuditLog::new();
        let label = audit.label_handle();
        let mut shell = McpServerShell::new(composite);
        shell.add_middleware(Box::new(audit));
        state.mcp = Some(Arc::new(McpMount::new(shell, label, "full", 0)));
        (build_router(state, &[], None), auth)
    }

    fn probe_uri(path: &str) -> String {
        let placeholder = regex::Regex::new(r"\{[^}]+\}").unwrap();
        placeholder.replace_all(path, "probe-id").into_owned()
    }

    /// The router really serves every documented operation, `{param}` routes
    /// included (probed with a dummy id), so the spec cannot describe a route
    /// that only exists in source text. A handler may answer 404 for an
    /// unknown id, but then it says so in a body; the router's own "no such
    /// route" 404 is empty.
    #[tokio::test]
    async fn every_documented_operation_is_routed() {
        for op in spec_ops() {
            // Fresh router per operation so the per-IP rate limit (429)
            // cannot mask a missing route.
            let (app, auth) = full_app();
            let (m, path) = op.split_once(' ').unwrap();
            let token = auth.generate_token(60).unwrap();
            let resp = app
                .oneshot(
                    Request::builder()
                        .method(Method::from_bytes(m.as_bytes()).unwrap())
                        .uri(probe_uri(path))
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let code = resp.status();
            // Only a 404 body is read: `/events` is a stream that never ends.
            let body = if code == StatusCode::NOT_FOUND {
                axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap_or_default()
            } else {
                Default::default()
            };
            assert!(
                code != StatusCode::NOT_FOUND || !body.is_empty(),
                "{op} is documented but the router answers an empty 404"
            );
            assert_ne!(code, StatusCode::TOO_MANY_REQUESTS, "{op}");
            assert_ne!(code, StatusCode::METHOD_NOT_ALLOWED, "{op}");
            assert_ne!(code, StatusCode::UNAUTHORIZED, "{op}: a fresh token must pass");
        }
    }

    /// Anonymous callers get 401 on every documented operation except health.
    #[tokio::test]
    async fn every_documented_operation_but_health_refuses_anonymous_callers() {
        for op in spec_ops() {
            // A fresh router per operation: one client IP would otherwise
            // hit the per-IP rate limit (429) long before the list ends.
            let (app, _auth) = full_app();
            let (m, path) = op.split_once(' ').unwrap();
            let resp = app
                .oneshot(
                    Request::builder()
                        .method(Method::from_bytes(m.as_bytes()).unwrap())
                        .uri(probe_uri(path))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            if op == "GET /api/health" {
                assert_eq!(resp.status(), StatusCode::OK, "{op}");
            } else {
                assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{op}");
            }
        }
    }

    #[tokio::test]
    async fn spec_route_serves_json_and_needs_a_token() {
        let auth = Arc::new(MemoryTokenValidator::new());
        let mut state = stub_state();
        state.auth = auth.clone();
        let app = build_router(state, &[], None);

        let anon = app
            .clone()
            .oneshot(Request::builder().uri("/api/openapi.json").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);

        let token = auth.generate_token(60).unwrap();
        let ok = app
            .oneshot(
                Request::builder()
                    .uri("/api/openapi.json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(ok.headers()[header::CONTENT_TYPE], "application/json");
        let bytes = axum::body::to_bytes(ok.into_body(), 1 << 22).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap()["openapi"], "3.1.0");
    }
}
