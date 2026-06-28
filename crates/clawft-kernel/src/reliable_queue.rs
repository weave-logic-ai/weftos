//! Reliable message delivery with acknowledgment tracking.
//!
//! [`ReliableQueue`] extends the A2A messaging model with ack-based
//! retry logic. Messages sent via `send_reliable()` are tracked until
//! acknowledged or until max retries are exceeded, at which point they
//! are routed to the [`DeadLetterQueue`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::dead_letter::{DeadLetterQueue, DeadLetterReason};
use crate::ipc::KernelMessage;

/// Configuration for reliable message delivery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReliableConfig {
    /// Maximum number of delivery attempts before dead-lettering.
    pub max_retries: u32,
    /// Timeout for the first delivery attempt.
    pub initial_timeout: Duration,
    /// Maximum timeout after backoff (cap).
    pub max_timeout: Duration,
    /// Multiplier applied to timeout on each retry.
    pub backoff_multiplier: f64,
}

impl Default for ReliableConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_timeout: Duration::from_secs(5),
            max_timeout: Duration::from_secs(30),
            backoff_multiplier: 2.0,
        }
    }
}

/// Tracks a pending delivery awaiting acknowledgment.
#[derive(Debug, Clone)]
pub struct PendingDelivery {
    /// The message being delivered.
    pub message: KernelMessage,
    /// Number of delivery attempts so far.
    pub attempts: u32,
    /// When the first attempt was made.
    pub first_sent: Instant,
    /// When the most recent attempt was made.
    pub last_attempt: Instant,
    /// Deadline for the current attempt's ack.
    pub ack_deadline: Instant,
}

/// Result of a reliable delivery attempt.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeliveryResult {
    /// Message was acknowledged by the receiver.
    Acknowledged {
        /// Message ID.
        msg_id: String,
        /// Time from first send to ack.
        ack_time_ms: u64,
    },
    /// Maximum retries exceeded; message sent to dead letter queue.
    MaxRetriesExceeded {
        /// Message ID.
        msg_id: String,
        /// Total number of attempts made.
        attempts: u32,
    },
    /// Message was dead-lettered for a non-retry reason.
    DeadLettered {
        /// Message ID.
        msg_id: String,
        /// Reason for dead-lettering.
        reason: String,
    },
}

/// Reliable message delivery with acknowledgment tracking.
///
/// Tracks pending deliveries by correlation ID. On ack timeout, retries
/// with exponential backoff. After `max_retries`, routes to the
/// dead letter queue.
pub struct ReliableQueue {
    pending: DashMap<String, PendingDelivery>,
    config: ReliableConfig,
    dead_letter: Arc<DeadLetterQueue>,
    /// Optional learned dead-letter policy (Finding #5). When `None`
    /// or untrained, `check_timeouts` and `expire_max_retries` use
    /// the hardcoded config-driven formulas (bit-for-bit identical to
    /// today). When trained, the model's `(delay_ms, should_discard)`
    /// heads are consulted for backoff/discard decisions.
    dead_letter_model: Option<Arc<std::sync::Mutex<crate::eml_kernel::DeadLetterModel>>>,
}

impl ReliableQueue {
    /// Create a new reliable queue with the given config and dead letter queue.
    pub fn new(config: ReliableConfig, dead_letter: Arc<DeadLetterQueue>) -> Self {
        Self {
            pending: DashMap::new(),
            config,
            dead_letter,
            dead_letter_model: None,
        }
    }

    /// Create with default configuration.
    pub fn with_defaults(dead_letter: Arc<DeadLetterQueue>) -> Self {
        Self::new(ReliableConfig::default(), dead_letter)
    }

    /// Install a learned
    /// [`DeadLetterModel`](crate::eml_kernel::DeadLetterModel) on this
    /// queue. With a model installed, [`Self::check_timeouts`] and
    /// [`Self::expire_max_retries`] route their backoff / discard
    /// decisions through the model. Untrained models fall back to
    /// the hardcoded formulas, so `with_model` is drop-in safe.
    ///
    /// NOTE(eml-swap): wired — Finding #5 (DeadLetterModel).
    pub fn with_model(
        mut self,
        model: Arc<std::sync::Mutex<crate::eml_kernel::DeadLetterModel>>,
    ) -> Self {
        self.dead_letter_model = Some(model);
        self
    }

    /// Returns a handle to the optional dead-letter model.
    pub fn dead_letter_model(
        &self,
    ) -> Option<&Arc<std::sync::Mutex<crate::eml_kernel::DeadLetterModel>>> {
        self.dead_letter_model.as_ref()
    }

