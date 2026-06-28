//! The `OntologyAdapter` trait + supporting declarations.
//!
//! ADR-017 §1–2, §4. An adapter is a trait object held behind `Arc` so
//! the substrate can outlive any one subscriber.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::delta::StateDelta;

/// Subscription handle — newtype around a u64 issued by the adapter.
///
/// ADR-009 tombstone discipline: once [`OntologyAdapter::close`] has
/// been called with a given id, late deltas arriving on the receiver
/// MUST fail cleanly. The substrate currently enforces this by dropping
/// the receiver; adapters that spin their own tasks for a subscription
/// must observe closure of their `Sender` and exit.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SubId(pub u64);

/// How often the composer should expect fresh data on a topic.
///
/// This is a hint: the composer uses it to decide whether to show
/// stale-chip affordances, not to drive polling. The adapter is still
/// responsible for actually producing deltas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RefreshHint {
    /// The adapter emits whenever the underlying source does.
    EventDriven,
    /// The adapter polls on a fixed interval.
    Periodic {
        /// Poll period in milliseconds.
        ms: u64,
    },
    /// The adapter only emits in response to a `request-only` read.
    /// No subscription; `open` returns one delta and closes.
    RequestOnly,
}

/// Privacy sensitivity of a topic's values. Drives the install-time
/// prompt copy and the ADR-012 tray-chip obligation (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    /// Safe to display anywhere; no prompt required.
    Public,
    /// Scoped to the current workspace/project; one-line summary prompt.
    Workspace,
    /// Personal data beyond the workspace (home dir, browser history).
    /// Full disclosure dialog required.
    Private,
    /// Derived from ambient capture (camera/mic/screen). Requires a
    /// per-goal ADR-012 `CapabilityGrant`; cannot be granted at install
    /// alone.
    Capture,
}

/// Back-pressure discipline per topic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BufferPolicy {
    /// Append-only streams — drop the oldest frame when the channel is
    /// full. Surfaces as a gap counter on adapter-health.
    DropOldest,
    /// Singletons and form-state — producer refuses and logs a warning.
    Refuse,
    /// Default for replace-by-id collections — producer blocks up to
    /// ~50ms then drops and self-degrades.
    BlockCapped,
}

/// Declaration of a single topic the adapter produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopicDecl {
    /// Topic path (e.g. `"substrate/kernel/processes"`). Literal prefix
    /// pattern; a single trailing `*` segment is the only wildcard
    /// allowed.
    pub path: &'static str,
    /// Ontology shape URI (e.g. `"ontology://process-list"`).
    /// Placeholder at M1.5; a formal schema registry is ADR-020+.
    pub shape: &'static str,
    /// Refresh cadence hint.
    pub refresh_hint: RefreshHint,
    /// Privacy sensitivity — see [`Sensitivity`].
    pub sensitivity: Sensitivity,
    /// Buffer policy — see [`BufferPolicy`].
    pub buffer_policy: BufferPolicy,
    /// Maximum retained length for list-typed topics.
    ///
    /// When `Some(n)`, [`crate::Substrate::apply`] auto-trims the
    /// front of a list topic on each [`StateDelta::Append`] so the
    /// array never holds more than `n` entries. `None` means unbounded
    /// (the topic is a singleton or the adapter manages its own
    /// retention). This is a substrate-side realisation of the
    /// drop-oldest ring contract described in ADR-017 §5 for the
    /// kernel log topic; M1.5-D may fold this into the ADR text.
    pub max_len: Option<usize>,
}

/// Capability the adapter needs the host to grant at install time.
///
/// Intersected with the app manifest's declared permissions during
/// install (ADR-015). Denial fails install closed.
///
/// Canonical enum lives in `clawft-app` as
/// [`clawft_app::manifest::Permission`]; `clawft-substrate` re-exports it
/// under the ADR-017 trait-contract name `PermissionReq`. The two were
/// duplicated during M1.5-A/C parallel development and unified in M1.5-D.
pub type PermissionReq = clawft_app::manifest::Permission;

/// Errors that an adapter can return from `open` / `close`.
#[derive(Debug, Error)]
pub enum AdapterError {
    /// Topic path not recognised by the adapter.
    #[error("unknown topic: {0}")]
    UnknownTopic(String),
    /// The adapter's underlying source is unreachable.
    #[error("source unavailable: {0}")]
    SourceUnavailable(String),
    /// Arguments passed to `open` were not valid for the topic.
    #[error("invalid args for topic {topic}: {reason}")]
    InvalidArgs {
        /// The topic that was being opened.
        topic: String,
        /// Reason the args were invalid.
        reason: String,
    },
    /// Permissions required by this topic have not been granted.
    /// Governance integration is M1.6+; M1.5 never emits this.
    #[error("permission denied: {0:?}")]
    PermissionDenied(Vec<PermissionReq>),
    /// Something else went wrong — carries a description.
    #[error("adapter error: {0}")]
    Other(String),
}

/// Handle returned from [`OntologyAdapter::open`]. Owns the receiving
/// half of an mpsc channel the adapter writes deltas to.
pub struct Subscription {
    /// Subscription id — pass back to [`OntologyAdapter::close`].
    pub id: SubId,
    /// Receiver end of the delta stream.
    pub rx: mpsc::Receiver<StateDelta>,
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .finish()
    }
}

/// Trait every ontology adapter implements.
///
/// ADR-017 §1. `open` returns a receiver bound to a specific topic; the
/// caller drains deltas and applies them to [`crate::Substrate`].
/// `close` is a tombstone (ADR-009) — late deltas after `close` MUST be
/// ignored.
#[async_trait]
pub trait OntologyAdapter: Send + Sync {
    /// Stable short identifier (`"kernel"`, `"git"`, `"gh"`, ...).
    fn id(&self) -> &'static str;

    /// Declared topic set. Const slice so consumers can introspect
    /// without instantiating the adapter.
    fn topics(&self) -> &'static [TopicDecl];

    /// Permissions the adapter requires at install-time.
    /// Intersected with app-manifest permissions by governance.
    fn permissions(&self) -> &'static [PermissionReq];

    /// Open a subscription on a topic. `args` carries topic-specific
    /// configuration (e.g. the root path for an `fs` watcher, the
    /// desired log-tail length for `substrate/kernel/logs`).
    async fn open(&self, topic: &str, args: Value) -> Result<Subscription, AdapterError>;

    /// Tombstone a subscription. MUST be idempotent — calling `close`
    /// for an unknown id is not an error.
    async fn close(&self, sub_id: SubId) -> Result<(), AdapterError>;
}
