//! Host-side `LeafSurface` — a `Vec`-backed simulator for testing
//! the compositor + `LeafPush` render path with zero hardware.
//!
//! `present()` is a no-op (the buffer is simply retained); callers
//! and tests inspect the result via [`SimSurface::buffer`] /
//! [`SimSurface::pixel`]. Kept dependency-light on purpose — no
//! windowing or image-encoding crates; a caller that wants a window
//! or a PNG can read `buffer()` and do that itself.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use weftos_leaf_types::DisplaySinkCap;

use crate::surface::LeafSurface;

/// A `Vec<Rgb888>`-backed [`LeafSurface`] for host-side testing.
pub struct SimSurface {
    width: u32,
    height: u32,
    buf: Vec<Rgb888>,
    frames_presented: u64,
}

impl SimSurface {
    /// New simulator surface, `width × height`, cleared to black.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            buf: vec![Rgb888::BLACK; (width * height) as usize],
            frames_presented: 0,
        }
    }

    /// The full pixel buffer, row-major.
    pub fn buffer(&self) -> &[Rgb888] {
        &self.buf
    }

    /// One pixel, or `None` if out of bounds.
    pub fn pixel(&self, x: u32, y: u32) -> Option<Rgb888> {
        if x < self.width && y < self.height {
            Some(self.buf[(y * self.width + x) as usize])
        } else {
            None
        }
    }

    /// How many times `present()` has been called.
    pub fn frames_presented(&self) -> u64 {
        self.frames_presented
    }
}

/// A `DrawTarget` view over a [`SimSurface`]'s buffer.
pub struct SimFrame<'a> {
    width: u32,
    height: u32,
    buf: &'a mut [Rgb888],
}

impl OriginDimensions for SimFrame<'_> {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for SimFrame<'_> {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels {
            if let (Ok(x), Ok(y)) = (u32::try_from(coord.x), u32::try_from(coord.y)) {
                if x < self.width && y < self.height {
                    self.buf[(y * self.width + x) as usize] = color;
                }
            }
        }
        Ok(())
    }
}

impl LeafSurface for SimSurface {
    type Frame<'a> = SimFrame<'a>;
    type Error = core::convert::Infallible;

    fn capability(&self) -> DisplaySinkCap {
        DisplaySinkCap {
            width: self.width,
            height: self.height,
            pixel_format: String::from("rgb888"),
            layers: 4,
            blend_modes: vec![String::from("normal")],
        }
    }

    fn frame(&mut self) -> SimFrame<'_> {
        SimFrame {
            width: self.width,
            height: self.height,
            buf: &mut self.buf,
        }
    }

    fn present(&mut self) -> Result<(), Self::Error> {
        self.frames_presented += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Compositor;
    use weftos_leaf_types::{DisplayText, LayerSlot, LeafPush};

    #[test]
    fn text_op_renders_to_sim_surface() {
        let mut comp = Compositor::new();
        let mut surf = SimSurface::new(200, 60);
        comp.apply(LeafPush::DisplayText(DisplayText {
            z: LayerSlot::Text,
            text: String::from("OK"),
            x: 4,
            y: 20,
            color: [255, 255, 255],
            clear_first: false,
        }));
        comp.compose(&mut surf).unwrap();
        assert_eq!(surf.frames_presented(), 1);
        // Some pixel in the text's bounding region must be non-black.
        let any_lit = (0..40)
            .flat_map(|x| (4..32).map(move |y| (x, y)))
            .any(|(x, y)| surf.pixel(x, y) != Some(Rgb888::BLACK));
        assert!(any_lit, "DisplayText should have lit pixels on the surface");
    }
}
