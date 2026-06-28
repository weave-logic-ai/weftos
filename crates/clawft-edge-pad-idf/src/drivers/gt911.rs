//! GT911 capacitive touch driver — blocking sync port of the async
//! driver in `clawft-edge-pad/src/drivers/gt911.rs`.
//!
//! Differences from the bare-metal version:
//! - I²C trait is `embedded_hal::i2c::I2c` (blocking) instead of
//!   `embedded_hal_async::i2c::I2c` (async). esp-idf-hal's `I2cDriver`
//!   implements the blocking trait.
//! - `read_frame` is a regular fn, not `async fn` — touch polling
//!   happens on its own FreeRTOS thread (see `main.rs::touch_task`),
//!   so blocking sleeps + blocking I²C are fine.
//!
//! The register map, address-probe sequence, and frame-buffer-clear
//! protocol are line-for-line identical.

#![allow(dead_code)]

use embedded_hal::i2c::I2c;

// ── Register addresses (subset — full map in TAMC_GT911.h) ────────
const REG_PRODUCT_ID: u16 = 0x8140; // 4 bytes ASCII, e.g. "911\0"
const REG_CONFIG_START: u16 = 0x8047;
const REG_POINT_INFO: u16 = 0x814E;
const REG_POINT_1: u16 = 0x814F;
const REG_CONFIG_CHKSUM: u16 = 0x80FF;
const REG_CONFIG_FRESH: u16 = 0x8100;

pub const CONFIG_SIZE: usize = 0xFF - 0x46;

const ADDR_PRIMARY: u8 = 0x14;
const ADDR_FALLBACK: u8 = 0x5D;

const MAX_TOUCHES: usize = 5;

/// One reported touch point.
#[derive(Clone, Copy, Debug, Default)]
pub struct TouchPoint {
    pub id: u8,
    pub x: u16,
    pub y: u16,
    pub size: u16,
}

/// Aggregated touch state from a single read of the GT911.
#[derive(Clone, Debug, Default)]
pub struct TouchFrame {
    pub points: [TouchPoint; MAX_TOUCHES],
    pub touch_count: u8,
    pub large_detect: bool,
}

/// Blocking GT911 driver bound to a shared I²C bus.
pub struct Gt911<I> {
    i2c: I,
    addr: u8,
}

impl<I, E> Gt911<I>
where
    I: I2c<Error = E>,
{
    /// Probe both I²C addresses (PRIMARY first, then FALLBACK) and
    /// bind to whichever responds to a `PRODUCT_ID` read.
    ///
    /// The factory config blob is left untouched — see the bare-metal
    /// port's doc-comment for the rationale (the GT911 ships with a
    /// valid 800×480 config; config-version 0xFF is max-priority, not
    /// blank; the chip self-scans once the PCA9557 has released RST).
    pub fn new(mut i2c: I) -> Result<Self, E> {
        let mut addr = ADDR_PRIMARY;
        let mut found = false;
        for &candidate in &[ADDR_PRIMARY, ADDR_FALLBACK] {
            let mut buf = [0u8; 4];
            let res = read_block(&mut i2c, candidate, REG_PRODUCT_ID, &mut buf);
            if res.is_ok() && &buf[..3] == b"911" {
                addr = candidate;
                found = true;
                break;
            }
        }
        if !found {
            // Surface the error from a primary-address probe.
            let mut buf = [0u8; 4];
            read_block(&mut i2c, ADDR_PRIMARY, REG_PRODUCT_ID, &mut buf)?;
        }
        Ok(Self { i2c, addr })
    }

    pub fn read_config_version(&mut self) -> Result<u8, E> {
        read_byte(&mut self.i2c, self.addr, REG_CONFIG_START)
    }

    pub fn read_config_blob(
        &mut self,
        out: &mut [u8; CONFIG_SIZE],
    ) -> Result<(), E> {
        let mut offset = 0usize;
        while offset < CONFIG_SIZE {
            let chunk = core::cmp::min(32, CONFIG_SIZE - offset);
            let reg = REG_CONFIG_START + offset as u16;
            read_block(&mut self.i2c, self.addr, reg, &mut out[offset..offset + chunk])?;
            offset += chunk;
        }
        Ok(())
    }

    pub fn read_raw_info(&mut self) -> Result<u8, E> {
        read_byte(&mut self.i2c, self.addr, REG_POINT_INFO)
    }

    pub fn address(&self) -> u8 {
        self.addr
    }

    /// Read one touch frame. See the bare-metal port for the protocol
    /// description — `info` is the raw status byte; `frame` is set
    /// only when buffer-ready AND touch_count > 0; the buffer-ready
    /// flag is cleared (write 0 to `POINT_INFO`) only when the chip
    /// actually posted a result.
    pub fn read_frame(&mut self) -> Result<(u8, Option<TouchFrame>), E> {
        let info = read_byte(&mut self.i2c, self.addr, REG_POINT_INFO)?;
        let buffer_ready = (info >> 7) & 1 == 1;
        let touches = (info & 0x0F) as usize;

        if !buffer_ready {
            return Ok((info, None));
        }

        let frame = if touches > 0 {
            let mut f = TouchFrame {
                touch_count: touches.min(MAX_TOUCHES) as u8,
                large_detect: (info >> 6) & 1 == 1,
                ..Default::default()
            };
            for i in 0..f.touch_count as usize {
                let mut data = [0u8; 7];
                let reg = REG_POINT_1 + (i as u16) * 8;
                read_block(&mut self.i2c, self.addr, reg, &mut data)?;
                f.points[i] = TouchPoint {
                    id: data[0],
                    x: u16::from_le_bytes([data[1], data[2]]),
                    y: u16::from_le_bytes([data[3], data[4]]),
                    size: u16::from_le_bytes([data[5], data[6]]),
                };
            }
            Some(f)
        } else {
            None
        };

        // Buffer-ready was set → acknowledge it so the GT911 can post
        // the next scan result.
        write_byte(&mut self.i2c, self.addr, REG_POINT_INFO, 0)?;
        Ok((info, frame))
    }
}

// ── I²C helpers — GT911 uses 16-bit register addresses ─────────────

fn read_byte<I, E>(i2c: &mut I, addr: u8, reg: u16) -> Result<u8, E>
where
    I: I2c<Error = E>,
{
    let mut buf = [0u8; 1];
    read_block(i2c, addr, reg, &mut buf)?;
    Ok(buf[0])
}

fn read_block<I, E>(i2c: &mut I, addr: u8, reg: u16, buf: &mut [u8]) -> Result<(), E>
where
    I: I2c<Error = E>,
{
    let reg_bytes = [(reg >> 8) as u8, (reg & 0xFF) as u8];
    i2c.write_read(addr, &reg_bytes, buf)
}

fn write_byte<I, E>(i2c: &mut I, addr: u8, reg: u16, value: u8) -> Result<(), E>
where
    I: I2c<Error = E>,
{
    let payload = [(reg >> 8) as u8, (reg & 0xFF) as u8, value];
    i2c.write(addr, &payload)
}
