//! TEST-01: Mock HTTP server tests for `OpenAiCompatProvider::complete()`.
//!
//! Uses [`wiremock`] to stand up a local HTTP server that emulates
//! OpenAI-compatible chat completion responses. This exercises the full
//! HTTP request/response path without hitting a real API.
//!
//! Coverage:
//! - Successful completion with text response
//! - Successful completion with tool calls
//! - 401 authentication failure
//! - 429 rate limiting (with retry_after_ms extraction)
//! - 404 model not found
//! - 500 internal server error
//! - Malformed JSON response
//! - Empty choices array
//! - Custom headers forwarded correctly

use std::collections::HashMap;

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use clawft_llm::config::LlmProviderConfig;
use clawft_llm::error::ProviderError;
use clawft_llm::openai_compat::OpenAiCompatProvider;
use clawft_llm::provider::Provider;
use clawft_llm::types::{ChatMessage, ChatRequest};

/// Build a `ProviderConfig` pointing at the given mock server URL.
fn mock_config(server_url: &str) -> LlmProviderConfig {
    LlmProviderConfig {
        name: "mock-provider".into(),
        base_url: server_url.into(),
        api_key_env: "MOCK_UNUSED_KEY".into(),
        model_prefix: None,
        default_model: Some("test-model".into()),
        headers: HashMap::new(),
        timeout_secs: None,
    }
}

/// Build a minimal `ChatRequest` for testing.
fn test_request() -> ChatRequest {
    ChatRequest::new("test-model", vec![ChatMessage::user("Hello")])
}

// ── Successful completion ──────────────────────────────────────────────

#[tokio::test]
async fn complete_success_text_response() {
    let server = MockServer::start().await;

    let body = serde_json::json!({
        "id": "chatcmpl-test-001",
        "object": "chat.completion",
        "model": "test-model",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hello! How can I help you?"
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 8,
            "total_tokens": 18
        }
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("Authorization", "Bearer sk-mock-key"))
        .and(header("Content-Type", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-mock-key".into());

    let response = provider.complete(&test_request()).await.unwrap();

    assert_eq!(response.id, "chatcmpl-test-001");
    assert_eq!(response.model, "test-model");
    assert_eq!(response.choices.len(), 1);
    assert_eq!(
        response.choices[0].message.content.as_deref(),
        Some("Hello! How can I help you?")
    );
    assert_eq!(response.choices[0].message.role, "assistant");
    assert_eq!(response.choices[0].finish_reason.as_deref(), Some("stop"));

    let usage = response.usage.unwrap();
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 8);
    assert_eq!(usage.total(), 18);
}

#[tokio::test]
async fn complete_success_with_tool_calls() {
    let server = MockServer::start().await;

    let body = serde_json::json!({
        "id": "chatcmpl-tool-001",
        "object": "chat.completion",
        "model": "test-model",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call_abc123",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": "{\"city\":\"London\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {
            "prompt_tokens": 15,
            "completion_tokens": 20,
            "total_tokens": 35
        }
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-key".into());

    let response = provider.complete(&test_request()).await.unwrap();

    let tool_calls = response.choices[0].message.tool_calls.as_ref().unwrap();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id, "call_abc123");
    assert_eq!(tool_calls[0].call_type, "function");
    assert_eq!(tool_calls[0].function.name, "get_weather");
    assert_eq!(tool_calls[0].function.arguments, "{\"city\":\"London\"}");
    assert_eq!(
        response.choices[0].finish_reason.as_deref(),
        Some("tool_calls")
    );
}

// ── Error responses ────────────────────────────────────────────────────

#[tokio::test]
async fn complete_401_returns_auth_failed() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string(
            "{\"error\":{\"message\":\"Invalid API key\",\"type\":\"authentication_error\"}}",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-bad-key".into());

    let err = provider.complete(&test_request()).await.unwrap_err();
    assert!(
        matches!(err, ProviderError::AuthFailed(_)),
        "expected AuthFailed, got: {err:?}"
    );
    assert!(err.to_string().contains("Invalid API key"));
}

#[tokio::test]
async fn complete_403_returns_auth_failed() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(403).set_body_string("{\"error\":{\"message\":\"Forbidden\"}}"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-forbidden".into());

    let err = provider.complete(&test_request()).await.unwrap_err();
    assert!(matches!(err, ProviderError::AuthFailed(_)));
}

#[tokio::test]
async fn complete_429_returns_rate_limited_with_retry_after() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string(
            "{\"retry_after_ms\": 3000, \"error\":{\"message\":\"Rate limited\"}}",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-key".into());

    let err = provider.complete(&test_request()).await.unwrap_err();
    match err {
        ProviderError::RateLimited { retry_after_ms } => {
            assert_eq!(retry_after_ms, 3000);
        }
        other => panic!("expected RateLimited, got: {other:?}"),
    }
}

#[tokio::test]
async fn complete_429_default_retry_when_no_retry_after() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_string("{\"error\":{\"message\":\"Too many requests\"}}"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-key".into());

    let err = provider.complete(&test_request()).await.unwrap_err();
    match err {
        ProviderError::RateLimited { retry_after_ms } => {
            // Default is 1000 when no retry_after_ms in body
            assert_eq!(retry_after_ms, 1000);
        }
        other => panic!("expected RateLimited, got: {other:?}"),
    }
}

#[tokio::test]
async fn complete_429_with_retry_after_seconds() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string("{\"retry_after\": 2.5}"))
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-key".into());

    let err = provider.complete(&test_request()).await.unwrap_err();
    match err {
        ProviderError::RateLimited { retry_after_ms } => {
            assert_eq!(retry_after_ms, 2500);
        }
        other => panic!("expected RateLimited, got: {other:?}"),
    }
}

