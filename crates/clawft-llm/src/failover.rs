//! Provider failover: try the next provider when all retries are exhausted.
//!
//! [`FailoverChain`] takes a list of providers (each typically wrapped in
//! [`RetryPolicy`](crate::retry::RetryPolicy)) and calls them in order.
//! If one provider fails with a retryable error after exhausting its retries,
//! the chain moves to the next provider. Non-retryable errors (auth failure,
//! model not found) are returned immediately.

use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::warn;

use crate::error::{ProviderError, Result};
use crate::provider::Provider;
use crate::retry::is_retryable;
use crate::types::{ChatRequest, ChatResponse, StreamChunk};

/// A chain of providers that fails over to the next on transient errors.
///
/// # Example
///
/// ```rust,ignore
/// use clawft_llm::failover::FailoverChain;
/// use clawft_llm::retry::{RetryPolicy, RetryConfig};
/// use clawft_llm::OpenAiCompatProvider;
///
/// let providers: Vec<Box<dyn Provider>> = vec![
///     Box::new(RetryPolicy::new(primary, RetryConfig::default())),
///     Box::new(RetryPolicy::new(fallback, RetryConfig::default())),
/// ];
/// let chain = FailoverChain::new(providers);
/// let response = chain.complete(&request).await?;
/// ```
pub struct FailoverChain {
    providers: Vec<Box<dyn Provider>>,
}

impl FailoverChain {
    /// Create a failover chain from an ordered list of providers.
    ///
    /// The first provider is the primary; subsequent providers are fallbacks.
    /// Returns `None` if the list is empty.
    pub fn new(providers: Vec<Box<dyn Provider>>) -> Option<Self> {
        if providers.is_empty() {
            return None;
        }
        Some(Self { providers })
    }

    /// Returns the number of providers in the chain.
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Returns `true` if the chain has no providers.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Returns the names of all providers in the chain, in order.
    pub fn provider_names(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.name()).collect()
    }
}

#[async_trait]
impl Provider for FailoverChain {
    fn name(&self) -> &str {
        // Report the primary provider's name
        self.providers
            .first()
            .map(|p| p.name())
            .unwrap_or("failover-chain")
    }

    async fn complete(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let mut errors: Vec<(String, ProviderError)> = Vec::new();

        for (idx, provider) in self.providers.iter().enumerate() {
            match provider.complete(request).await {
                Ok(response) => return Ok(response),
                Err(err) => {
                    let provider_name = provider.name().to_owned();

                    // If the error is not retryable, don't try fallback providers
                    // for certain error classes
                    if !is_retryable(&err) && !is_failover_eligible(&err) {
                        return Err(err);
                    }

                    warn!(
                        provider = %provider_name,
                        provider_index = idx,
                        total_providers = self.providers.len(),
                        error = %err,
                        "provider failed, trying next in failover chain"
                    );

                    errors.push((provider_name, err));
                }
            }
        }

        // All providers exhausted
        let summary: Vec<String> = errors
            .iter()
            .map(|(name, err)| format!("{name}: {err}"))
            .collect();

        Err(ProviderError::AllProvidersExhausted { attempts: summary })
    }

    async fn complete_stream(
        &self,
        request: &ChatRequest,
        tx: mpsc::Sender<StreamChunk>,
    ) -> Result<()> {
        let mut errors: Vec<(String, ProviderError)> = Vec::new();

        for (idx, provider) in self.providers.iter().enumerate() {
            // Buffer chunks per attempt to prevent partial output concatenation.
            // Only forward buffered chunks to the real sender on success.
            let (attempt_tx, mut attempt_rx) = mpsc::channel::<StreamChunk>(256);

            match provider.complete_stream(request, attempt_tx).await {
                Ok(()) => {
                    // Success: forward all buffered chunks to the real sender.
                    while let Some(chunk) = attempt_rx.recv().await {
                        if tx.send(chunk).await.is_err() {
                            break;
                        }
                    }
                    return Ok(());
                }
                Err(err) => {
                    let provider_name = provider.name().to_owned();

                    // Discard buffered chunks from this failed attempt by
                    // dropping attempt_rx (already moved out of scope).

                    if !is_retryable(&err) && !is_failover_eligible(&err) {
                        return Err(err);
                    }

                    warn!(
                        provider = %provider_name,
                        provider_index = idx,
                        total_providers = self.providers.len(),
                        error = %err,
                        "provider streaming failed, trying next in failover chain"
                    );

                    errors.push((provider_name, err));
                }
            }
        }

        let summary: Vec<String> = errors
            .iter()
            .map(|(name, err)| format!("{name}: {err}"))
            .collect();

        Err(ProviderError::AllProvidersExhausted { attempts: summary })
    }
}