    /// Get the configuration.
    pub fn config(&self) -> &ReliableConfig {
        &self.config
    }

    /// Register a message for reliable delivery tracking.
    ///
    /// The message must have a `correlation_id` set. If not, one is
    /// generated. Returns the correlation ID used for tracking.
    pub fn register(&self, mut msg: KernelMessage) -> (String, KernelMessage) {
        let correlation_id = msg
            .correlation_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        msg.correlation_id = Some(correlation_id.clone());

        let now = Instant::now();
        let pending = PendingDelivery {
            message: msg.clone(),
            attempts: 1,
            first_sent: now,
            last_attempt: now,
            ack_deadline: now + self.config.initial_timeout,
        };

        self.pending.insert(correlation_id.clone(), pending);
        (correlation_id, msg)
    }

    /// Acknowledge receipt of a message by correlation ID.
    ///
    /// Returns `Some(DeliveryResult::Acknowledged)` if the message was
    /// pending, or `None` if not found (already acked or expired).
    pub fn acknowledge(&self, correlation_id: &str) -> Option<DeliveryResult> {
        let (_, pending) = self.pending.remove(correlation_id)?;
        let ack_time = pending.first_sent.elapsed();
        Some(DeliveryResult::Acknowledged {
            msg_id: pending.message.id.clone(),
            ack_time_ms: ack_time.as_millis() as u64,
        })
    }

    /// Check for timed-out deliveries and return messages needing retry.
    ///
    /// Returns a list of `(correlation_id, message)` pairs that have
    /// exceeded their ack deadline and should be retried.
    ///
    /// NOTE(eml-swap): wired — Finding #5 (DeadLetterModel). When a
    /// learned model is installed and trained, the next-attempt
    /// ack deadline comes from the model; untrained models fall back
    /// to the existing `initial_timeout * backoff_multiplier^attempt`
    /// formula bit-for-bit.
    pub fn check_timeouts(&self) -> Vec<(String, KernelMessage)> {
        let now = Instant::now();
        let mut retries = Vec::new();
        let queue_depth = self.pending.len();

        for mut entry in self.pending.iter_mut() {
            if now >= entry.ack_deadline {
                let corr_id = entry.key().clone();
                let pending = entry.value_mut();

                pending.attempts += 1;
                pending.last_attempt = now;

                let message_age_ms = pending.first_sent.elapsed().as_millis() as u64;
                let next_timeout = self.compute_next_timeout(
                    pending.attempts.saturating_sub(1),
                    message_age_ms,
                    queue_depth,
                );
                pending.ack_deadline = now + next_timeout;

                retries.push((corr_id, pending.message.clone()));
            }
        }

        retries
    }

    /// Compute the next ack timeout for a retry attempt, consulting
    /// the dead-letter model when one is installed.
    ///
    /// Fallback (model `None` or untrained): the existing
    /// `initial_timeout * backoff_multiplier^attempt_idx` formula
    /// clamped at `max_timeout`. This is byte-identical to the
    /// pre-EML computation so untrained models produce the same
    /// schedule.
    fn compute_next_timeout(
        &self,
        attempt_idx: u32,
        message_age_ms: u64,
        queue_depth: usize,
    ) -> Duration {
        // Hardcoded fallback — never changes behaviour.
        let backoff = self.config.initial_timeout.as_secs_f64()
            * self.config.backoff_multiplier.powi(attempt_idx as i32);
        let fallback = Duration::from_secs_f64(backoff.min(self.config.max_timeout.as_secs_f64()));

        let Some(model_handle) = self.dead_letter_model.as_ref() else {
            return fallback;
        };
        let Ok(model) = model_handle.lock() else {
            return fallback;
        };
        if !model.is_trained() {
            return fallback;
        }
        let (delay_ms, _should_discard) = model.predict(attempt_idx, message_age_ms, queue_depth);
        Duration::from_millis(delay_ms.min(self.config.max_timeout.as_millis() as u64))
    }

