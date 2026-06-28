// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-3-Clause
//
// Derived from LovyanGFX (https://github.com/lovyan03/LovyanGFX) —
// copyright lovyan03 et al., BSD-3-Clause (FreeBSD).
//
// Source: components/LovyanGFX-master/src/lgfx/v1/platforms/esp32s3/Bus_RGB.cpp

//! VSYNC ISR — force-stop the GDMA and re-arm it at the FIFO-skip
//! restart descriptor once per frame. Extended in this crate (a
//! deliberate divergence from LovyanGFX, see below) with **page-flip
//! support** for the double-buffered driver: on each VSYNC the ISR
//! consults a `PRESENT_PENDING` atomic and points the `out_link` at
//! either the currently-scanning ring's restart descriptor (no swap)
//! or the offscreen ring's restart descriptor (swap).
//!
//! Direct port of Bus_RGB.cpp:66-94 (`lcd_default_isr_handler`):
//!
//! ```c
//! IRAM_ATTR void Bus_RGB::lcd_default_isr_handler(void *args) {
//!   Bus_RGB *me = (Bus_RGB*)args;
//!   auto dev = getDev(me->config().port);
//!   uint32_t intr_status = dev->lc_dma_int_st.val & 0x03;
//!   dev->lc_dma_int_clr.val = intr_status;
//!   if (intr_status & LCD_LL_EVENT_VSYNC_END) {
//!     GDMA.channel[me->_dma_ch].out.conf0.out_rst = 1;
//!     GDMA.channel[me->_dma_ch].out.conf0.out_rst = 0;
//!     GDMA.channel[me->_dma_ch].out.link.addr = (uintptr_t)&(me->_dmadesc_restart);
//!     GDMA.channel[me->_dma_ch].out.link.start = 1;
//!   }
//! }
//! ```
//!
//! ## Divergences from LovyanGFX
//!
//! 1. `void *args` → zero-arg `#[handler]` + atomic-published state.
//!    Same in single- and double-buffer modes.
//!
//! 2. `IRAM_ATTR` → Rust `#[ram]`.
//!
//! 3. **Page-flip extension** (this crate, not LovyanGFX). LovyanGFX
//!    does not double-buffer at the bus layer — its
//!    `Panel_FrameBufferBase` owns a single framebuffer and LVGL
//!    above it owns its own draw buffers. Here we add a per-VSYNC
//!    swap check between the int-status clear and the re-arm:
//!
//!    - [`SCANNING_RESTART_DESC_ADDR`] — restart descriptor of the FB
//!      the GDMA is currently scanning. This is the address used for
//!      the re-arm if no swap is pending.
//!    - [`OFFSCREEN_RESTART_DESC_ADDR`] — restart descriptor of the
//!      FB the consumer is writing to. Becomes the scanning value
//!      after a swap.
//!    - [`PRESENT_PENDING`] — set by `present()`, cleared by ISR via
//!      `compare_exchange` so the swap is one-shot.
//!    - [`SCANNING_FB`] — index (0 or 1) of the currently-scanning
//!      framebuffer. Flipped in lockstep with the restart-desc swap.
//!      `BusRgb::framebuffer_addr` reads it and returns the *other*
//!      buffer's base.
//!
//! The swap happens exactly at the VSYNC boundary, before the GDMA
//! starts walking the next frame's first descriptor — so the GDMA
//! never reads from a buffer the consumer is mid-write to.
//!
//! ## ISR rules (do not violate)
//!
//! - `#[ram]` (= LovyanGFX's `IRAM_ATTR`). No flash-fetch stall.
//! - No allocation, no locks. Atomic-only state hand-off.
//! - The swap uses `compare_exchange(true, false)` on
//!   [`PRESENT_PENDING`] so concurrent `present()` calls from
//!   non-ISR context are race-safe: at most one swap per VSYNC.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use esp_hal::handler;
use esp_hal::peripherals::{DMA, LCD_CAM};
use esp_hal::ram;

/// Restart descriptor address of the framebuffer the GDMA is
/// currently scanning. On VSYNC the ISR uses this (or the offscreen
/// value, if a swap is pending) as the re-arm target.
///
/// `0` = not yet armed (ISR no-ops). On the ESP32-S3 internal DRAM
/// is identity-mapped, so the pointer value equals the address.
///
/// In single-buffer compile-time configurations this collapses to
/// the single restart descriptor — [`OFFSCREEN_RESTART_DESC_ADDR`]
/// stays zero and the swap branch is dead code.
pub(crate) static SCANNING_RESTART_DESC_ADDR: AtomicU32 = AtomicU32::new(0);

/// Restart descriptor address of the framebuffer the consumer is
/// writing to. Becomes the scanning value on the next acknowledged
/// swap. Stays `0` in single-buffer mode.
pub(crate) static OFFSCREEN_RESTART_DESC_ADDR: AtomicU32 = AtomicU32::new(0);

/// `true` when `BusRgb::present()` has asked the ISR to swap on the
/// next VSYNC. The ISR clears it via `compare_exchange` after
/// performing the swap. Stays `false` in single-buffer mode.
pub(crate) static PRESENT_PENDING: AtomicBool = AtomicBool::new(false);

/// Index of the currently-scanning framebuffer: `0` for FB-A, `1` for
/// FB-B. `BusRgb::framebuffer_addr` reads this and returns the OTHER
/// framebuffer's base — i.e. the offscreen / safe-to-write address.
///
/// `0xFF` means "not yet initialised" (matches [`GDMA_CH`]'s
/// sentinel) — `BusRgb::framebuffer_addr` checks for this so a
/// pre-init read does not return junk.
pub(crate) static SCANNING_FB: AtomicU8 = AtomicU8::new(0xFF);

