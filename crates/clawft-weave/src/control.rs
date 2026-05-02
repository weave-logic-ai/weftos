//! Sensor / service control plane.
//!
//! Substrate-backed enable/disable mechanism for both **daemon-side
//! services** (e.g. the Whisper STT pipeline) and **remote-node
//! sensors** (e.g. the ESP32 mic emissions). The substrate path is
//! the source of truth; in-process flags are derived from it.
//!
//! # Path scheme
//!
//! Every control intent is owned by an *authority node* — the
//! kernel-class node that decided policy. The authority publishes
//! intents under its own prefix:
//!
//! ```text
//! substrate/<authority-node>/control/services/<service-name>
//! substrate/<authority-node>/control/sensors/<target-node>/<sensor-tail>
//! ```
//!
//! Examples:
//!
//! ```text
//! substrate/n-046780/control/services/whisper
//! substrate/n-046780/control/sensors/n-bfc4cd/mic/pcm_chunk
//! substrate/n-046780/control/sensors/n-bfc4cd/mic/rms
//! ```
//!
//! The authority's own prefix means the existing `publish_gated`
//! node-private rule accepts the write — no special-case in the
//! gate, the control plane is just substrate.
//!
//! # Enforcement
//!
//! - **Service intents** (kind = `service`): the affected daemon
//!   service holds an `Arc<AtomicBool>`; the RPC handler updates
//!   it; the service checks the flag in its main loop.
//! - **Sensor intents** (kind = `sensor`): the *target node* (e.g.
//!   the ESP32) subscribes to the path and stops emission when
//!   `enabled == false`. Until firmware-side subscribe ships, the
//!   daemon also soft-disables on its own consumer side — chunks
//!   that arrive on the wire are dropped before processing.
//!
//! # Value shape
//!
//! ```json
//! {
//!   "enabled":   true,
//!   "kind":      "service" | "sensor",
//!   "target":    "whisper" | "n-bfc4cd/mic/pcm_chunk",
//!   "label":     "Whisper STT",
//!   "updated_at_ms": 1700000000000
//! }
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Top-level kind of control intent.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ControlKind {
    /// Toggles a daemon-side in-process service.
    Service,
    /// Toggles a remote-node sensor emission. Targets read this
    /// path and stop emitting when disabled.
    Sensor,
}

impl ControlKind {
    fn path_segment(self) -> &'static str {
        match self {
            ControlKind::Service => "services",
            ControlKind::Sensor => "sensors",
        }
    }

    /// Parse from the JSON wire form (lowercase).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "service" => Some(ControlKind::Service),
            "sensor" => Some(ControlKind::Sensor),
            _ => None,
        }
    }
}

/// One control intent — the wire shape published at the control
/// path. Decoded by the GUI's toggle viewer; written by the daemon
/// when the user toggles or at boot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlIntent {
    /// Current state.
    pub enabled: bool,
    /// What this intent controls.
    pub kind: ControlKind,
    /// Target identifier — service name for `Service`, or
    /// `<node-id>/<sensor-tail>` for `Sensor`. Always matches
    /// the slug encoded in the path.
    pub target: String,
    /// Human-readable label. Used by the GUI; not load-bearing.
    pub label: String,
    /// Wall-clock-ish ms when this intent was last set.
    pub updated_at_ms: u64,
}

impl ControlIntent {
    /// Render the intent as a JSON value for `substrate.publish`.
    pub fn to_value(&self) -> serde_json::Value {
        json!({
            "enabled":       self.enabled,
            "kind":          match self.kind { ControlKind::Service => "service", ControlKind::Sensor => "sensor" },
            "target":        self.target,
            "label":         self.label,
            "updated_at_ms": self.updated_at_ms,
        })
    }
}

/// Build the substrate path for a control intent.
///
/// `target` is treated as one or more path segments — slashes
/// inside it are passed through. Leading / trailing slashes are
/// trimmed.
pub fn intent_path(authority_node: &str, kind: ControlKind, target: &str) -> String {
    let target = target.trim_matches('/');
    format!(
        "substrate/{authority_node}/control/{kind_seg}/{target}",
        kind_seg = kind.path_segment()
    )
}

/// In-process registry mapping `(kind, target)` to the
/// `Arc<AtomicBool>` enforcement flag. Daemon-side services and
/// the soft-disable consumer-side shim register here so the RPC
/// handler can flip flags atomically when the user toggles.
#[derive(Debug, Default, Clone)]
pub struct ControlFlags {
    inner: Arc<DashMap<(ControlKind, String), Arc<AtomicBool>>>,
}

