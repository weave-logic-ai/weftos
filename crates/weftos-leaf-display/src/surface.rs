//! The `LeafSurface` primitive — see `docs/leaf-push-protocol.md` §7.

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::DrawTarget;
use weftos_leaf_types::DisplaySinkCap;

/// The leaf-side display-bus contract: a physical (or simulated)
/// display that a compositor can draw a frame into and present.
///
/// `LeafSurface` is the leaf-side mirror of the publisher-side
/// `LeafRenderer` (`docs/leaf-push-protocol.md` §6): where
/// `LeafRenderer` is "service state → `LeafPush`", `LeafSurface` is
/// "composited frame → photons".
///
/// All hardware specifics — DMA, PSRAM, cache coherency, double-
/// buffering, blit — are sealed inside the implementation's
/// [`present`](LeafSurface::present). Nothing above this trait sees
/// a descriptor or a cache line. That separation is the whole point:
/// the layer compositor and the `LeafPush` dispatch are written
/// **once**, against this trait; swapping the physical bus (esp-hal
/// DPI ↔ HUB75 ↔ SPI panel ↔ host simulator) is a type parameter.
pub trait LeafSurface {
    /// The back buffer the compositor draws into — an
    /// `embedded-graphics` `DrawTarget` of `Rgb888` pixels.
    type Frame<'a>: DrawTarget<Color = Rgb888, Error = Self::Error>
    where
        Self: 'a;

    /// Error type for `frame()` draws and `present()`.
    type Error: core::fmt::Debug;

    /// This surface's capability profile — the "head profile".
    /// Single source of truth: leaf firmware builds
    /// `LeafServices.display_sink` straight from this, and every
    /// `LeafRenderer` designs against it.
    fn capability(&self) -> DisplaySinkCap;

    /// Borrow the back buffer as a `DrawTarget`. The compositor
    /// renders the full composited layer stack into it each frame.
    fn frame(&mut self) -> Self::Frame<'_>;

    /// Present the back buffer. Swap / DMA-kick / cache-flush / blit
    /// semantics are entirely the implementation's problem. Returns
    /// when it is safe to draw the next frame.
    fn present(&mut self) -> Result<(), Self::Error>;

    /// Optional hardware brightness (PWM duty / backlight on-time, in
    /// microseconds). Default: a no-op `Ok` — surfaces without
    /// hardware brightness silently accept; a compositor that cares
    /// can fall back to pixel-level dimming.
    fn set_brightness(&mut self, _on_us: u32) -> Result<(), Self::Error> {
        Ok(())
    }
}