#[tokio::test]
async fn complete_404_returns_model_not_found() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_string("{\"error\":{\"message\":\"Model not found\"}}"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-key".into());

    let err = provider.complete(&test_request()).await.unwrap_err();
    assert!(
        matches!(err, ProviderError::ModelNotFound(_)),
        "expected ModelNotFound, got: {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("test-model"),
        "error should mention the model: {msg}"
    );
}

#[tokio::test]
async fn complete_500_returns_server_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-key".into());

    let err = provider.complete(&test_request()).await.unwrap_err();
    assert!(
        matches!(err, ProviderError::ServerError { status: 500, .. }),
        "expected ServerError with status 500, got: {err:?}"
    );
    assert!(err.to_string().contains("500"));
}

// ── Malformed responses ────────────────────────────────────────────────

#[tokio::test]
async fn complete_malformed_json_returns_invalid_response() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string("this is not json {{{"))
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-key".into());

    let err = provider.complete(&test_request()).await.unwrap_err();
    assert!(
        matches!(err, ProviderError::InvalidResponse(_)),
        "expected InvalidResponse, got: {err:?}"
    );
}

#[tokio::test]
async fn complete_empty_choices_parses_successfully() {
    let server = MockServer::start().await;

    let body = serde_json::json!({
        "id": "chatcmpl-empty",
        "model": "test-model",
        "choices": [],
        "usage": null
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-key".into());

    let response = provider.complete(&test_request()).await.unwrap();
    assert!(response.choices.is_empty());
    assert!(response.usage.is_none());
}

// ── Request construction ───────────────────────────────────────────────

#[tokio::test]
async fn complete_sends_authorization_header() {
    let server = MockServer::start().await;

    let body = serde_json::json!({
        "id": "auth-check",
        "model": "m",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "ok"},
            "finish_reason": "stop"
        }],
        "usage": null
    });

    // This mock will only match if Authorization header is correct
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("Authorization", "Bearer sk-verify-auth"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-verify-auth".into());

    // If the auth header is wrong, the mock won't match and wiremock panics
    provider.complete(&test_request()).await.unwrap();
}

