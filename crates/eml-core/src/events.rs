//! Events emitted by EML models for chain logging.
//!
//! Each EML model accumulates events during its lifecycle (training,
//! prediction, drift detection, save/load). The kernel-level code is
//! responsible for draining these events and appending them to the
//! ExoChain — the models themselves are chain-agnostic.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// EmlEvent
// ---------------------------------------------------------------------------

/// An event emitted by an EML model during its lifecycle.
///
/// These events are accumulated in a per-model event log and drained
/// by the caller for chain persistence. The model itself never touches
/// the ExoChain directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EmlEvent {
    /// Model training completed.
    Trained {
        model_name: String,
        samples_used: usize,
        mse_before: f64,
        mse_after: f64,
        converged: bool,
        param_count: usize,
    },
    /// Significant model prediction recorded.
    Prediction {
        model_name: String,
        /// BLAKE3 hash of input features (hex-encoded).
        inputs_hash: String,
        output: Vec<f64>,
    },
    /// Drift detected between prediction and actual value.
    Drift {
        model_name: String,
        predicted: f64,
        actual: f64,
        drift_pct: f64,
    },
    /// Model state saved to disk.
    Saved {
        model_name: String,
        path: String,
        param_count: usize,
    },
    /// Model state loaded from disk.
    Loaded {
        model_name: String,
        path: String,
        trained: bool,
        samples: usize,
    },
    /// Model was reset.
    Reset { model_name: String, reason: String },
}

impl EmlEvent {
    /// Return the canonical event type string for chain logging.
    ///
    /// Used as the `kind` field when appending to ExoChain.
    pub fn event_type(&self) -> &'static str {
        match self {
            EmlEvent::Trained { .. } => "eml.trained",
            EmlEvent::Prediction { .. } => "eml.prediction",
            EmlEvent::Drift { .. } => "eml.drift",
            EmlEvent::Saved { .. } => "eml.saved",
            EmlEvent::Loaded { .. } => "eml.loaded",
            EmlEvent::Reset { .. } => "eml.reset",
        }
    }

    /// Return the model name embedded in this event.
    pub fn model_name(&self) -> &str {
        match self {
            EmlEvent::Trained { model_name, .. }
            | EmlEvent::Prediction { model_name, .. }
            | EmlEvent::Drift { model_name, .. }
            | EmlEvent::Saved { model_name, .. }
            | EmlEvent::Loaded { model_name, .. }
            | EmlEvent::Reset { model_name, .. } => model_name,
        }
    }
}

// ---------------------------------------------------------------------------
// EmlEventLog
// ---------------------------------------------------------------------------

/// Accumulator for EML lifecycle events.
///
/// Models push events here during operations. Callers drain the log
/// periodically and forward events to the ExoChain or other sinks.
#[derive(Debug, Clone, Default)]
pub struct EmlEventLog {
    events: Vec<EmlEvent>,
}

impl EmlEventLog {
    /// Create a new empty event log.
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// Push a new event.
    pub fn push(&mut self, event: EmlEvent) {
        self.events.push(event);
    }

    /// Drain all accumulated events, returning them and clearing the log.
    pub fn drain(&mut self) -> Vec<EmlEvent> {
        std::mem::take(&mut self.events)
    }

    /// Number of pending events.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_strings() {
        let e = EmlEvent::Trained {
            model_name: "test".into(),
            samples_used: 100,
            mse_before: 1.0,
            mse_after: 0.01,
            converged: true,
            param_count: 50,
        };
        assert_eq!(e.event_type(), "eml.trained");

        let e = EmlEvent::Drift {
            model_name: "test".into(),
            predicted: 1.0,
            actual: 1.1,
            drift_pct: 10.0,
        };
        assert_eq!(e.event_type(), "eml.drift");
    }

    #[test]
    fn event_log_drain() {
        let mut log = EmlEventLog::new();
        assert!(log.is_empty());

        log.push(EmlEvent::Reset {
            model_name: "test".into(),
            reason: "manual".into(),
        });
        assert_eq!(log.len(), 1);

        let drained = log.drain();
        assert_eq!(drained.len(), 1);
        assert!(log.is_empty());
    }

    #[test]
    fn model_name_accessor() {
        let e = EmlEvent::Saved {
            model_name: "coherence".into(),
            path: "/tmp/test".into(),
            param_count: 50,
        };
        assert_eq!(e.model_name(), "coherence");
    }
}
