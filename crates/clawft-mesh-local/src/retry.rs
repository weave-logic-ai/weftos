//! Reconnect support: jittered backoff and a retrying connect.

use std::time::Duration;

use ed25519_dalek::SigningKey;

use crate::client::{ClientConfig, ClientError, MeshLocalClient, RegisterParams};

/// Full-jitter exponential backoff for reconnects.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
}

impl Backoff {
    pub fn new(base: Duration, max: Duration) -> Self {
        Self { base, max, attempt: 0 }
    }

    /// Next delay: uniformly between half and the full exponential step.
    pub fn next_delay(&mut self) -> Duration {
        let step = self.base.saturating_mul(1u32 << self.attempt.min(16)).min(self.max);
        self.attempt = self.attempt.saturating_add(1);
        let lo = step / 2;
        let span = (step - lo).as_millis().max(1) as u64;
        lo + Duration::from_millis(rand::random::<u64>() % span)
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

/// Connect and register, retrying transport failures with backoff.
/// Verification and protocol failures return immediately.
pub async fn connect_with_retry(
    cfg: &ClientConfig,
    user_key: &SigningKey,
    params: &RegisterParams,
    mut backoff: Backoff,
    max_attempts: u32,
) -> Result<MeshLocalClient, ClientError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match MeshLocalClient::connect_and_register(cfg, user_key, params).await {
            Ok(c) => return Ok(c),
            Err(e) if e.is_retryable() && attempt < max_attempts => {
                tokio::time::sleep(backoff.next_delay()).await;
            }
            Err(e) => return Err(e),
        }
    }
}
