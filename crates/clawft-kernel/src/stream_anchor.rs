//! Stream-window chain anchor.
//!
//! Subscribes to configured kernel topics via the topic router, runs
//! a rolling BLAKE3 hash + message / byte counter over a fixed time
//! window, and on each window close appends a
//! `stream.window_commit` event to the local chain. This is the
//! control-plane audit anchor for stream data (audio frames, sensor
//! batches, etc.) — the data itself stays on the fast path, but the
//! chain records a tamper-evident summary.
//!
//! # Layout
//!
//! One [`StreamWindowAnchor`] per anchored topic, wired to the a2a
//! topic router via [`crate::topic::SubscriberSink::ExternalStream`].
//! The anchor's consumer task pulls serialized `KernelMessage` JSON
//! lines off the channel, updates its in-progress window, and fires
//! a chain event when the configured duration elapses.
//!
//! # Feature gating
//!
//! Only compiled with the `exochain` feature — without it there is
//! nothing to anchor to. Gracefully no-ops when the kernel lacks a
//! `ChainManager`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::a2a::A2ARouter;
use crate::chain::ChainManager;
use crate::topic::{SubscriberId, SubscriberSink};

/// Per-topic window accumulator state.
#[derive(Debug)]
struct Window {
    started_at_ms: u64,
    first_msg_ms: Option<u64>,
    last_msg_ms: Option<u64>,
    message_count: u64,
    byte_count: u64,
    hasher: blake3::Hasher,
}

impl Window {
    fn new(now_ms: u64) -> Self {
        Self {
            started_at_ms: now_ms,
            first_msg_ms: None,
            last_msg_ms: None,
            message_count: 0,
            byte_count: 0,
            hasher: blake3::Hasher::new(),
        }
    }

    fn record(&mut self, bytes: &[u8], now_ms: u64) {
        if self.first_msg_ms.is_none() {
            self.first_msg_ms = Some(now_ms);
        }
        self.last_msg_ms = Some(now_ms);
        self.message_count += 1;
        self.byte_count += bytes.len() as u64;
        self.hasher.update(bytes);
    }

    fn is_empty(&self) -> bool {
        self.message_count == 0
    }
}

/// Per-topic anchor handle; dropping it cancels the consumer task.
pub struct TopicAnchor {
    /// Topic name being anchored.
    pub topic: String,
    cancel: CancellationToken,
    subscriber_id: SubscriberId,
    router: Arc<A2ARouter>,
}

impl TopicAnchor {
    /// Cancel the consumer and unsubscribe from the router.
    pub fn shutdown(self) {
        self.cancel.cancel();
        self.router
            .topic_router()
            .unsubscribe_id(&self.topic, self.subscriber_id);
    }
}

/// Stream window anchor service.
///
/// Construct via [`StreamWindowAnchor::start_topic`]. One instance
/// per topic is cheapest, but the service itself holds no state —
/// the [`TopicAnchor`] handles carry all the lifetime.
pub struct StreamWindowAnchor;

impl StreamWindowAnchor {
    /// Start anchoring a single topic.
    ///
    /// Subscribes an [`SubscriberSink::ExternalStream`] sink to the
    /// topic router, then spawns a consumer task that fires a
    /// `stream.window_commit` chain event every `window` duration.
    ///
    /// Returns the [`TopicAnchor`] handle. Drop it or call
    /// [`TopicAnchor::shutdown`] to stop.
    pub fn start_topic(
        a2a: Arc<A2ARouter>,
        chain: Option<Arc<ChainManager>>,
        topic: String,
        window: Duration,
    ) -> TopicAnchor {
        let (tx, rx) = mpsc::channel::<Vec<u8>>(1024);
        let subscriber_id = a2a
            .topic_router()
            .subscribe_sink(&topic, SubscriberSink::ExternalStream(tx));

        let cancel = CancellationToken::new();
        let child = cancel.child_token();
        let topic_for_task = topic.clone();
        tokio::spawn(async move {
            run_consumer(topic_for_task, window, rx, chain, child).await;
        });

        info!(topic = %topic, sub_id = subscriber_id.0, window_ms = window.as_millis() as u64, "stream anchor started");

        TopicAnchor {
            topic,
            cancel,
            subscriber_id,
            router: a2a,
        }
    }
}

