//! WASI HTTP client stub.
//!
//! Provides a [`WasiHttpClient`] with a self-contained API for HTTP operations.
//! Currently all methods return errors, as WASI HTTP preview2 support is not yet
//! available. Once the WASI HTTP outbound API stabilises, this will be replaced with
//! real network calls using the `wasi:http/outgoing-handler` interface.
//!
//! This module is fully decoupled from `clawft-platform` so it can compile for
//! `wasm32-wasip2` without pulling in tokio or reqwest.

use std::collections::HashMap;

/// HTTP response from a request.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// HTTP status code (e.g., 200, 404, 500).
    pub status: u16,
    /// Response headers as key-value pairs.
    pub headers: HashMap<String, String>,
    /// Raw response body bytes.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Parse body as UTF-8 text.
    pub fn text(&self) -> Result<String, std::string::FromUtf8Error> {
        String::from_utf8(self.body.clone())
    }

    /// Check if status is success (2xx).
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// HTTP client for WASI environments.
///
/// This is a stub implementation that will use WASI HTTP preview2
/// (`wasi:http/outgoing-handler`) once it is stable. Until then, all
/// methods return an error indicating the feature is not yet available.
pub struct WasiHttpClient;

impl WasiHttpClient {
    /// Create a new WASI HTTP client.
    pub fn new() -> Self {
        Self
    }

    /// Send an HTTP request with the given method, URL, headers, and optional body.
    pub fn request(
        &self,
        _method: &str,
        _url: &str,
        _headers: &HashMap<String, String>,
        _body: Option<&[u8]>,
    ) -> Result<HttpResponse, Box<dyn std::error::Error + Send + Sync>> {
        Err(
            "WASI HTTP not yet implemented: waiting for wasi:http/outgoing-handler stabilisation"
                .into(),
        )
    }

    /// Send an HTTP GET request.
    pub fn get(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
    ) -> Result<HttpResponse, Box<dyn std::error::Error + Send + Sync>> {
        self.request("GET", url, headers, None)
    }

    /// Send an HTTP POST request with a body.
    pub fn post(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<HttpResponse, Box<dyn std::error::Error + Send + Sync>> {
        self.request("POST", url, headers, Some(body))
    }
}

impl Default for WasiHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Sandboxed HTTP client (behind wasm-plugins feature)
// ---------------------------------------------------------------------------

/// Sandboxed HTTP client that validates all requests against a plugin's
/// network permissions before executing them.
///
/// Only available when the `wasm-plugins` feature is enabled.
///
/// Note: Actual HTTP execution requires a runtime (e.g., reqwest + tokio).
/// This struct currently validates the request and returns an error for
/// the actual network call. When wasmtime integration is wired (C2.8+),
/// this will be connected to a real HTTP client.
#[cfg(feature = "wasm-plugins")]
pub struct SandboxedHttpClient {
    /// The plugin sandbox that governs all access decisions.
    pub sandbox: std::sync::Arc<crate::sandbox::PluginSandbox>,
}

#[cfg(feature = "wasm-plugins")]
impl SandboxedHttpClient {
    /// Create a new sandboxed HTTP client for a plugin.
    pub fn new(sandbox: std::sync::Arc<crate::sandbox::PluginSandbox>) -> Self {
        Self { sandbox }
    }

    /// Validate an HTTP request against the plugin's sandbox permissions.
    ///
    /// Returns the validated URL if the request is permitted.
    /// This does NOT execute the request -- it only performs security validation.
    ///
    /// Use this to pre-validate before passing to an actual HTTP client.
    pub fn validate_request(
        &self,
        method: &str,
        url: &str,
        body: Option<&str>,
    ) -> Result<url::Url, crate::sandbox::HttpValidationError> {
        crate::sandbox::validate_http_request(&self.sandbox, method, url, body)
    }

