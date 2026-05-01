//! WeftOS substrate state tree + `OntologyAdapter` contract.
//!
//! Implements [ADR-017](../../.planning/symposiums/compositional-ui/adrs/adr-017-ontology-adapter-contract.md).
//!
//! An **ontology adapter** is a stream *producer* that owns a data source
//! (the kernel daemon, `git`, GitHub, an LSP, …) and publishes its state
//! onto declared **topics** as a sequence of structured
//! [`StateDelta`]s. The [`Substrate`] state tree aggregates deltas from
//! every subscribed adapter into a flat `BTreeMap<path, Value>` that
//! surface composers read via [`OntologySnapshot`].
//!
//! M1.5 ships the `kernel` reference adapter (see [`kernel`]). Additional
//! adapters (`git`, `gh`, `workspace`, `fs`, `lsp`, `deployment`) are
//! scheduled for M1.6–M1.9 per Session 10 §7.
//!
//! # What this crate does NOT do (yet)
//! - No governance gating of [`OntologyAdapter::open`]. ADR-017 §3 calls
//!   for install-time permission intersection; M1.5 treats `permissions()`
//!   as advisory and expects the app-manifest layer (M1.5-A, TODO) to
//!   enforce it before calling `open`.
//! - No dynamic-lib adapter registration (ADR-017 §3 path 2 — deferred).
//! - [`PermissionReq`] is a re-export of
//!   [`clawft_app::manifest::Permission`] (unified in M1.5-D).
//!
//! # Adapter-health topic
//!
//! Each subscription emits lifecycle events on
//! `substrate/meta/adapter/<id>/health` (see [`health`]) — `subscription-opened`
//! immediately after a successful [`OntologyAdapter::open`], and
//! `subscription-closed` when the drain task exits (graceful close or
//! abort). Adapters that emit their own per-payload health snapshots
//! (per [`healthcheck`]) write to `substrate/meta/adapter/<id>/healthcheck`
//! independently. ADR-017 §7 obligation satisfied for in-tree adapters.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

pub mod adapter;
pub mod delta;
pub mod health;
pub mod healthcheck;
pub mod projection;
pub mod snapshot;

// Kernel adapter requires a real daemon RPC client and tokio's full
// runtime (time + net). WASM targets proxy RPC through postMessage
// and wire their substrate bindings through the webview host, so the
// kernel module is native-only.
#[cfg(not(target_arch = "wasm32"))]
pub mod kernel;

/// Host-local WiFi / ethernet / battery adapter. Reads `/sys/class/*`
/// directly — no daemon round-trip, no NetworkManager dependency.
/// Native-only; the wasm path is covered by the legacy-Snapshot
/// fallback in `clawft_gui_egui::live` (M1.6+ migrates this to a real
/// substrate-over-postMessage bridge).
#[cfg(not(target_arch = "wasm32"))]
pub mod network;

/// Host-local bluetooth adapter. Reads `/sys/class/bluetooth` +
/// `/sys/class/rfkill` directly — no bluez / bluetoothctl dependency.
/// Native-only for the same reason as [`network`].
#[cfg(not(target_arch = "wasm32"))]
pub mod bluetooth;

/// Mesh adapter — polls the daemon's `cluster.*` RPC verbs. Replaces
/// the tray's `service_present(snap, ["mesh"])` heuristic with real
/// peer/shard counts. Native-only; M1.6+ bridges to wasm.
#[cfg(not(target_arch = "wasm32"))]
pub mod mesh;

/// ExoChain adapter — polls `chain.status`. Emits `available: false`
/// when the daemon lacks the `exochain` feature, so the tray can show
/// a grey chip instead of pretending the chain is up.
#[cfg(not(target_arch = "wasm32"))]
pub mod chain;

/// Physical-sensor extension trait for [`adapter::OntologyAdapter`].
/// Every hardware adapter (mic, camera, radar, speaker, geiger tube,
/// load cell, …) implements both traits so the substrate sees one
/// pluggable surface while the tray / admin UI can interrogate the
/// physical interface and — critically — the
/// [`physical::Characterization`] level (spectrometer principle).
/// Platform-neutral: the trait works on native and wasm; individual
/// sensor implementations gate themselves on what their backing
/// source needs.
pub mod physical;

/// Microphone reference sensor adapter. File-backed preview stub —
/// reads signed-16-bit LE PCM from a configurable path and emits
/// RMS + peak dBFS levels on `substrate/sensor/mic`. Host-audio
/// (CPAL / ALSA / CoreAudio / WASAPI) backing lands in a follow-up.
#[cfg(not(target_arch = "wasm32"))]
pub mod mic;

/// Rfkill enumerated-state sensor adapter. Reads `/sys/class/rfkill/*`
/// and emits one of `unblocked | soft-blocked | hard-blocked | absent`
/// per radio class (wifi, bluetooth, wwan). Native-only — sysfs is
/// Linux-specific. The second [`physical::Characterization`] exemplar
/// (after [`mic`]'s `Rate`) — exercises the `Enumerated` arm of the
/// spectrometer-principle framework.
#[cfg(not(target_arch = "wasm32"))]
pub mod rfkill;

pub use adapter::{
    AdapterError, BufferPolicy, OntologyAdapter, PermissionReq, RefreshHint, Sensitivity, SubId,
    Subscription, TopicDecl,
};
pub use delta::StateDelta;
pub use health::{health_topic_path, AdapterHealthEvent};
pub use healthcheck::{healthcheck_topic_path, SensorHealthReport, SensorStatus};
pub use snapshot::{OntologySnapshot, Substrate};