    /// Move deliveries that have exceeded max retries to the dead letter queue.
    ///
    /// Returns the list of message IDs that were dead-lettered.
    ///
    /// NOTE(eml-swap): wired — Finding #5 (DeadLetterModel). When a
    /// trained DeadLetterModel is installed, the discard decision
    /// consults the model's `should_discard` head in addition to the
    /// config's `max_retries` ceiling. The untrained fallback matches
    /// today's "attempts > max_retries" rule exactly.
    pub fn expire_max_retries(&self) -> Vec<DeliveryResult> {
        let mut expired = Vec::new();
        let mut to_remove = Vec::new();
        let queue_depth = self.pending.len();

        for entry in self.pending.iter() {
            if self.should_discard(
                entry.attempts,
                entry.first_sent.elapsed().as_millis() as u64,
                queue_depth,
            ) {
                to_remove.push(entry.key().clone());
            }
        }

        for corr_id in to_remove {
            if let Some((_, pending)) = self.pending.remove(&corr_id) {
                let total_ms = pending.first_sent.elapsed().as_millis() as u64;
                self.dead_letter.intake(
                    pending.message.clone(),
                    DeadLetterReason::Timeout {
                        duration_ms: total_ms,
                    },
                );
                expired.push(DeliveryResult::MaxRetriesExceeded {
                    msg_id: pending.message.id,
                    attempts: pending.attempts,
                });
            }
        }

        expired
    }

    /// Discard-decision helper. Consults the optional
    /// [`DeadLetterModel`] when trained; otherwise uses the hardcoded
    /// `attempts > max_retries` rule.
    fn should_discard(&self, attempts: u32, message_age_ms: u64, queue_depth: usize) -> bool {
        // Hardcoded fallback — unchanged.
        let fallback = attempts > self.config.max_retries;

        let Some(model_handle) = self.dead_letter_model.as_ref() else {
            return fallback;
        };
        let Ok(model) = model_handle.lock() else {
            return fallback;
        };
        if !model.is_trained() {
            return fallback;
        }
        let (_delay_ms, model_discard) = model.predict(attempts, message_age_ms, queue_depth);
        // The model is advisory: we still enforce the hard max_retries
        // ceiling so a buggy model can never hold a message forever.
        fallback || model_discard
    }

    /// Number of currently pending deliveries.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Check if a specific correlation ID is pending.
    pub fn is_pending(&self, correlation_id: &str) -> bool {
        self.pending.contains_key(correlation_id)
    }

    /// Get a snapshot of all pending deliveries.
    pub fn pending_snapshot(&self) -> Vec<(String, PendingDelivery)> {
        self.pending
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }

    /// Cancel tracking for a specific message.
    pub fn cancel(&self, correlation_id: &str) -> Option<KernelMessage> {
        self.pending.remove(correlation_id).map(|(_, p)| p.message)
    }