    /// Send an HTTP request (validation + stub execution).
    ///
    /// The request is validated against the sandbox. If validation passes,
    /// an error is returned indicating that actual HTTP execution is not yet
    /// wired. This will be connected to reqwest once wasmtime integration
    /// is complete (C2.8).
    pub fn request(
        &self,
        method: &str,
        url: &str,
        _headers: &HashMap<String, String>,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, Box<dyn std::error::Error + Send + Sync>> {
        // Convert body bytes to str for validation
        let body_str = body.map(|b| String::from_utf8_lossy(b));
        let body_ref = body_str.as_deref();

        // Validate against sandbox
        let _validated_url = self.validate_request(method, url, body_ref)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;

        // Actual HTTP execution not yet wired -- will be connected in C2.8
        Err("sandboxed HTTP request validated but execution not yet wired (pending C2.8)".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasi_http_client_can_be_created() {
        let _client = WasiHttpClient::new();
    }

    #[test]
    fn wasi_http_client_default() {
        let _client = WasiHttpClient;
    }

    #[test]
    fn request_returns_error() {
        let client = WasiHttpClient::new();
        let headers = HashMap::new();
        let result = client.request("GET", "https://example.com", &headers, None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("WASI HTTP not yet implemented"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn get_returns_error() {
        let client = WasiHttpClient::new();
        let headers = HashMap::new();
        let result = client.get("https://example.com", &headers);
        assert!(result.is_err());
    }

    #[test]
    fn post_returns_error() {
        let client = WasiHttpClient::new();
        let headers = HashMap::new();
        let result = client.post("https://example.com", &headers, b"body");
        assert!(result.is_err());
    }

    #[test]
    fn wasi_http_client_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WasiHttpClient>();
    }

    // -- SandboxedHttpClient tests (wasm-plugins feature) --

    #[cfg(feature = "wasm-plugins")]
    mod sandboxed {
        use super::*;
        use crate::sandbox::PluginSandbox;
        use clawft_plugin::{PluginPermissions, PluginResourceConfig};
        use std::sync::Arc;

        fn sandbox_with_network(network: Vec<String>) -> Arc<PluginSandbox> {
            let permissions = PluginPermissions {
                network,
                ..Default::default()
            };
            Arc::new(PluginSandbox::from_manifest(
                "test-http-plugin".into(),
                permissions,
                &PluginResourceConfig::default(),
            ))
        }

        #[test]
        fn sandboxed_validate_allowed_domain() {
            let client = SandboxedHttpClient::new(
                sandbox_with_network(vec!["api.example.com".into()]),
            );
            let result = client.validate_request(
                "GET",
                "https://api.example.com/data",
                None,
            );
            assert!(result.is_ok());
        }

        #[test]
        fn sandboxed_validate_denied_domain() {
            let client = SandboxedHttpClient::new(
                sandbox_with_network(vec!["api.example.com".into()]),
            );
            let result = client.validate_request(
                "GET",
                "https://evil.example.com/data",
                None,
            );
            assert!(result.is_err());
        }

        #[test]
        fn sandboxed_validate_private_ip_blocked() {
            let client = SandboxedHttpClient::new(
                sandbox_with_network(vec!["*".into()]),
            );
            let result = client.validate_request(
                "GET",
                "http://127.0.0.1/",
                None,
            );
            assert!(result.is_err());
        }

        #[test]
        fn sandboxed_request_validates_then_returns_not_wired() {
            let client = SandboxedHttpClient::new(
                sandbox_with_network(vec!["api.example.com".into()]),
            );
            let headers = HashMap::new();
            let result = client.request("GET", "https://api.example.com/", &headers, None);
            // Should pass validation but return "not wired" error
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("not yet wired"), "expected 'not yet wired', got: {err}");
        }

        #[test]
        fn sandboxed_request_fails_validation_before_wiring() {
            let client = SandboxedHttpClient::new(
                sandbox_with_network(vec!["api.example.com".into()]),
            );
            let headers = HashMap::new();
            let result = client.request("GET", "https://evil.com/", &headers, None);
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            // Should fail at validation (host not allowed), not at "not wired"
            assert!(err.contains("not in network allowlist"), "expected allowlist error, got: {err}");
        }

        #[test]
        fn sandboxed_request_no_network_denied() {
            let client = SandboxedHttpClient::new(
                sandbox_with_network(vec![]),
            );
            let headers = HashMap::new();
            let result = client.request("GET", "https://api.example.com/", &headers, None);
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("not permitted"), "expected denied error, got: {err}");
        }
    }
}