/// Determines whether a non-retryable error should still trigger failover
/// to the next provider (as opposed to immediately returning the error).
///
/// For example, if provider A is not configured but provider B is, we should
/// try provider B. Similarly, if provider A has exhausted credits/billing
/// but provider B is a free tier, we should try provider B.
fn is_failover_eligible(err: &ProviderError) -> bool {
    matches!(
        err,
        ProviderError::NotConfigured(_)
            | ProviderError::ModelNotFound(_)
            | ProviderError::RequestFailed(_)
            | ProviderError::InvalidResponse(_)
    )
}

impl std::fmt::Debug for FailoverChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FailoverChain")
            .field("providers", &self.provider_names())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatMessage, ChatResponse, Choice, Usage};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn test_request() -> ChatRequest {
        ChatRequest::new("test-model", vec![ChatMessage::user("Hi")])
    }

    fn success_response(model: &str) -> ChatResponse {
        ChatResponse {
            id: "resp-1".into(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage::assistant(format!("Hello from {model}")),
                finish_reason: Some("stop".into()),
            }],
            usage: Some(Usage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
            }),
            model: model.into(),
        }
    }

    struct SuccessProvider {
        name: String,
    }

    #[async_trait]
    impl Provider for SuccessProvider {
        fn name(&self) -> &str {
            &self.name
        }
        async fn complete(&self, _req: &ChatRequest) -> Result<ChatResponse> {
            Ok(success_response(&self.name))
        }
    }

    struct FailProvider {
        name: String,
        error: fn() -> ProviderError,
    }

    #[async_trait]
    impl Provider for FailProvider {
        fn name(&self) -> &str {
            &self.name
        }
        async fn complete(&self, _req: &ChatRequest) -> Result<ChatResponse> {
            Err((self.error)())
        }
    }

    #[test]
    fn new_empty_returns_none() {
        assert!(FailoverChain::new(vec![]).is_none());
    }

    #[test]
    fn len_and_names() {
        let chain = FailoverChain::new(vec![
            Box::new(SuccessProvider { name: "a".into() }),
            Box::new(SuccessProvider { name: "b".into() }),
        ])
        .unwrap();
        assert_eq!(chain.len(), 2);
        assert!(!chain.is_empty());
        assert_eq!(chain.provider_names(), vec!["a", "b"]);
    }

    #[test]
    fn name_returns_primary() {
        let chain = FailoverChain::new(vec![
            Box::new(SuccessProvider {
                name: "primary".into(),
            }),
            Box::new(SuccessProvider {
                name: "fallback".into(),
            }),
        ])
        .unwrap();
        assert_eq!(chain.name(), "primary");
    }

    #[tokio::test]
    async fn primary_succeeds() {
        let chain = FailoverChain::new(vec![
            Box::new(SuccessProvider {
                name: "primary".into(),
            }),
            Box::new(SuccessProvider {
                name: "fallback".into(),
            }),
        ])
        .unwrap();

        let resp = chain.complete(&test_request()).await.unwrap();
        assert!(
            resp.choices[0]
                .message
                .content
                .as_deref()
                .unwrap()
                .contains("primary")
        );
    }

    #[tokio::test]
    async fn failover_to_second_on_retryable_error() {
        let chain = FailoverChain::new(vec![
            Box::new(FailProvider {
                name: "broken".into(),
                error: || ProviderError::ServerError {
                    status: 503,
                    body: "unavailable".into(),
                },
            }),
            Box::new(SuccessProvider {
                name: "backup".into(),
            }),
        ])
        .unwrap();

        let resp = chain.complete(&test_request()).await.unwrap();
        assert!(
            resp.choices[0]
                .message
                .content
                .as_deref()
                .unwrap()
                .contains("backup")
        );
    }

    #[tokio::test]
    async fn failover_on_not_configured() {
        let chain = FailoverChain::new(vec![
            Box::new(FailProvider {
                name: "unconfigured".into(),
                error: || ProviderError::NotConfigured("no API key".into()),
            }),
            Box::new(SuccessProvider {
                name: "configured".into(),
            }),
        ])
        .unwrap();

        let resp = chain.complete(&test_request()).await.unwrap();
        assert!(
            resp.choices[0]
                .message
                .content
                .as_deref()
                .unwrap()
                .contains("configured")
        );
    }

    #[tokio::test]
    async fn no_failover_on_auth_error() {
        let call_count = Arc::new(AtomicU32::new(0));
        let count = call_count.clone();

        struct CountingSuccess {
            count: Arc<AtomicU32>,
        }
        #[async_trait]
        impl Provider for CountingSuccess {
            fn name(&self) -> &str {
                "counter"
            }
            async fn complete(&self, _req: &ChatRequest) -> Result<ChatResponse> {
                self.count.fetch_add(1, Ordering::SeqCst);
                Ok(success_response("counter"))
            }
        }

        let chain = FailoverChain::new(vec![
            Box::new(FailProvider {
                name: "authed".into(),
                error: || ProviderError::AuthFailed("bad key".into()),
            }),
            Box::new(CountingSuccess { count }),
        ])
        .unwrap();

        let err = chain.complete(&test_request()).await.unwrap_err();
        assert!(matches!(err, ProviderError::AuthFailed(_)));
        // The second provider should NOT have been called
        assert_eq!(call_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn all_providers_exhausted() {
        let chain = FailoverChain::new(vec![
            Box::new(FailProvider {
                name: "p1".into(),
                error: || ProviderError::ServerError {
                    status: 500,
                    body: "error".into(),
                },
            }),
            Box::new(FailProvider {
                name: "p2".into(),
                error: || ProviderError::Timeout,
            }),
        ])
        .unwrap();

        let err = chain.complete(&test_request()).await.unwrap_err();
        match err {
            ProviderError::AllProvidersExhausted { attempts } => {
                assert_eq!(attempts.len(), 2);
                assert!(attempts[0].contains("p1"));
                assert!(attempts[1].contains("p2"));
            }
            other => panic!("expected AllProvidersExhausted, got: {other}"),
        }
    }

    #[tokio::test]
    async fn failover_through_three_providers() {
        let chain = FailoverChain::new(vec![
            Box::new(FailProvider {
                name: "p1".into(),
                error: || ProviderError::ServerError {
                    status: 502,
                    body: "bad gateway".into(),
                },
            }),
            Box::new(FailProvider {
                name: "p2".into(),
                error: || ProviderError::NotConfigured("no key".into()),
            }),
            Box::new(SuccessProvider { name: "p3".into() }),
        ])
        .unwrap();

        let resp = chain.complete(&test_request()).await.unwrap();
        assert!(
            resp.choices[0]
                .message
                .content
                .as_deref()
                .unwrap()
                .contains("p3")
        );
    }

    #[test]
    fn is_failover_eligible_not_configured() {
        assert!(super::is_failover_eligible(&ProviderError::NotConfigured(
            "missing".into()
        )));
    }

    #[test]
    fn is_failover_eligible_model_not_found() {
        assert!(super::is_failover_eligible(&ProviderError::ModelNotFound(
            "gpt-5".into()
        )));
    }

    #[test]
    fn is_failover_eligible_request_failed() {
        assert!(super::is_failover_eligible(&ProviderError::RequestFailed(
            "credits exhausted".into()
        )));
    }

    #[test]
    fn is_failover_eligible_invalid_response() {
        assert!(super::is_failover_eligible(
            &ProviderError::InvalidResponse("bad json".into())
        ));
    }

    #[tokio::test]
    async fn failover_on_billing_error() {
        let chain = FailoverChain::new(vec![
            Box::new(FailProvider {
                name: "paid".into(),
                error: || ProviderError::RequestFailed("credits exhausted".into()),
            }),
            Box::new(SuccessProvider {
                name: "free".into(),
            }),
        ])
        .unwrap();

        let resp = chain.complete(&test_request()).await.unwrap();
        assert!(
            resp.choices[0]
                .message
                .content
                .as_deref()
                .unwrap()
                .contains("free")
        );
    }

    #[test]
    fn is_not_failover_eligible_auth() {
        assert!(!super::is_failover_eligible(&ProviderError::AuthFailed(
            "bad key".into()
        )));
    }
}
