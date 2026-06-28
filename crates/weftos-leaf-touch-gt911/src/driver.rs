//! GT911 capacitive touch — async register I/O.
//!
//! Datasheet / register-map reference: the TAMC_GT911 Arduino driver,
//! at <https://github.com/Elecrow-RD/gt911_for_crowpanel>.
//!
//! Bus: 400 kHz I²C. Address: 0x14 on v3.0+ boards (factory default
//! for CrowPanel DIS08070H), 0x5D on older revs. The driver probes
//! both at construction time.
//!
//! Per-poll lifecycle:
//!
//! 1. Read `POINT_INFO` (0x814E). If the buffer-ready bit (bit 7) is
//!    clear, the chip hasn't posted a fresh scan — return `None`.
//! 2. If `touch_count > 0`, read each point's 7-byte block at
//!    `POINT_1` + 8·i.
//! 3. Write 0 to `POINT_INFO` to ack the buffer. The chip can't post
//!    the next scan until this happens.
//!
//! Up to 5 simultaneous touches; we report all of them.
//!
//! ## Multi-touch tracking
//!
//! The GT911 assigns a stable `id` (track-id) per finger. Between
//! polls, the driver compares the new frame's set of ids against the
//! previous frame's:
//!
//! - ids in `prev` but not `new` → `PointerUp` for each.
//! - ids in `new` but not `prev` → `PointerDown` for each.
//! - ids in both → `PointerMove` for each (even if x/y didn't change;
//!   the cadence is intentional so the consumer sees liveness).
//!
//! This is the same model Wayland's `wl_touch` exposes (down / motion /
//! up keyed on a track-id). The `Gt911::poll_events` helper expands
//! one `TouchFrame` into the right `Vec<TouchEvent>`.

use alloc::vec::Vec;

use embedded_hal_async::i2c::I2c;

// ─── Register addresses (subset — full map in TAMC_GT911.h) ────────
const REG_PRODUCT_ID: u16 = 0x8140; // 4 bytes ASCII, e.g. "911\0"
const REG_CONFIG_START: u16 = 0x8047;
const REG_POINT_INFO: u16 = 0x814E;
const REG_POINT_1: u16 = 0x814F;

/// Config blob span: 0x8047..=0x80FF inclusive (185 bytes), per the
/// TAMC GT911 Arduino library `GT911_CONFIG_SIZE = 0xFF - 0x46`.
pub const CONFIG_SIZE: usize = 0xFF - 0x46;

const ADDR_PRIMARY: u8 = 0x14;
const ADDR_FALLBACK: u8 = 0x5D;

/// Maximum simultaneous touches the GT911 reports.
pub const MAX_TOUCHES: usize = 5;

/// One reported touch point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TouchPoint {
    /// Track id assigned by the chip. Stable across polls for the
    /// same finger.
    pub id: u8,
    /// Display x in **integer pixels**.
    pub x: u16,
    /// Display y in **integer pixels**.
    pub y: u16,
    /// Touch-area metric (chip-dependent units).
    pub size: u16,
}

/// Aggregated touch state from a single read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TouchFrame {
    pub points: [TouchPoint; MAX_TOUCHES],
    pub touch_count: u8,
    pub large_detect: bool,
}

/// Phase of a per-finger touch event. Mirrors Wayland's `wl_touch` /
/// the W3C Pointer Events model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    /// First frame the track id appears in.
    Down,
    /// Subsequent frame for the same track id.
    Move,
    /// Track id was present in the previous frame but missing now.
    Up,
}

/// One touch event, derived from a track-id diff between consecutive
/// frames. Coordinates are in display **integer pixels** (the GT911
/// reports them that way; hit-test conversion to Q24.8 happens in
/// [`super::hit_test_event`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TouchEvent {
    pub phase: TouchPhase,
    pub pointer_id: u8,
    pub x_px: i32,
    pub y_px: i32,
    /// Touch-area metric; 0 on `Up` events.
    pub size: u16,
}

/// Async GT911 driver bound to a shared I²C bus.
///
/// `Gt911` keeps a copy of the previous frame's track-ids so
/// [`Gt911::poll_events`] can synthesize `Down` / `Move` / `Up`
/// transitions without the caller maintaining external state.
pub struct Gt911<I> {
    i2c: I,
    addr: u8,
    /// Previous frame's active track ids — used to diff against the
    /// next read and emit Up events.
    prev_ids: heapless::FnvIndexSet<u8, 8>,
}