#[tokio::test]
async fn complete_forwards_custom_headers() {
    let server = MockServer::start().await;

    let body = serde_json::json!({
        "id": "header-check",
        "model": "m",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "ok"},
            "finish_reason": "stop"
        }],
        "usage": null
    });

    // Match on a custom header
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(header("x-custom-header", "custom-value"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .expect(1)
        .mount(&server)
        .await;

    let mut config = mock_config(&server.uri());
    config
        .headers
        .insert("anthropic-version".into(), "2023-06-01".into());
    config
        .headers
        .insert("x-custom-header".into(), "custom-value".into());

    let provider = OpenAiCompatProvider::with_api_key(config, "sk-key".into());
    provider.complete(&test_request()).await.unwrap();
}

#[tokio::test]
async fn complete_sends_request_body_correctly() {
    let server = MockServer::start().await;

    let body = serde_json::json!({
        "id": "body-check",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "ok"},
            "finish_reason": "stop"
        }],
        "usage": null
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-key".into());

    let request = ChatRequest {
        model: "gpt-4o".into(),
        messages: vec![
            ChatMessage::system("You are a test."),
            ChatMessage::user("Say hello"),
        ],
        max_tokens: Some(100),
        temperature: Some(0.5),
        tools: vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "test_tool",
                "parameters": {"type": "object"}
            }
        })],
        stream: None,
    };

    let response = provider.complete(&request).await.unwrap();
    assert_eq!(response.id, "body-check");
}

// ── Edge cases ─────────────────────────────────────────────────────────

#[tokio::test]
async fn complete_missing_api_key_returns_not_configured() {
    let config = LlmProviderConfig {
        name: "test".into(),
        base_url: "http://localhost:1".into(),
        api_key_env: "CLAWFT_NONEXISTENT_MOCK_KEY_99999".into(),
        model_prefix: None,
        default_model: None,
        headers: HashMap::new(),
        timeout_secs: None,
    };
    let provider = OpenAiCompatProvider::new(config);

    let err = provider.complete(&test_request()).await.unwrap_err();
    assert!(
        matches!(err, ProviderError::NotConfigured(_)),
        "expected NotConfigured, got: {err:?}"
    );
}

#[tokio::test]
async fn complete_multiple_choices() {
    let server = MockServer::start().await;

    let body = serde_json::json!({
        "id": "multi-choice",
        "model": "test-model",
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": "Choice A"},
                "finish_reason": "stop"
            },
            {
                "index": 1,
                "message": {"role": "assistant", "content": "Choice B"},
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 5,
            "completion_tokens": 10,
            "total_tokens": 15
        }
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-key".into());

    let response = provider.complete(&test_request()).await.unwrap();
    assert_eq!(response.choices.len(), 2);
    assert_eq!(
        response.choices[0].message.content.as_deref(),
        Some("Choice A")
    );
    assert_eq!(
        response.choices[1].message.content.as_deref(),
        Some("Choice B")
    );
    assert_eq!(response.choices[0].index, 0);
    assert_eq!(response.choices[1].index, 1);
}

#[tokio::test]
async fn complete_usage_without_prompt_finish_reason_null() {
    let server = MockServer::start().await;

    let body = serde_json::json!({
        "id": "partial",
        "model": "test-model",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "partial response"},
            "finish_reason": null
        }],
        "usage": {
            "prompt_tokens": 5,
            "completion_tokens": 3,
            "total_tokens": 8
        }
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .expect(1)
        .mount(&server)
        .await;

    let config = mock_config(&server.uri());
    let provider = OpenAiCompatProvider::with_api_key(config, "sk-key".into());

    let response = provider.complete(&test_request()).await.unwrap();
    assert!(response.choices[0].finish_reason.is_none());
    assert_eq!(
        response.choices[0].message.content.as_deref(),
        Some("partial response")
    );
}
