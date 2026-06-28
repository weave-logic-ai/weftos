//! Hardware drivers for the ESP-IDF Inkpad port.
//!
//! The PCA9557 + GT911 drivers are written against the **blocking**
//! `embedded_hal::i2c::I2c` trait (esp-idf-hal's `I2cDriver` implements
//! it), so the porting seam is the trait — not esp-hal vs esp-idf-hal.
//! These files are mechanical sync→blocking ports of the matching
//! files in `crates/clawft-edge-pad/src/drivers/`.
//!
//! The LCD bringup is *not* in a `dpi_surface.rs` here — it lives in
//! the parent module's `display.rs`, which wraps the `esp_lcd_panel_rgb`
//! IDF driver and exposes a `LeafSurface` over it. That driver
//! replaces the ~1300-line hand-rolled `dpi_surface.rs` from the
//! bare-metal port wholesale; bounce buffers, frame sync, and the
//! FIFO-skip restart descriptor are all handled inside Espressif's
//! supported driver.

pub mod gt911;
pub mod pca9557;
