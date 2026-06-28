//! # weftos-leaf-touch-gt911
//!
//! Phase E touch driver for the WeftOS vector-first leaf display.
//! Combines a raw GT911 capacitive-touch register driver with a
//! scene-aware hit-test layer that resolves [`TouchEvent`]s into
//! [`weftos_leaf_scene::InputEnvelope`]s ready to publish on
//! `mesh.leaf.<pk>.input`.
//!
//! Canonical design doc: `docs/design/vector-leaf-display.md` §10
//! Migration Plan (Phase E, "weftos-leaf-touch-gt911").
//!
//! ## Shape at a glance
//!
//! ```text
//!   I²C bus ─► Gt911::poll() ─► Option<TouchEvent>
//!                                       │
//!                                       ▼
//!                                hit_test_event(&scene_store, display_id, event)
//!                                       │
//!                                       ▼
//!                                InputEnvelope { node_id, event, … }
//!                                       │
//!                                       ▼
//!                                publish on mesh.leaf.<pk>.input
//! ```
//!
//! ## What's in this crate
//!
//! | Module           | Contents                                                              |
//! |------------------|-----------------------------------------------------------------------|
//! | [`driver`]       | [`Gt911`] — async GT911 register I/O (port of the edge-pad driver)    |
//! | [`hit_test`]     | [`hit_test_event`] — `TouchEvent` → [`InputEnvelope`] via `SceneStore`|
//!
//! ## What's NOT in this crate
//!
//! - PCA9557 reset dance. The CrowPanel's GT911 RST is on PCA9557 IO1;
//!   the reset must run BEFORE this driver's `new()` opens an I²C
//!   session. That dance lives in `clawft-edge-pad::drivers::pca9557`
//!   because it's board-specific.
//! - Mesh transport. The caller publishes the produced
//!   `InputEnvelope` via whatever mesh path is wired up; this crate is
//!   transport-agnostic.
//! - Gesture recognition (tap / drag / long-press / flick / pinch).
//!   v1 emits raw `PointerDown` / `PointerMove` / `PointerUp`; gestures
//!   are a v1.1 layer above this driver.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod driver;
pub mod hit_test;

pub use driver::{Gt911, TouchEvent, TouchFrame, TouchPhase, TouchPoint};
pub use hit_test::hit_test_event;

// Compile-time guards: every public type that crosses a task boundary
// (channels, statics) should be Send + Sync. The `Gt911<I>` itself is
// NOT required to be Send + Sync — it owns the I²C peripheral, which
// is single-task-owned by construction. The pure-data types ARE.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<TouchPoint>();
    assert_send_sync::<TouchFrame>();
    assert_send_sync::<TouchPhase>();
    assert_send_sync::<TouchEvent>();
};
