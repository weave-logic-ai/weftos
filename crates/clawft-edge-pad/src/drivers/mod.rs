//! Hardware drivers for the Inkpad spike.
//!
//! Each driver consumes the pin constants from [`crate::board`] and
//! exposes an async-friendly API for the embassy executor.
//!
//! Phase E note: the GT911 driver has been extracted to the
//! standalone `weftos-leaf-touch-gt911` crate (no_std + alloc) so its
//! scene-aware hit-test path can be unit-tested off-target. Consumers
//! should `use weftos_leaf_touch_gt911::Gt911;` directly.

pub mod dpi_surface;
pub mod lcd_rgb;
pub mod pca9557;