async fn run_consumer(
    topic: String,
    window: Duration,
    mut rx: mpsc::Receiver<Vec<u8>>,
    chain: Option<Arc<ChainManager>>,
    cancel: CancellationToken,
) {
    let started = Instant::now();
    let now_ms = || chrono::Utc::now().timestamp_millis().max(0) as u64;
    let mut current = Window::new(now_ms());
    let mut timer = tokio::time::interval_at(tokio::time::Instant::now() + window, window);
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                flush_window(&topic, &mut current, chain.as_deref(), now_ms());
                debug!(topic = %topic, "stream anchor cancelled");
                return;
            }
            msg = rx.recv() => {
                match msg {
                    Some(bytes) => current.record(&bytes, now_ms()),
                    None => {
                        // sender side dropped — router went away.
                        flush_window(&topic, &mut current, chain.as_deref(), now_ms());
                        debug!(topic = %topic, "stream anchor: subscriber channel closed");
                        return;
                    }
                }
            }
            _ = timer.tick() => {
                let now = now_ms();
                flush_window(&topic, &mut current, chain.as_deref(), now);
                // Start the next window aligned on the tick.
                current = Window::new(now);
                // Stall-guard: if we've been running for hours, log it — otherwise quiet.
                if started.elapsed() > Duration::from_secs(3600) {
                    debug!(topic = %topic, "stream anchor: hourly mark");
                }
            }
        }
    }
}

fn flush_window(topic: &str, window: &mut Window, chain: Option<&ChainManager>, now_ms: u64) {
    if window.is_empty() {
        return;
    }
    let hash = window.hasher.finalize();
    let hash_hex = hex_encode(hash.as_bytes());
    let payload = build_commit_payload(topic, window, now_ms, &hash_hex);

    if let Some(cm) = chain {
        cm.append("stream", "stream.window_commit", Some(payload.clone()));
        debug!(
            topic,
            samples = window.message_count,
            bytes = window.byte_count,
            hash = %hash_hex,
            "stream.window_commit appended"
        );
    } else {
        warn!(
            topic,
            samples = window.message_count,
            "chain manager unavailable; window_commit dropped"
        );
    }
}

fn build_commit_payload(topic: &str, window: &Window, window_end_ms: u64, hash_hex: &str) -> Value {
    serde_json::json!({
        "topic": topic,
        "window_start_ms": window.started_at_ms,
        "window_end_ms": window_end_ms,
        "first_msg_ms": window.first_msg_ms,
        "last_msg_ms": window.last_msg_ms,
        "sample_count": window.message_count,
        "byte_count": window.byte_count,
        "chunk_count": window.message_count,
        "blake3": hash_hex,
    })
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Glob-match a topic name against a configured pattern.
///
/// Supports the same trivial pattern vocabulary as the substrate
/// `TopicDecl`: exact match, or a trailing `.*` / `*` wildcard
/// matching any suffix. Returns true on match.
pub fn topic_matches(pattern: &str, topic: &str) -> bool {
    if pattern == topic {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("*") {
        return topic.starts_with(prefix);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_match_exact() {
        assert!(topic_matches("sensor.mic", "sensor.mic"));
        assert!(!topic_matches("sensor.mic", "sensor.cam"));
    }

    #[test]
    fn topic_match_trailing_wildcard() {
        assert!(topic_matches("sensor.*", "sensor.mic"));
        assert!(topic_matches("sensor.*", "sensor.cam.depth"));
        assert!(!topic_matches("sensor.*", "mic.left"));
    }

    #[test]
    fn window_builds_hash_and_counts() {
        let mut w = Window::new(10);
        w.record(b"{\"a\":1}\n", 12);
        w.record(b"{\"a\":2}\n", 14);
        assert_eq!(w.message_count, 2);
        assert_eq!(w.byte_count, 16);
        let h = w.hasher.finalize();
        // The hash is non-zero and reproducible.
        let again = {
            let mut h2 = blake3::Hasher::new();
            h2.update(b"{\"a\":1}\n");
            h2.update(b"{\"a\":2}\n");
            h2.finalize()
        };
        assert_eq!(h.as_bytes(), again.as_bytes());
    }
}
