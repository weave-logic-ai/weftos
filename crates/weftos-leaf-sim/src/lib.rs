//! # weftos-leaf-sim
//!
//! Desktop simulator for the WeftOS vector-first leaf display.
//!
//! Implements [`weftos_leaf_renderer::SceneSurface`] against an
//! [`embedded_graphics_simulator::SimulatorDisplay`] so the full
//! scene-to-pixels pipeline runs without hardware. The `boot` example
//! renders a representative frame; the `tween_demo` example exercises
//! the v1 snap-to-end tween behaviour.
//!
//! ## Usage
//!
//! ```no_run
//! use weftos_leaf_renderer::render_damage;
//! use weftos_leaf_scene::{DamageSet, Rect, SceneStore};
//! use weftos_leaf_sim::SimSurface;
//!
//! let mut store = SceneStore::new();
//! store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
//! // ... apply some ops ...
//!
//! let mut surface = SimSurface::new(800, 480, "WeftOS leaf sim");
//! render_damage(&store, 0, &DamageSet::full(), &mut surface).unwrap();
//! surface.show();
//! ```
//!
//! ## Capability profile
//!
//! The sim declares
//! `ALPHA | SUBPIXEL | ANTIALIASED | BLEND_MODES` — it "cheats" in the
//! sense that `embedded-graphics-simulator` can show all of these for
//! free, so devs see rich rendering even before the hardware backends
//! catch up. The actual v1 DPI backend (Phase C) ships
//! `CapabilityMask::empty()`; the canvas backend (Phase D) ships the
//! same flags as the sim plus `BITMAP_PNG`.

pub mod sim_surface;

pub use sim_surface::{SimError, SimSurface};