impl<I, E> Gt911<I>
where
    I: I2c<Error = E>,
{
    /// Probe both I²C addresses (ADDR_PRIMARY first) and bind to
    /// whichever responds to a `PRODUCT_ID` read.
    ///
    /// The factory config blob is left **completely untouched** — the
    /// CrowPanel's GT911 ships with a valid 800×480 config (confirmed
    /// by a full register dump 2026-05-14). The `config version` byte
    /// reading 0xFF is *valid* — it's the max-priority version, not
    /// "blank". No `CONFIG_FRESH` poke, no checksum rewrite; the chip
    /// scans on its own once the PCA9557 has released its RST line.
    pub async fn new(mut i2c: I) -> Result<Self, E> {
        let mut addr = ADDR_PRIMARY;
        let mut found = false;
        for &candidate in &[ADDR_PRIMARY, ADDR_FALLBACK] {
            let mut buf = [0u8; 4];
            let res = read_block(&mut i2c, candidate, REG_PRODUCT_ID, &mut buf).await;
            if res.is_ok() && &buf[..3] == b"911" {
                addr = candidate;
                found = true;
                break;
            }
        }
        if !found {
            // Surface the error from a primary-address probe.
            let mut buf = [0u8; 4];
            read_block(&mut i2c, ADDR_PRIMARY, REG_PRODUCT_ID, &mut buf).await?;
        }
        Ok(Self {
            i2c,
            addr,
            prev_ids: heapless::FnvIndexSet::new(),
        })
    }

    /// Read the raw 1-byte config-version register. Diagnostic.
    pub async fn read_config_version(&mut self) -> Result<u8, E> {
        read_byte(&mut self.i2c, self.addr, REG_CONFIG_START).await
    }

    /// Dump the full 185-byte config region (0x8047..=0x80FF) into the
    /// caller's buffer. Diagnostic helper.
    pub async fn read_config_blob(&mut self, out: &mut [u8; CONFIG_SIZE]) -> Result<(), E> {
        let mut offset = 0usize;
        while offset < CONFIG_SIZE {
            let chunk = core::cmp::min(32, CONFIG_SIZE - offset);
            let reg = REG_CONFIG_START + offset as u16;
            read_block(&mut self.i2c, self.addr, reg, &mut out[offset..offset + chunk]).await?;
            offset += chunk;
        }
        Ok(())
    }

    /// Read the raw `POINT_INFO` status byte without interpreting or
    /// clearing it. Diagnostic helper.
    pub async fn read_raw_info(&mut self) -> Result<u8, E> {
        read_byte(&mut self.i2c, self.addr, REG_POINT_INFO).await
    }

    /// The bound I²C address (0x14 or 0x5D), for diagnostics.
    pub fn address(&self) -> u8 {
        self.addr
    }

    /// Low-level poll: return the raw status byte + optional
    /// [`TouchFrame`]. Acknowledges the buffer-ready bit on read.
    ///
    /// Use [`Gt911::poll_events`] for the high-level "what changed
    /// since last frame" view.
    pub async fn read_frame(&mut self) -> Result<(u8, Option<TouchFrame>), E> {
        let info = read_byte(&mut self.i2c, self.addr, REG_POINT_INFO).await?;
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
                read_block(&mut self.i2c, self.addr, reg, &mut data).await?;
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

        // Buffer-ready was set → ack so the chip can post the next scan.
        write_byte(&mut self.i2c, self.addr, REG_POINT_INFO, 0).await?;
        Ok((info, frame))
    }

    /// High-level poll: read one frame, diff against the previous, and
    /// return a `Vec<TouchEvent>` for the host to publish.
    ///
    /// Returns an empty Vec when nothing changed (idle scan); the
    /// caller can use that to gate the next mesh publish.
    pub async fn poll_events(&mut self) -> Result<Vec<TouchEvent>, E> {
        let (_, frame) = self.read_frame().await?;

        let mut new_ids: heapless::FnvIndexSet<u8, 8> = heapless::FnvIndexSet::new();
        let mut events: Vec<TouchEvent> = Vec::new();

        if let Some(frame) = frame {
            for i in 0..frame.touch_count as usize {
                let p = &frame.points[i];
                let _ = new_ids.insert(p.id);
                let phase = if self.prev_ids.contains(&p.id) {
                    TouchPhase::Move
                } else {
                    TouchPhase::Down
                };
                events.push(TouchEvent {
                    phase,
                    pointer_id: p.id,
                    x_px: p.x as i32,
                    y_px: p.y as i32,
                    size: p.size,
                });
            }
        }

        // Emit Up events for ids that disappeared.
        for prev in self.prev_ids.iter() {
            if !new_ids.contains(prev) {
                events.push(TouchEvent {
                    phase: TouchPhase::Up,
                    pointer_id: *prev,
                    // We don't know the last x/y when the GT911 reports
                    // an absence — emit (0, 0). v1.1 can stash the last
                    // known position keyed on track id; v1's consumer
                    // (the host) only needs to know *that* the touch
                    // ended.
                    x_px: 0,
                    y_px: 0,
                    size: 0,
                });
            }
        }

        self.prev_ids = new_ids;
        Ok(events)
    }
}

// ─── I²C helpers — GT911 uses 16-bit register addresses ─────────────

async fn read_byte<I, E>(i2c: &mut I, addr: u8, reg: u16) -> Result<u8, E>
where
    I: I2c<Error = E>,
{
    let mut buf = [0u8; 1];
    read_block(i2c, addr, reg, &mut buf).await?;
    Ok(buf[0])
}

async fn read_block<I, E>(i2c: &mut I, addr: u8, reg: u16, buf: &mut [u8]) -> Result<(), E>
where
    I: I2c<Error = E>,
{
    let reg_bytes = [(reg >> 8) as u8, (reg & 0xFF) as u8];
    i2c.write_read(addr, &reg_bytes, buf).await
}

async fn write_byte<I, E>(i2c: &mut I, addr: u8, reg: u16, value: u8) -> Result<(), E>
where
    I: I2c<Error = E>,
{
    let payload = [(reg >> 8) as u8, (reg & 0xFF) as u8, value];
    i2c.write(addr, &payload).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // The async I²C trait is hard to mock cheaply at unit-test time;
    // these tests cover the pure-data plumbing only. The full
    // hardware-in-the-loop path is covered by smoke tests on the
    // CrowPanel.

    #[test]
    fn touch_frame_defaults_to_empty() {
        let f = TouchFrame::default();
        assert_eq!(f.touch_count, 0);
        assert!(!f.large_detect);
    }

    #[test]
    fn touch_event_phases_distinct() {
        assert_ne!(TouchPhase::Down, TouchPhase::Move);
        assert_ne!(TouchPhase::Move, TouchPhase::Up);
    }
}