impl ControlFlags {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or look up) the flag for `(kind, target)` with a
    /// default initial value. Returns the shared handle.
    ///
    /// Idempotent — re-registering the same key returns the existing
    /// flag without resetting state, so subsequent toggles persist
    /// across re-registration.
    pub fn register(&self, kind: ControlKind, target: &str, default_enabled: bool) -> Arc<AtomicBool> {
        let key = (kind, target.to_string());
        self.inner
            .entry(key)
            .or_insert_with(|| Arc::new(AtomicBool::new(default_enabled)))
            .clone()
    }

    /// Look up an existing flag without inserting.
    pub fn get(&self, kind: ControlKind, target: &str) -> Option<Arc<AtomicBool>> {
        self.inner
            .get(&(kind, target.to_string()))
            .map(|e| e.value().clone())
    }

    /// Atomically set a flag's value. Returns the prior value, or
    /// `None` if the flag isn't registered.
    pub fn set(&self, kind: ControlKind, target: &str, enabled: bool) -> Option<bool> {
        let flag = self.get(kind, target)?;
        let prior = flag.swap(enabled, Ordering::SeqCst);
        Some(prior)
    }

    /// Snapshot every registered flag — useful for `control.list`.
    pub fn list(&self) -> Vec<(ControlKind, String, bool)> {
        let mut out = Vec::with_capacity(self.inner.len());
        for entry in self.inner.iter() {
            let (kind, target) = entry.key().clone();
            let enabled = entry.value().load(Ordering::SeqCst);
            out.push((kind, target, enabled));
        }
        out
    }
}

/// Current epoch-ms (or `0` on clock failures — always-monotonic is
/// not required, this is a label for humans).
pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_path_for_service() {
        let p = intent_path("n-046780", ControlKind::Service, "whisper");
        assert_eq!(p, "substrate/n-046780/control/services/whisper");
    }

    #[test]
    fn intent_path_for_sensor_with_nested_tail() {
        let p = intent_path(
            "n-046780",
            ControlKind::Sensor,
            "n-bfc4cd/mic/pcm_chunk",
        );
        assert_eq!(
            p,
            "substrate/n-046780/control/sensors/n-bfc4cd/mic/pcm_chunk"
        );
    }

    #[test]
    fn intent_path_trims_target_slashes() {
        let p = intent_path("n-046780", ControlKind::Sensor, "/n-bfc4cd/mic/rms/");
        assert_eq!(p, "substrate/n-046780/control/sensors/n-bfc4cd/mic/rms");
    }

    #[test]
    fn flags_register_and_get_round_trip() {
        let flags = ControlFlags::new();
        let h = flags.register(ControlKind::Service, "whisper", true);
        assert!(h.load(Ordering::SeqCst));
        let h2 = flags.get(ControlKind::Service, "whisper").unwrap();
        assert!(Arc::ptr_eq(&h, &h2), "same Arc returned on lookup");
    }

    #[test]
    fn flags_register_is_idempotent_keeps_state() {
        let flags = ControlFlags::new();
        let h1 = flags.register(ControlKind::Service, "whisper", true);
        h1.store(false, Ordering::SeqCst);
        // Re-registering with default_enabled=true must NOT clobber.
        let h2 = flags.register(ControlKind::Service, "whisper", true);
        assert!(!h2.load(Ordering::SeqCst));
    }

    #[test]
    fn flags_set_returns_prior_value() {
        let flags = ControlFlags::new();
        flags.register(ControlKind::Service, "whisper", true);
        let prior = flags.set(ControlKind::Service, "whisper", false);
        assert_eq!(prior, Some(true));
        let prior = flags.set(ControlKind::Service, "whisper", false);
        assert_eq!(prior, Some(false));
    }

    #[test]
    fn flags_set_unknown_returns_none() {
        let flags = ControlFlags::new();
        let prior = flags.set(ControlKind::Service, "ghost", true);
        assert_eq!(prior, None);
    }

    #[test]
    fn intent_to_value_round_trips() {
        let i = ControlIntent {
            enabled: true,
            kind: ControlKind::Sensor,
            target: "n-bfc4cd/mic/pcm_chunk".into(),
            label: "Mic PCM".into(),
            updated_at_ms: 12345,
        };
        let v = i.to_value();
        assert_eq!(v["enabled"], true);
        assert_eq!(v["kind"], "sensor");
        assert_eq!(v["target"], "n-bfc4cd/mic/pcm_chunk");
        assert_eq!(v["updated_at_ms"], 12345);
    }

    #[test]
    fn control_kind_parse_round_trips_lowercase() {
        assert_eq!(ControlKind::parse("service"), Some(ControlKind::Service));
        assert_eq!(ControlKind::parse("sensor"), Some(ControlKind::Sensor));
        assert_eq!(ControlKind::parse("nope"), None);
    }
}
