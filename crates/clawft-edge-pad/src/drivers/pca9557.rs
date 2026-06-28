//! PCA9557 8-bit I²C I/O expander — board control lines on the
//! CrowPanel DIS08070H v3.0.
//!
//! The v3.0 board routes the GT911 touch-controller RST (and the LCD
//! panel reset) through this expander rather than direct ESP32 GPIOs.
//! Without driving the expander, the GT911 never gets a clean reset
//! pulse and sits in a non-scanning state — `POINT_INFO` reads 0x00
//! forever even while the panel is being touched.
//!
//! Sequence transcribed from the Elecrow v3.0 touch demo `setup()`:
//!   <https://github.com/Elecrow-RD/CrowPanel-ESP32-Display-Course-File>
//!   `Code/7.0 v3.0 touch new code/4/.../crowpanel-esp32-7.0-3.0-touch.ino`
//!
//! ```c
//! Out.reset(); Out.setMode(IO_OUTPUT);
//! Out.setState(IO0, IO_LOW); Out.setState(IO1, IO_LOW);  // both low
//! delay(20);
//! Out.setState(IO0, IO_HIGH);                            // IO0 → high
//! delay(100);
//! Out.setMode(IO1, IO_INPUT);                            // IO1 released
//! ```
//!
//! IO0 = LCD panel reset, IO1 = GT911 RST. Releasing IO1 to input
//! mode lets a board pull-up take the GT911 RST high → chip boots
//! its scan engine.
//!
//! PCA9557 register map (NXP datasheet):
//!   0x00 input port · 0x01 output port · 0x02 polarity · 0x03 config
//!   config bit: 1 = input, 0 = output. Power-on default 0xFF (all in).

#![allow(dead_code)]

use embedded_hal_async::i2c::I2c;
use embassy_time::{Duration, Timer};

const REG_OUTPUT: u8 = 0x01;
const REG_CONFIG: u8 = 0x03;

/// Candidate I²C addresses — PCA9557 strap range is 0x18..=0x1F.
const ADDR_CANDIDATES: [u8; 8] = [0x19, 0x18, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F];

/// Probe the shared I²C bus for the PCA9557 and run the v3.0 board's
/// power-up control sequence: pulse-reset the GT911 (and LCD panel)
/// control lines so the touch controller starts scanning.
///
/// Returns the address the expander was found at, or the I²C error
/// from the last probe attempt.
pub async fn reset_board_peripherals<I, E>(i2c: &mut I) -> Result<u8, E>
where
    I: I2c<Error = E>,
{
    // Probe: a 1-byte write of the config register's power-on default
    // (0xFF) is harmless and ACKs only at the real address.
    let mut addr = ADDR_CANDIDATES[0];
    let mut found = false;
    for &candidate in &ADDR_CANDIDATES {
        if i2c.write(candidate, &[REG_CONFIG, 0xFF]).await.is_ok() {
            addr = candidate;
            found = true;
            break;
        }
    }
    if !found {
        // Surface the error from a probe of the most-likely address.
        i2c.write(ADDR_CANDIDATES[0], &[REG_CONFIG, 0xFF]).await?;
    }

    // 1. Preset output latches low, then switch all pins to output:
    //    IO0 + IO1 are now actively driven LOW (resets asserted).
    i2c.write(addr, &[REG_OUTPUT, 0x00]).await?;
    i2c.write(addr, &[REG_CONFIG, 0x00]).await?;
    Timer::after(Duration::from_millis(20)).await;

    // 2. IO0 → HIGH (LCD panel out of reset). IO1 stays low.
    i2c.write(addr, &[REG_OUTPUT, 0x01]).await?;
    Timer::after(Duration::from_millis(100)).await;

    // 3. IO1 → input mode: releases the GT911 RST line, board pull-up
    //    takes it high, the GT911 boots its scan engine. (config bit
    //    1 = 1 ⇒ input; bits 0 and 2..7 stay output.)
    i2c.write(addr, &[REG_CONFIG, 0x02]).await?;
    Timer::after(Duration::from_millis(100)).await; // let GT911 boot

    Ok(addr)
}