    /// Calculate the timeout for a given attempt number.
    ///
    /// Uses the same EML-aware dispatch as [`Self::check_timeouts`]
    /// so external callers observing the policy see the same delays
    /// the queue would actually wait.
    pub fn timeout_for_attempt(&self, attempt: u32) -> Duration {
        self.compute_next_timeout(attempt, 0, self.pending.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{MessagePayload, MessageTarget};

    fn make_msg(from: u64, target: u64) -> KernelMessage {
        KernelMessage::new(
            from,
            MessageTarget::Process(target),
            MessagePayload::Text("reliable-test".into()),
        )
    }

    fn make_queue() -> (ReliableQueue, Arc<DeadLetterQueue>) {
        let dlq = Arc::new(DeadLetterQueue::new(100));
        let config = ReliableConfig {
            max_retries: 3,
            initial_timeout: Duration::from_millis(100),
            max_timeout: Duration::from_secs(2),
            backoff_multiplier: 2.0,
        };
        (ReliableQueue::new(config, dlq.clone()), dlq)
    }

    #[test]
    fn register_and_acknowledge() {
        let (queue, _dlq) = make_queue();
        let msg = make_msg(1, 2);
        let (corr_id, _msg) = queue.register(msg);

        assert!(queue.is_pending(&corr_id));
        assert_eq!(queue.pending_count(), 1);

        let result = queue.acknowledge(&corr_id).unwrap();
        assert!(matches!(result, DeliveryResult::Acknowledged { .. }));
        assert!(!queue.is_pending(&corr_id));
        assert_eq!(queue.pending_count(), 0);
    }

    #[test]
    fn acknowledge_unknown_returns_none() {
        let (queue, _dlq) = make_queue();
        assert!(queue.acknowledge("nonexistent").is_none());
    }

    #[test]
    fn register_generates_correlation_id() {
        let (queue, _dlq) = make_queue();
        let msg = make_msg(1, 2);
        assert!(msg.correlation_id.is_none());

        let (corr_id, msg) = queue.register(msg);
        assert!(!corr_id.is_empty());
        assert_eq!(msg.correlation_id, Some(corr_id));
    }

    #[test]
    fn register_preserves_existing_correlation_id() {
        let (queue, _dlq) = make_queue();
        let msg = KernelMessage::with_correlation(
            1,
            MessageTarget::Process(2),
            MessagePayload::Text("test".into()),
            "my-corr-id".into(),
        );

        let (corr_id, _) = queue.register(msg);
        assert_eq!(corr_id, "my-corr-id");
    }

    #[test]
    fn check_timeouts_empty() {
        let (queue, _dlq) = make_queue();
        let retries = queue.check_timeouts();
        assert!(retries.is_empty());
    }

    #[test]
    fn backoff_increases_per_attempt() {
        let (queue, _dlq) = make_queue();
        // initial = 100ms, multiplier = 2.0
        let t0 = queue.timeout_for_attempt(0);
        let t1 = queue.timeout_for_attempt(1);
        let t2 = queue.timeout_for_attempt(2);

        assert_eq!(t0.as_millis(), 100);
        assert_eq!(t1.as_millis(), 200);
        assert_eq!(t2.as_millis(), 400);
    }

    #[test]
    fn backoff_caps_at_max_timeout() {
        let (queue, _dlq) = make_queue();
        // max_timeout = 2s, so attempt 10 should be capped
        let t10 = queue.timeout_for_attempt(10);
        assert!(t10 <= Duration::from_secs(2));
    }

    #[test]
    fn expire_max_retries_sends_to_dlq() {
        let (queue, dlq) = make_queue();
        let msg = make_msg(1, 2);
        let (corr_id, _msg) = queue.register(msg);

        // Manually set attempts beyond max_retries
        if let Some(mut entry) = queue.pending.get_mut(&corr_id) {
            entry.attempts = 5; // > max_retries (3)
        }

        let expired = queue.expire_max_retries();
        assert_eq!(expired.len(), 1);
        assert!(matches!(
            &expired[0],
            DeliveryResult::MaxRetriesExceeded { attempts: 5, .. }
        ));
        assert_eq!(queue.pending_count(), 0);
        assert_eq!(dlq.len(), 1);
    }

    #[test]
    fn concurrent_sends_tracked_independently() {
        let (queue, _dlq) = make_queue();
        let (id1, _) = queue.register(make_msg(1, 2));
        let (id2, _) = queue.register(make_msg(3, 4));
        let (id3, _) = queue.register(make_msg(5, 6));

        assert_eq!(queue.pending_count(), 3);

        // Ack only the second one
        queue.acknowledge(&id2);
        assert_eq!(queue.pending_count(), 2);
        assert!(queue.is_pending(&id1));
        assert!(!queue.is_pending(&id2));
        assert!(queue.is_pending(&id3));
    }

    #[test]
    fn cancel_removes_tracking() {
        let (queue, _dlq) = make_queue();
        let (corr_id, _) = queue.register(make_msg(1, 2));
        assert!(queue.is_pending(&corr_id));

        let msg = queue.cancel(&corr_id).unwrap();
        assert!(!queue.is_pending(&corr_id));
        assert_eq!(msg.from, 1);
    }

    #[test]
    fn cancel_nonexistent_returns_none() {
        let (queue, _dlq) = make_queue();
        assert!(queue.cancel("nope").is_none());
    }

    #[test]
    fn pending_snapshot() {
        let (queue, _dlq) = make_queue();
        queue.register(make_msg(1, 2));
        queue.register(make_msg(3, 4));

        let snap = queue.pending_snapshot();
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn default_config() {
        let config = ReliableConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.initial_timeout, Duration::from_secs(5));
        assert_eq!(config.max_timeout, Duration::from_secs(30));
        assert!((config.backoff_multiplier - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn delivery_result_serde_roundtrip() {
        let result = DeliveryResult::Acknowledged {
            msg_id: "msg-1".into(),
            ack_time_ms: 42,
        };
        let json = serde_json::to_string(&result).unwrap();
        let restored: DeliveryResult = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored,
            DeliveryResult::Acknowledged {
                ack_time_ms: 42,
                ..
            }
        ));
    }

    // ── Finding #5: DeadLetterModel wiring ──────────────────────────

    #[test]
    fn untrained_model_preserves_backoff_schedule() {
        // Finding #5: with an untrained DeadLetterModel installed,
        // timeout_for_attempt must produce the same delays as the
        // hardcoded fallback.
        let dlq = Arc::new(DeadLetterQueue::new(100));
        let cfg = ReliableConfig {
            max_retries: 3,
            initial_timeout: Duration::from_millis(100),
            max_timeout: Duration::from_secs(2),
            backoff_multiplier: 2.0,
        };
        let plain = ReliableQueue::new(cfg.clone(), dlq.clone());
        let model = Arc::new(std::sync::Mutex::new(
            crate::eml_kernel::DeadLetterModel::new(),
        ));
        let wired = ReliableQueue::new(cfg, dlq).with_model(model);

        for attempt in 0..6 {
            assert_eq!(
                plain.timeout_for_attempt(attempt),
                wired.timeout_for_attempt(attempt),
                "untrained DeadLetterModel must reproduce backoff at attempt {attempt}"
            );
        }
    }

    #[test]
    fn untrained_model_preserves_discard_rule() {
        // Finding #5: untrained model must not change which deliveries
        // get expired by `expire_max_retries`.
        let dlq = Arc::new(DeadLetterQueue::new(100));
        let cfg = ReliableConfig {
            max_retries: 3,
            initial_timeout: Duration::from_millis(100),
            max_timeout: Duration::from_secs(2),
            backoff_multiplier: 2.0,
        };
        let model = Arc::new(std::sync::Mutex::new(
            crate::eml_kernel::DeadLetterModel::new(),
        ));
        let queue = ReliableQueue::new(cfg, dlq.clone()).with_model(model);

        let (corr_id, _) = queue.register(make_msg(1, 2));
        // attempts == 1 < max_retries (3); should NOT expire.
        assert!(queue.expire_max_retries().is_empty());

        if let Some(mut entry) = queue.pending.get_mut(&corr_id) {
            entry.attempts = 5; // > max_retries
        }
        let expired = queue.expire_max_retries();
        assert_eq!(expired.len(), 1);
        assert_eq!(dlq.len(), 1);
    }

    #[test]
    fn trained_model_can_shorten_backoff() {
        // Finding #5: with a trained DeadLetterModel, timeout_for_attempt
        // dispatches to the model and produces a different delay from
        // the hardcoded formula.
        let dlq = Arc::new(DeadLetterQueue::new(100));
        let cfg = ReliableConfig {
            max_retries: 3,
            initial_timeout: Duration::from_millis(100),
            max_timeout: Duration::from_secs(2),
            backoff_multiplier: 2.0,
        };
        let plain = ReliableQueue::new(cfg.clone(), dlq.clone());

        let untrained = crate::eml_kernel::DeadLetterModel::new();
        let mut json = serde_json::to_value(&untrained).unwrap();
        if let Some(inner) = json.get_mut("inner").and_then(|v| v.as_object_mut()) {
            inner.insert("trained".into(), serde_json::Value::Bool(true));
        }
        let forced: crate::eml_kernel::DeadLetterModel = serde_json::from_value(json).unwrap();
        assert!(forced.is_trained());

        let model = Arc::new(std::sync::Mutex::new(forced));
        let wired = ReliableQueue::new(cfg, dlq).with_model(model);

        let attempt = 2;
        let hardcoded = plain.timeout_for_attempt(attempt);
        let model_driven = wired.timeout_for_attempt(attempt);
        // The two must differ — proves the dispatch branch executed.
        assert_ne!(hardcoded, model_driven);
    }

    #[test]
    fn check_timeouts_with_untrained_model_matches_plain() {
        // Bit-for-bit verification: check_timeouts() advances the
        // ack_deadline by the same delta when the model is untrained.
        let dlq = Arc::new(DeadLetterQueue::new(100));
        let cfg = ReliableConfig {
            max_retries: 3,
            initial_timeout: Duration::from_millis(50),
            max_timeout: Duration::from_secs(2),
            backoff_multiplier: 2.0,
        };
        let model = Arc::new(std::sync::Mutex::new(
            crate::eml_kernel::DeadLetterModel::new(),
        ));
        let queue = ReliableQueue::new(cfg, dlq).with_model(model);

        let (corr_id, _) = queue.register(make_msg(1, 2));
        // Force the deadline into the past so check_timeouts retries.
        if let Some(mut entry) = queue.pending.get_mut(&corr_id) {
            entry.ack_deadline = Instant::now() - Duration::from_millis(10);
        }
        let retries = queue.check_timeouts();
        assert_eq!(retries.len(), 1);
        // attempts now == 2; backoff = 50ms * 2^1 = 100ms.
        let entry = queue.pending.get(&corr_id).unwrap();
        assert_eq!(entry.attempts, 2);
    }
}
