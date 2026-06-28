//! Voice command audit logging (WEFT-210 / SC-9).
//!
//! When the whisper service emits a transcript, we want a tamper-
//! evident record of *what was transcribed, by whom, on what model,
//! when* — without ever putting the raw audio or the user-visible
//! transcript text on a permanent log. The audit row is what an
//! operator pulls when investigating "did the agent really act on a
//! voice command at 02:14, and what level of permission did the
//! command get?".
//!
//! # Shape of an audit row
//!
//! Each transcript fires exactly one [`TranscriptAuditEvent`] before
//! the transcript is published to the substrate. The event carries:
//!
//! - `transcript_id` — sortable id (here: the substrate output
//!   `tick`, which is a monotonic kernel-issued sequence) to correlate
//!   with downstream consumers.
//! - `source_node` — the sensor node id that produced the PCM
//!   (extracted from the configured input/output paths).
//! - `model_id` — which model produced the transcript. Sourced from
//!   the verified manifest (see `manifest.rs`) when present, else a
//!   constant fallback so the audit row is never empty.
//! - `principal_inferred` — best-effort principal id, from the
//!   actor/node-id the service runs as. The agent loop reconciles
//!   this against its session-principal table when scoring permission
//!   levels.
//! - `transcript_text_hash` — SHA-256 of the transcript text. We do
//!   NOT log the text itself; SC-9 P0 control "Text only, not audio"
//!   is met by hashing here.
//! - `ts_unix_micros` — wall-clock μs of the audit event for
//!   timeline reconstruction.
//!
//! # Sink
//!
//! Events are emitted via `tracing::info!(target = AUDIT_TARGET, ...)`
//! with structured fields so the chain-event layer (which already
//! subscribes to `tracing` targets) picks them up unchanged. There
//! is intentionally no separate log file: a single audit pipeline
//! across the daemon is easier to ship-of-Theseus through retention
//! / rotation policies.

use sha2::{Digest, Sha256};

/// Tracing target string for voice audit events. Any subscriber that
/// filters on `target == "voice.audit"` (e.g. the chain-event layer)
/// will see exactly the audit rows.
pub const AUDIT_TARGET: &str = "voice.audit";

/// Structured shape of a single transcript audit row.
///
/// Constructed by the whisper service immediately after a successful
/// inference, just before the transcript is published. The
/// [`Self::emit`] helper writes the row to the configured tracing
/// target.
#[derive(Debug, Clone)]
pub struct TranscriptAuditEvent {
    /// Monotonic id correlating this audit row with the published
    /// transcript. We use the substrate `tick` returned by the
    /// publish call.
    pub transcript_id: u64,
    /// Sensor node id that produced the PCM.
    pub source_node: String,
    /// Model id (from the signed manifest, when present).
    pub model_id: String,
    /// Best-effort principal id of the actor running the service.
    pub principal_inferred: String,
    /// Lower-hex SHA-256 of the transcript text. Not the text itself.
    pub transcript_text_hash: String,
    /// Wall-clock microseconds since the Unix epoch.
    pub ts_unix_micros: i128,
}

impl TranscriptAuditEvent {
    /// Build an audit event for a freshly-produced transcript.
    pub fn new(
        transcript_id: u64,
        source_node: impl Into<String>,
        model_id: impl Into<String>,
        principal_inferred: impl Into<String>,
        transcript_text: &str,
    ) -> Self {
        Self {
            transcript_id,
            source_node: source_node.into(),
            model_id: model_id.into(),
            principal_inferred: principal_inferred.into(),
            transcript_text_hash: hash_transcript(transcript_text),
            ts_unix_micros: now_unix_micros(),
        }
    }

