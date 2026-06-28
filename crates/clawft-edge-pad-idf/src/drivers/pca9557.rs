//! PCA9557 8-bit I²C I/O expander — board control lines on the
//! CrowPanel DIS08070H v3.0.
//!
//! Mechanical sync port of `clawft-edge-pad/src/drivers/pca9557.rs`.
//! The original used `embedded_hal_async::i2c::I2c`; here we use the
//! blocking `embedded_hal::i2c::I2c` (the only thing esp-idf-hal's
//! `I2cDriver` implements). The reset sequence is identical.
//!
//! Sequence transcribed from the Elecrow v3.0 touch demo (see the
//! original file for the full source-of-truth cite):
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
//! IO0 = LCD panel reset, IO1 = GT911 RST.
//!
//! PCA9557 register map (NXP datasheet):
//!   0x00 input port · 0x01 output port · 0x02 polarity · 0x03 config
//!   config bit: 1 = input, 0 = output. Power-on default 0xFF (all in).

#![allow(dead_code)]

use embedded_hal::i2c::I2c;
use std::thread;
use std::time::Duration;

const REG_OUTPUT: u8 = 0x01;
const REG_CONFIG: u8 = 0x03;

/// Candidate I²C addresses — PCA9557 strap range is 0x18..=0x1F.
const ADDR_CANDIDATES: [u8; 8] = [0x19, 0x18, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F];

/// Probe the shared I²C bus for the PCA9557 and run the v3.0 board's
/// power-up control sequence. See file-level docs for the sequence.
pub fn reset_board_peripherals<I, E>(i2c: &mut I) -> Result<u8, E>
where
    I: I2c<Error = E>,
{
    // Probe: a 1-byte write of the config register's power-on default
    // (0xFF) is harmless and ACKs only at the real address.
    let mut addr = ADDR_CANDIDATES[0];
    let mut found = false;
    for &candidate in &ADDR_CANDIDATES {
        if i2c.write(candidate, &[REG_CONFIG, 0xFF]).is_ok() {
            addr = candidate;
            found = true;
            break;
        }
    }
    if !found {
        // Surface the error from a probe of the most-likely address.
        i2c.write(ADDR_CANDIDATES[0], &[REG_CONFIG, 0xFF])?;
    }

    // 1. Preset output latches low, then switch all pins to output:
    //    IO0 + IO1 are now actively driven LOW (resets asserted).
    i2c.write(addr, &[REG_OUTPUT, 0x00])?;
    i2c.write(addr, &[REG_CONFIG, 0x00])?;
    thread::sleep(Duration::from_millis(20));

    // 2. IO0 → HIGH (LCD panel out of reset). IO1 stays low.
    i2c.write(addr, &[REG_OUTPUT, 0x01])?;
    thread::sleep(Duration::from_millis(100));

    // 3. IO1 → input mode: releases the GT911 RST line, board pull-up
    //    takes it high, the GT911 boots its scan engine.
    i2c.write(addr, &[REG_CONFIG, 0x02])?;
    thread::sleep(Duration::from_millis(100));

    Ok(addr)
}
