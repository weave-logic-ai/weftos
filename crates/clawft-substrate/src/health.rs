//! Adapter-health topic — `substrate/meta/adapter/<id>/health`.
//!
//! ADR-017 §7 reserves a meta-topic per adapter so subscribers can
//! distinguish "no data because nothing changed" from "no data because
//! the adapter died." This module provides the event vocabulary and a
//! small helper for emitting events into the substrate.
//!
//! ## Event vocabulary
//!
//! Three event kinds today, all emitted as a `Replace` on the per-adapter
//! health path:
//!
//! - `subscription-opened` — a subscription was successfully opened on
//!   one of the adapter's topics. Emitted by [`Substrate::subscribe_adapter`]
//!   immediately after a successful `open()` call.
//! - `subscription-closed` — the drain task has exited. Emitted when the
//!   adapter terminates the sender (graceful close) or when
//!   [`Substrate::close_all`] aborts the drain. Subscribers can read
//!   this to differentiate "stalled adapter" from "dead adapter."
//! - `error` — the adapter reported an error from `open()`. Emitted with
//!   the [`AdapterError`] description.
//!
//! ## Path shape
//!
//! `substrate/meta/adapter/<adapter-id>/health` — singleton per adapter.
//! Wholesale `Replace` per event; the most recent event wins. Event
//! history is intentionally not retained (subscribers that need it can
//! mirror the path into their own state). This keeps the substrate's
//! flat-map cheap to clone for snapshots.
//!
//! ## Relationship to the per-sensor healthcheck contract
//!
//! [`crate::healthcheck`] is the richer per-sensor / per-node health
//! shape (rate observation, error counters, status enum). This module is
//! the lower-level *adapter lifecycle* health — it answers "is the
//! adapter wired in and pumping?" rather than "is the underlying signal
//! healthy?" The two coexist: an adapter can be `subscription-opened`
//! while the sensor it fronts is `down` (e.g. mic is open but the
//! backing audio source is missing).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::adapter::SubId;
use crate::delta::StateDelta;

/// Lifecycle event for an adapter subscription.
///
/// Serialized as the `event` field on the per-adapter health topic.
/// Stable string form so wire-compatible consumers (Explorer, tray) can
/// match without a Rust dep on this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterHealthEvent {
    /// A subscription was opened successfully — the adapter is pumping.
    SubscriptionOpened,
    /// The drain task exited — adapter terminated the sender, or the
    /// substrate aborted us. Subscribers should treat the previously
    /// observed path as stale until a new `subscription-opened` arrives.
    SubscriptionClosed,
    /// `open()` failed — the adapter could not start the subscription.
    /// Carried alongside a free-form `reason` field on the topic value.
    Error,
}

impl AdapterHealthEvent {
    /// Stable lower-case-kebab form for serialization.
    pub fn as_str(self) -> &'static str {
        match self {
            AdapterHealthEvent::SubscriptionOpened => "subscription-opened",
            AdapterHealthEvent::SubscriptionClosed => "subscription-closed",
            AdapterHealthEvent::Error => "error",
        }
    }
}

/// Build the standard adapter-health topic path for `adapter_id`.
///
/// Stable so consumers can construct the path without a runtime
/// dependency on the adapter being instantiated.
pub fn health_topic_path(adapter_id: &str) -> String {
    format!("substrate/meta/adapter/{adapter_id}/health")
}

/// Build a `Replace` delta for an adapter-health event.
///
/// `topic` is the substrate topic that triggered the event (e.g. the
/// kernel-status path on `subscription-opened`). `sub_id` is the
/// subscription handle when known; `None` for `Error` events that
/// occurred before a subscription was issued. `reason` is a free-form
/// human-readable string for `Error` events; `None` otherwise.
pub fn build_event_delta(
    adapter_id: &str,
    event: AdapterHealthEvent,
    topic: &str,
    sub_id: Option<SubId>,
    reason: Option<&str>,
) -> StateDelta {
    let mut value = serde_json::Map::new();
    value.insert("event".into(), json!(event.as_str()));
    value.insert("adapter".into(), json!(adapter_id));
    value.insert("topic".into(), json!(topic));
    if let Some(SubId(id)) = sub_id {
        value.insert("sub_id".into(), json!(id));
    }
    if let Some(r) = reason {
        value.insert("reason".into(), json!(r));
    }
    StateDelta::Replace {
        path: health_topic_path(adapter_id),
        value: Value::Object(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_topic_path_uses_meta_prefix() {
        assert_eq!(
            health_topic_path("kernel"),
            "substrate/meta/adapter/kernel/health"
        );
    }

    #[test]
    fn event_strings_are_kebab_case() {
        assert_eq!(
            AdapterHealthEvent::SubscriptionOpened.as_str(),
            "subscription-opened"
        );
        assert_eq!(
            AdapterHealthEvent::SubscriptionClosed.as_str(),
            "subscription-closed"
        );
        assert_eq!(AdapterHealthEvent::Error.as_str(), "error");
    }

    #[test]
    fn build_event_delta_carries_topic_and_sub_id() {
        let d = build_event_delta(
            "kernel",
            AdapterHealthEvent::SubscriptionOpened,
            "substrate/kernel/status",
            Some(SubId(42)),
            None,
        );
        match d {
            StateDelta::Replace { path, value } => {
                assert_eq!(path, "substrate/meta/adapter/kernel/health");
                assert_eq!(value["event"], "subscription-opened");
                assert_eq!(value["adapter"], "kernel");
                assert_eq!(value["topic"], "substrate/kernel/status");
                assert_eq!(value["sub_id"], 42);
                assert!(value.get("reason").is_none());
            }
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    #[test]
    fn build_event_delta_includes_reason_for_errors() {
        let d = build_event_delta(
            "mesh",
            AdapterHealthEvent::Error,
            "substrate/mesh/status",
            None,
            Some("daemon-unreachable"),
        );
        let Some(value) = (match d {
            StateDelta::Replace { value, .. } => Some(value),
            _ => None,
        }) else {
            panic!("expected Replace");
        };
        assert_eq!(value["event"], "error");
        assert_eq!(value["reason"], "daemon-unreachable");
        assert!(value.get("sub_id").is_none());
    }
}