/// GDMA out-channel index. `0xFF` = unset.
///
/// LovyanGFX caches this as `Bus_RGB::_dma_ch` (Bus_RGB.hpp:135), set
/// at runtime by `search_dma_out_ch` (Bus_RGB.cpp:170). esp-hal does
/// not expose the index off a constructed `Dpi`, so the caller passes
/// it through [`crate::BusConfig::gdma_channel`].
pub(crate) static GDMA_CH: AtomicU8 = AtomicU8::new(0xFF);

/// The VSYNC ISR.
///
/// Mirrors Bus_RGB.cpp:66-94 for the int-status read + clear + the
/// `out_rst` + `out_link` re-arm. **Extends** the C++ with the
/// page-flip check between the int-status clear and the re-arm.
///
/// # Safety / real-time
///
/// Touches only the `LCD_CAM` and `DMA` register-block singletons
/// (no allocation, no locks); the `AtomicU*` reads are lock-free.
/// Must be the registered handler for `Interrupt::LCD_CAM`.
#[handler]
#[ram]
pub(crate) fn lcd_vsync_isr() {
    let lcd_cam = LCD_CAM::regs();

    // Bus_RGB.cpp:71-72
    //   uint32_t intr_status = dev->lc_dma_int_st.val & 0x03;
    //   dev->lc_dma_int_clr.val = intr_status;
    //
    // Bits 0..1 of `lc_dma_int_st` are LCD_VSYNC + LCD_TRANS_DONE
    // (`LCD_LL_EVENT_VSYNC_END = BIT(0)` in `hal/lcd_ll.h`). Clear
    // exactly the bits that were set via write-1-to-clear.
    let st = lcd_cam.lc_dma_int_st().read();
    let vsync = st.lcd_vsync_int_st().bit_is_set();
    let trans_done = st.lcd_trans_done_int_st().bit_is_set();
    lcd_cam.lc_dma_int_clr().write(|w| {
        if vsync {
            w.lcd_vsync_int_clr().set_bit();
        }
        if trans_done {
            w.lcd_trans_done_int_clr().set_bit();
        }
        w
    });

    // Bus_RGB.cpp:73 `if (intr_status & LCD_LL_EVENT_VSYNC_END) { ... }`
    if !vsync {
        return;
    }

    let ch = GDMA_CH.load(Ordering::Relaxed);
    if ch == 0xFF {
        return;
    }

    // ── Page-flip check (divergence from LovyanGFX) ──────────────────
    //
    // If the consumer asked for a swap, atomically swap which ring
    // the ISR will re-arm against. `compare_exchange` ensures the
    // pending flag is only cleared if it was actually set — so two
    // ISR fires that both observe pending=true cannot both swap (the
    // second sees pending=false and falls through to the no-swap
    // branch).
    let do_swap = PRESENT_PENDING
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok();

    if do_swap {
        // Atomic swap of scanning ↔ offscreen restart-desc addresses.
        // Read both first, then write both — a torn read from
        // `framebuffer_addr` (non-ISR context) can still happen
        // mid-swap, but `framebuffer_addr` keys off `SCANNING_FB`
        // (one atomic) rather than the desc addrs, so the swap is
        // observably atomic from the consumer's view.
        let old_scanning = SCANNING_RESTART_DESC_ADDR.load(Ordering::Relaxed);
        let new_scanning = OFFSCREEN_RESTART_DESC_ADDR.load(Ordering::Relaxed);
        SCANNING_RESTART_DESC_ADDR.store(new_scanning, Ordering::Relaxed);
        OFFSCREEN_RESTART_DESC_ADDR.store(old_scanning, Ordering::Relaxed);

        // Flip the scanning-FB index in lockstep. XOR with 1 toggles
        // 0 ↔ 1; the `0xFF` sentinel is only seen before init.
        // Release ordering pairs with `framebuffer_addr`'s Acquire
        // load so a consumer reading after a swap sees the new
        // offscreen target's base address consistently.
        let cur = SCANNING_FB.load(Ordering::Relaxed);
        if cur != 0xFF {
            SCANNING_FB.store(cur ^ 1, Ordering::Release);
        }
    }

    let restart = SCANNING_RESTART_DESC_ADDR.load(Ordering::Relaxed);
    if restart == 0 {
        // Not yet armed by `BusRgb::new`.
        return;
    }

    let dma_ch = DMA::regs().ch(ch as usize);

    // Bus_RGB.cpp:74-75
    //   GDMA.channel[me->_dma_ch].out.conf0.out_rst = 1;
    //   GDMA.channel[me->_dma_ch].out.conf0.out_rst = 0;
    //
    // Two distinct `modify` writes — the C++ does two separate
    // assignments; esp-hal's PAC requires the same since `out_rst` is
    // a one-cycle pulse field.
    dma_ch.out_conf0().modify(|_, w| w.out_rst().set_bit());
    dma_ch.out_conf0().modify(|_, w| w.out_rst().clear_bit());

    // Bus_RGB.cpp:76-77
    //   GDMA.channel[me->_dma_ch].out.link.addr = (uintptr_t)&(me->_dmadesc_restart);
    //   GDMA.channel[me->_dma_ch].out.link.start = 1;
    //
    // esp-hal collapses both writes into one `modify` of `out_link`.
    //
    // SAFETY: `restart` is the address of a `'static` restart
    // descriptor (either FB-A's or FB-B's, picked above) published by
    // `BusRgb::new`; both outlive every ISR fire.
    dma_ch.out_link().modify(|_, w| {
        unsafe { w.outlink_addr().bits(restart) };
        w.outlink_start().set_bit()
    });
}
