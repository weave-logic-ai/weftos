//! `weftos-leaf-display` — the `LeafSurface` display-bus primitive
//! plus a generic layer compositor for WeftOS leaf devices.
//!
//! See `docs/leaf-push-protocol.md` §7-8 for the design.
//!
//! - [`LeafSurface`] — the contract a physical display bus implements
//!   (esp-hal DPI, a HUB75 matrix, an SPI panel, a host simulator).
//!   All hardware specifics — DMA, PSRAM, cache coherency, double-
//!   buffering — are sealed inside each implementation's `present()`.
//! - [`Compositor`] — routes received `LeafPush` ops into per-
//!   `LayerSlot` op-lists and composites them bottom-up into any
//!   `LeafSurface`.
//! - [`SimSurface`] (`std` feature) — a `Vec`-backed `LeafSurface`
//!   for testing the compositor + render path with zero hardware.
//!
//! This crate is `no_std + alloc`. It is deliberately separate from
//! `weftos-leaf-types` (the pure, hardware-free wire-schema crate);
//! concrete hardware `LeafSurface` impls live in their own crates
//! and depend on this one — this crate never imports esp-hal.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod compositor;
mod surface;

pub use compositor::Compositor;
pub use surface::LeafSurface;

#[cfg(feature = "std")]
mod sim;
#[cfg(feature = "std")]
pub use sim::{SimFrame, SimSurface};

// Re-export the wire types that appear in this crate's public API.
pub use weftos_leaf_types::{DisplaySinkCap, LayerSlot, LeafPush};