    /// Emit this event via the `voice.audit` tracing target. Any
    /// downstream subscriber filtering on the target picks it up.
    pub fn emit(&self) {
        tracing::info!(
            target: AUDIT_TARGET,
            transcript_id = self.transcript_id,
            source_node = %self.source_node,
            model_id = %self.model_id,
            principal_inferred = %self.principal_inferred,
            transcript_text_hash = %self.transcript_text_hash,
            ts_unix_micros = %self.ts_unix_micros,
            "voice transcript audit"
        );
    }
}

/// Compute the canonical lower-hex SHA-256 of a transcript text.
/// Pulled out so the agent layer can re-derive it when correlating
/// audit rows with the actual command that fired.
pub fn hash_transcript(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let bytes = hasher.finalize();
    let mut s = String::with_capacity(64);
    for b in bytes {
        use std::fmt::Write;
        write!(&mut s, "{:02x}", b).expect("write to String");
    }
    s
}

fn now_unix_micros() -> i128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_micros() as i128,
        // Negative: clock is before epoch (test envs, weird hosts).
        Err(e) => -(e.duration().as_micros() as i128),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::Subscriber;
    use tracing::subscriber;
    use tracing_subscriber::Registry;
    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt;

    /// A tiny tracing layer that captures records emitted on the
    /// audit target so tests can assert one-row-per-transcription.
    #[derive(Clone, Default)]
    struct CaptureLayer {
        rows: Arc<Mutex<Vec<String>>>,
    }
    impl<S: Subscriber> tracing_subscriber::Layer<S> for CaptureLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if event.metadata().target() != AUDIT_TARGET {
                return;
            }
            // Format the event into a single string so we can assert
            // field membership cheaply.
            let mut visitor = StringVisitor::default();
            event.record(&mut visitor);
            self.rows.lock().unwrap().push(visitor.0);
        }
    }

    #[derive(Default)]
    struct StringVisitor(String);
    impl tracing::field::Visit for StringVisitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write;
            let _ = write!(self.0, "{}={:?} ", field.name(), value);
        }
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            use std::fmt::Write;
            let _ = write!(self.0, "{}={} ", field.name(), value);
        }
        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            use std::fmt::Write;
            let _ = write!(self.0, "{}={} ", field.name(), value);
        }
    }

    #[test]
    fn hash_transcript_known_vector() {
        // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        assert_eq!(
            hash_transcript("hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn one_transcription_one_audit_row() {
        let cap = CaptureLayer::default();
        let cap_clone = cap.clone();
        let subscriber = Registry::default().with(cap_clone).with(fmt::layer());
        subscriber::with_default(subscriber, || {
            let event = TranscriptAuditEvent::new(
                42,
                "n-bfc4cd",
                "ggml-base.en",
                "n-test00",
                "open the pod bay doors",
            );
            event.emit();
        });
        let rows = cap.rows.lock().unwrap();
        assert_eq!(rows.len(), 1, "exactly one audit row per emit");
        let row = &rows[0];
        // Required fields per WEFT-210.
        for needle in &[
            "transcript_id",
            "source_node",
            "model_id",
            "principal_inferred",
            "transcript_text_hash",
            "ts_unix_micros",
        ] {
            assert!(row.contains(needle), "missing field {needle}: {row}");
        }
        // The transcript text must NOT appear verbatim — only its hash.
        assert!(
            !row.contains("pod bay doors"),
            "raw transcript text leaked into audit row: {row}"
        );
        // Sanity: hash present and 64 hex chars.
        let expected_hash = hash_transcript("open the pod bay doors");
        assert!(row.contains(&expected_hash), "expected hash absent: {row}");
        assert_eq!(expected_hash.len(), 64);
    }

    #[test]
    fn audit_event_carries_all_fields() {
        let e = TranscriptAuditEvent::new(7, "src", "mid", "p", "x");
        assert_eq!(e.transcript_id, 7);
        assert_eq!(e.source_node, "src");
        assert_eq!(e.model_id, "mid");
        assert_eq!(e.principal_inferred, "p");
        assert_eq!(e.transcript_text_hash.len(), 64);
        assert!(e.ts_unix_micros > 0);
    }
}
