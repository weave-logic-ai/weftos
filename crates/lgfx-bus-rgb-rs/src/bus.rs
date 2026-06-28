// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-3-Clause
//
// Derived from LovyanGFX (https://github.com/lovyan03/LovyanGFX) —
// copyright lovyan03 et al., BSD-3-Clause (FreeBSD).
//
// Source: components/LovyanGFX-master/src/lgfx/v1/platforms/esp32s3/Bus_RGB.cpp

//! The `BusRgb` driver — `Bus_RGB::init()` in Rust, with the
//! double-buffer page-flip extension.
//!
//! ## What this is, what this is not
//!
//! `Bus_RGB::init()` (Bus_RGB.cpp:103-316) does several things in one
//! function:
//!
//! - **A.** Initialise the I8080 dummy bus to get DMA channel + pin
//!   muxing (Bus_RGB.cpp:135-167) — esp-hal 1.0 covers this when the
//!   caller calls `Dpi::new(...).with_pclk(...).with_vsync(...)...` etc.
//! - **B.** Configure GDMA `out.conf0` / `out.conf1` for burst,
//!   external-memory access, EOF-mode (Bus_RGB.cpp:179-191) — esp-hal
//!   covers this via `Preparation { burst_transfer, accesses_psram, ... }`
//!   returned from a `DmaTxBuffer::prepare` impl.
//! - **C.** Allocate the framebuffer in PSRAM + build the circular
//!   descriptor chain + build the FIFO-skip restart descriptor
//!   (Bus_RGB.cpp:195-225) — **esp-hal does not cover this**; we do.
//! - **D.** Configure LCD_CAM clocks / `lcd_user` / `lcd_misc` /
//!   `lcd_ctrl` / `lcd_ctrl1` / `lcd_ctrl2` (Bus_RGB.cpp:228-302) —
//!   esp-hal covers this in `Dpi::apply_config` / `Dpi::new`.
//! - **E.** Install the VSYNC ISR + enable interrupts +
//!   `lcd_start = 1` (Bus_RGB.cpp:304-314) — `lcd_start` is in
//!   `Dpi::send`; the **VSYNC ISR is not** in esp-hal; we install it.
//!
//! Where esp-hal already does the C++'s work correctly (A, B, D, the
//! `lcd_start` half of E), use esp-hal. Where it does not (C, the ISR
//! half of E), translate the C++ line-by-line and cite the source.
//!
//! ## Double-buffering — divergence from LovyanGFX
//!
//! With the default `double-buffer` Cargo feature on (the recommended
//! configuration), this crate goes beyond `Bus_RGB.cpp` by adding
//! **two** PSRAM framebuffers and **two** descriptor rings. The VSYNC
//! ISR ([`crate::isr::lcd_vsync_isr`]) atomically swaps which ring
//! it re-arms against when the consumer calls [`BusRgb::present`].
//!
//! Why diverge: `Bus_RGB.cpp` is a bus driver; LovyanGFX expects its
//! upstream `Panel_FrameBufferBase` + LVGL stack to do
//! double-buffering one layer up. We don't have those layers in this
//! crate's scope, and a single framebuffer scanned by the GDMA while
//! the consumer writes to it produces visible compose-during-scan
//! tearing in a non-LVGL render path. Two framebuffers eliminate that
//! tearing: the GDMA always reads a stable, completely-drawn frame.
//!
//! With the `double-buffer` feature off, the crate falls back to a
//! single framebuffer + single restart descriptor — the behaviour of
//! `lgfx-bus-rgb-rs` v0.1.0. The hardware-proven path from 2026-05-15.

use core::alloc::Layout;
use core::ptr;
use core::sync::atomic::Ordering;

use esp_alloc::MemoryCapability;
use esp_hal::dma::{
    BurstConfig, DmaDescriptor, DmaTxBuffer, ExternalBurstConfig, InternalBurstConfig, Owner,
    Preparation, TransferDirection,
};
use esp_hal::interrupt::Priority;
use esp_hal::lcd_cam::lcd::dpi::Dpi;
use esp_hal::peripherals::{Interrupt, LCD_CAM};
use esp_hal::DriverMode;

use crate::config::BusConfig;
use crate::descriptor::{
    build_circular_chain, build_restart_descriptor, descriptor_count, MAX_DMA_LEN,
};
use crate::isr::{
    lcd_vsync_isr, GDMA_CH, OFFSCREEN_RESTART_DESC_ADDR, PRESENT_PENDING, SCANNING_FB,
    SCANNING_RESTART_DESC_ADDR,
};

// ── Static descriptor + framebuffer-pointer storage ──────────────────
//
// LovyanGFX allocates the descriptor array with `heap_caps_malloc(...,
// MALLOC_CAP_DMA)` (Bus_RGB.cpp:200). That capability flag means
// "internal DRAM, DMA-reachable" — equivalent to a Rust `static` in
// `.bss`, which lands in internal DRAM. We use `static`s here to
// avoid pulling in a DMA-tagged allocator just for this.
//
// The fixed maximum here covers the largest practical RGB-DPI panel
// at 24-bit color: 1024 × 768 × 3 B = 2.36 MB → 600 descriptors at
// MAX_DMA_LEN. Round up to 1024 for headroom. With double-buffering
// we need TWO rings, so we declare both. The actual descriptor count
// per ring is computed at runtime from `BusConfig::fb_bytes()`.
const MAX_DESCRIPTORS: usize = 1024;

/// Single-use guard for [`BusRgb::new`].
///
/// The static descriptor storage can only be handed out once. The
/// guard is set inside `new()` under single-threaded init.
static mut TAKEN: bool = false;

/// Descriptor ring for framebuffer A (FB-0). Lives in `.bss` (internal
/// DRAM — required by GDMA: `DmaDescriptor::next` must point at DRAM).
///
/// Bus_RGB.cpp:200 `auto dmadesc = (dma_descriptor_t*)heap_caps_malloc(..., MALLOC_CAP_DMA);`
static mut DESCRIPTORS_A: [DmaDescriptor; MAX_DESCRIPTORS] =
    [DmaDescriptor::EMPTY; MAX_DESCRIPTORS];

/// Descriptor ring for framebuffer B (FB-1). Only populated when the
/// `double-buffer` feature is enabled; otherwise this `static` stays
/// zero and untouched.
#[cfg(feature = "double-buffer")]
static mut DESCRIPTORS_B: [DmaDescriptor; MAX_DESCRIPTORS] =
    [DmaDescriptor::EMPTY; MAX_DESCRIPTORS];

/// FIFO-skip restart descriptor for FB-A.
///
/// Bus_RGB.cpp:132 (Bus_RGB.hpp) `dma_descriptor_t _dmadesc_restart;`
static mut DESCRIPTOR_RESTART_A: DmaDescriptor = DmaDescriptor::EMPTY;

/// FIFO-skip restart descriptor for FB-B. `double-buffer` only.
#[cfg(feature = "double-buffer")]
static mut DESCRIPTOR_RESTART_B: DmaDescriptor = DmaDescriptor::EMPTY;

/// Per-FB base pointers. Indexed by [`SCANNING_FB`] (or its
/// inverse, for the offscreen target). `[null; 2]` until init; index
/// 1 stays null in single-buffer mode.
static mut FB_PTRS: [*mut u8; 2] = [ptr::null_mut(), ptr::null_mut()];

// ── Error type ───────────────────────────────────────────────────────

/// Errors surfaced by [`BusRgb::new`].
#[derive(Debug)]
pub enum BusError {
    /// `BusRgb::new` was called more than once — the static descriptor
    /// storage can only be handed out once.
    AlreadyInitialised,

    /// The framebuffer is larger than the static descriptor capacity.
    /// Recompile with a larger [`MAX_DESCRIPTORS`].
    FramebufferTooLarge,

    /// `esp_alloc::HEAP.alloc_caps(External, ...)` returned null — no
    /// PSRAM heap region is registered, or it is too small to hold the
    /// framebuffer(s). With `double-buffer` enabled (the default) two
    /// framebuffers are allocated, doubling the PSRAM budget — see the
    /// README for the math. The caller is responsible for registering
    /// the PSRAM region before calling `BusRgb::new`; see the example.
    PsramAllocationFailed,

    /// `Dpi::send` rejected the transfer (DMA error).
    DmaSendFailed,

    /// `Interrupt::LCD_CAM` could not be enabled. Without the VSYNC
    /// ISR the GDMA never re-arms and the panel renders one frame
    /// then stalls.
    VsyncIrqEnableFailed,
}

// ── The bus driver ───────────────────────────────────────────────────

/// LovyanGFX `Bus_RGB`, ported, with the double-buffer page-flip
/// extension.
///
/// `BusRgb` owns:
///
/// - one or two PSRAM framebuffers (allocated with
///   `MemoryCapability::External` so PSRAM stays uncontended by the
///   global allocator). With `double-buffer` (default) it's two;
///   without it, one.
/// - one or two circular GDMA descriptor rings + matching FIFO-skip
///   restart descriptors (both backed by `'static` DRAM)
/// - the LCD_CAM VSYNC interrupt binding
///
/// After `new()` returns:
///
/// - the GDMA is walking the FB-A ring, the LCD_CAM is driving the
///   panel, and the VSYNC ISR re-anchors the active ring every frame
/// - both framebuffers are zeroed
/// - the caller writes pixels to [`BusRgb::framebuffer_addr`] (the
///   *offscreen* buffer) and calls [`BusRgb::present`] to schedule a
///   swap-and-flush on the next VSYNC
pub struct BusRgb {
    cfg: BusConfig,
    /// FB-A pointer — also FB_PTRS[0]. Cached on the struct for the
    /// pre-init fallback branch in `framebuffer_addr` and so the
    /// public API can return a stable address before any swap.
    fb_a_ptr: *mut u8,
    /// FB-B pointer — null in single-buffer mode. The actual reads
    /// of this in double-buffer mode go through `FB_PTRS[1]` (the
    /// `'static`) so the ISR doesn't need a `&BusRgb`; the cached
    /// copy here is kept for diagnostic completeness only.
    #[allow(dead_code)]
    fb_b_ptr: *mut u8,
    /// Cached framebuffer length in bytes (same for both buffers).
    fb_len: usize,
}

impl BusRgb {
    /// Build the bus and start the circular GDMA scan.
    ///
    /// `dpi` must already be fully pin-wired and clock-configured by
    /// the caller via `esp_hal::lcd_cam::lcd::dpi::Dpi::new(...)
    /// .with_pclk(...).with_vsync(...).with_hsync(...).with_de(...)
    /// .with_data0(...)..with_data15(...)`. The caller's `Dpi` is
    /// consumed; `Dpi::send` is called from inside this function and
    /// the resulting `DpiTransfer` is `forget`-leaked so the GDMA
    /// keeps scanning for the program's lifetime.
    ///
    /// `cfg.gdma_channel` must match the `DMA_CH<N>` peripheral the
    /// caller passed to `Dpi::new` (esp-hal does not expose it off
    /// the constructed `Dpi`, so the caller has to repeat it).
    ///
    /// Mirrors Bus_RGB.cpp:103-316 with the divisions documented at
    /// the top of this module, plus the double-buffer extension.
    pub fn new<'d, Dm>(dpi: Dpi<'d, Dm>, cfg: BusConfig) -> Result<Self, BusError>
    where
        Dm: DriverMode,
    {
        // Single-use guard.
        //
        // SAFETY: single-threaded init.
        unsafe {
            if TAKEN {
                return Err(BusError::AlreadyInitialised);
            }
            TAKEN = true;
        }

        let pixel_bytes = cfg.pixel_format.bytes_per_pixel();
        let fb_len = cfg.fb_bytes();
        let dcount = descriptor_count(fb_len);
        if dcount > MAX_DESCRIPTORS {
            return Err(BusError::FramebufferTooLarge);
        }

        // ── (C) Allocate framebuffer A in PSRAM ─────────────────────
        //
        // Bus_RGB.cpp:195-196
        //   size_t fb_len = (_cfg.panel->width() * pixel_bytes) * _cfg.panel->height();
        //   auto data = (uint8_t*)heap_alloc_psram(fb_len);
        //
        // LovyanGFX's `heap_alloc_psram` is an ESP-IDF wrapper for
        // `heap_caps_malloc(fb_len, MALLOC_CAP_SPIRAM)`. The Rust
        // equivalent is esp-alloc's capability-tagged allocator
        // entry. The caller must have registered a PSRAM heap region
        // tagged `MemoryCapability::External`; see the example for
        // the two-region heap pattern.
        //
        // 64-byte alignment: esp-hal's `prepare_transfer` rejects a
        // PSRAM buffer whose base or size is not dcache-line aligned.
        let fb_a_ptr = alloc_psram_fb(fb_len)?;

        // FB-B: only allocate if double-buffer is on.
        #[cfg(feature = "double-buffer")]
        let fb_b_ptr = alloc_psram_fb(fb_len)?;
        #[cfg(not(feature = "double-buffer"))]
        let fb_b_ptr: *mut u8 = ptr::null_mut();

        // Bus_RGB.cpp:204-217 doesn't zero the framebuffer (PSRAM
        // contents at boot are undefined). We do so the first frame
        // is black, not whatever garbage was in PSRAM. With
        // double-buffer, mirror the zero on both buffers so the
        // first VSYNC swap reveals a black frame instead of garbage.
        //
        // SAFETY: each fb_ptr..+fb_len is a just-allocated PSRAM
        // region of exactly fb_len bytes.
        unsafe {
            ptr::write_bytes(fb_a_ptr, 0, fb_len);
            #[cfg(feature = "double-buffer")]
            ptr::write_bytes(fb_b_ptr, 0, fb_len);
        }

        // Publish the FB base pointers — `framebuffer_addr` reads
        // these once `SCANNING_FB` has been initialised.
        // SAFETY: TAKEN-guard above; only `BusRgb::new` writes
        // FB_PTRS, and only once.
        unsafe {
            FB_PTRS[0] = fb_a_ptr;
            FB_PTRS[1] = fb_b_ptr;
        }

        // ── (C cont.) Build ring A ──────────────────────────────────
        //
        // SAFETY: TAKEN-guard above proves this is the only `&mut`
        // to DESCRIPTORS_A.
        let desc_a: &'static mut [DmaDescriptor] = unsafe {
            let full: &mut [DmaDescriptor; MAX_DESCRIPTORS] =
                &mut *core::ptr::addr_of_mut!(DESCRIPTORS_A);
            &mut full[..dcount]
        };
        // SAFETY: desc_a.len() == descriptor_count(fb_len); fb_a_ptr
        // points at fb_len bytes of valid PSRAM.
        unsafe {
            build_circular_chain(desc_a, fb_a_ptr, fb_len);
        }

        let restart_a: &'static mut DmaDescriptor =
            unsafe { &mut *core::ptr::addr_of_mut!(DESCRIPTOR_RESTART_A) };
        unsafe {
            build_restart_descriptor(restart_a, desc_a, pixel_bytes);
        }

        // ── (C cont.) Build ring B (double-buffer only) ─────────────
        #[cfg(feature = "double-buffer")]
        let desc_b: &'static mut [DmaDescriptor] = unsafe {
            let full: &mut [DmaDescriptor; MAX_DESCRIPTORS] =
                &mut *core::ptr::addr_of_mut!(DESCRIPTORS_B);
            &mut full[..dcount]
        };
        #[cfg(feature = "double-buffer")]
        // SAFETY: desc_b.len() == descriptor_count(fb_len); fb_b_ptr
        // points at fb_len bytes of valid PSRAM.
        unsafe {
            build_circular_chain(desc_b, fb_b_ptr, fb_len);
        }
        #[cfg(feature = "double-buffer")]
        let restart_b: &'static mut DmaDescriptor =
            unsafe { &mut *core::ptr::addr_of_mut!(DESCRIPTOR_RESTART_B) };
        #[cfg(feature = "double-buffer")]
        unsafe {
            build_restart_descriptor(restart_b, desc_b, pixel_bytes);
        }

        // ── Publish ISR state ───────────────────────────────────────
        //
        // FB-A starts as scanning; FB-B starts as offscreen. In
        // single-buffer mode the offscreen address stays 0 and the
        // ISR's swap branch is dead code (PRESENT_PENDING never gets
        // set because `present()` does not set it in that mode).
        //
        // DRAM is identity-mapped on the S3 → pointer == address.
        SCANNING_RESTART_DESC_ADDR.store(
            (restart_a as *const DmaDescriptor) as u32,
            Ordering::Relaxed,
        );
        #[cfg(feature = "double-buffer")]
        OFFSCREEN_RESTART_DESC_ADDR.store(
            (restart_b as *const DmaDescriptor) as u32,
            Ordering::Relaxed,
        );
        #[cfg(not(feature = "double-buffer"))]
        OFFSCREEN_RESTART_DESC_ADDR.store(0, Ordering::Relaxed);

        PRESENT_PENDING.store(false, Ordering::Relaxed);
        // SCANNING_FB.store with `Release` ordering so when
        // `framebuffer_addr` does an Acquire load it sees the
        // FB_PTRS writes above.
        SCANNING_FB.store(0, Ordering::Release);
        GDMA_CH.store(cfg.gdma_channel, Ordering::Relaxed);

        // ── (B + the lcd_start half of E) Kick the transfer ─────────
        //
        // Bus_RGB.cpp:312-313
        //   dev->lcd_user.lcd_update = 1;
        //   dev->lcd_user.lcd_start  = 1;
        //
        // We hand `Dpi::send` ring A; from this point on the VSYNC
        // ISR redirects the out-link as needed for swaps.
        let buffer = RingBuffer {
            descriptors: desc_a,
            fb_ptr: fb_a_ptr,
            fb_len,
        };
        match dpi.send(true, buffer) {
            Ok(transfer) => {
                core::mem::forget(transfer);
            }
            Err(_) => {
                return Err(BusError::DmaSendFailed);
            }
        }

        // ── (E) Install the VSYNC ISR ───────────────────────────────
        //
        // Bus_RGB.cpp:304-310
        //   dev->lc_dma_int_ena.val = 1;
        //   int isr_flags = ESP_INTR_FLAG_INTRDISABLED | ESP_INTR_FLAG_SHARED;
        //   esp_intr_alloc_intrstatus(lcd_periph_signals.panels[_cfg.port].irq_id, ...);
        //   esp_intr_enable(_intr_handle);
        //
        // Under esp-hal we use `bind_interrupt` + `enable`, then
        // enable the LCD_VSYNC source in the LCD_CAM block.
        //
        // SAFETY: `lcd_vsync_isr.handler()` returns the wrapped
        // `extern "C" fn` for `Interrupt::LCD_CAM`. The handler only
        // touches stolen register-block singletons and lock-free
        // atomics; published statics outlive every ISR fire.
        unsafe {
            esp_hal::interrupt::bind_interrupt(Interrupt::LCD_CAM, lcd_vsync_isr.handler());
        }
        if esp_hal::interrupt::enable(Interrupt::LCD_CAM, Priority::Priority3).is_err() {
            return Err(BusError::VsyncIrqEnableFailed);
        }
        LCD_CAM::regs()
            .lc_dma_int_ena()
            .modify(|_, w| w.lcd_vsync_int_ena().set_bit());

        Ok(Self {
            cfg,
            fb_a_ptr,
            fb_b_ptr,
            fb_len,
        })
    }

    /// `true` if this bus was built with double-buffering — i.e. with
    /// the `double-buffer` Cargo feature enabled (the default).
    ///
    /// Consumers that want different draw strategies for the two
    /// modes (e.g. partial-redraw optimisation when single-buffered)
    /// can branch on this.
    #[inline]
    pub fn is_double_buffered(&self) -> bool {
        cfg!(feature = "double-buffer")
    }

    /// Base address of the *offscreen* framebuffer — the one the
    /// caller is safe to write to without tearing.
    ///
    /// **In double-buffer mode** this is the framebuffer the GDMA is
    /// **not** currently scanning. The address can change across
    /// [`present`](Self::present) calls — the caller MUST treat each
    /// `framebuffer_addr()` value as valid only until the next
    /// `present()` returns. Re-fetching the address after every
    /// present is the safe, idiomatic pattern.
    ///
    /// **In single-buffer mode** this is the single shared buffer
    /// (it is both the scan target and the draw target — tearing is
    /// the accepted tradeoff). The address is stable.
    ///
    /// Equivalent to LovyanGFX's `getDMABuffer(...)`
    /// (Bus_RGB.cpp:318), with the page-flip extension.
    pub fn framebuffer_addr(&self) -> *mut u8 {
        // SAFETY: FB_PTRS was written exactly once in `BusRgb::new`
        // before SCANNING_FB was published with Release ordering;
        // the Acquire load below synchronises with that publish.
        let scanning = SCANNING_FB.load(Ordering::Acquire);
        if scanning == 0xFF {
            // Pre-init read (should not happen because `BusRgb::new`
            // has to return before the consumer can call this) —
            // fall back to FB-A.
            return self.fb_a_ptr;
        }
        if cfg!(feature = "double-buffer") {
            let offscreen_idx = (scanning ^ 1) as usize;
            // SAFETY: FB_PTRS[0..2] was fully initialised in `new`.
            unsafe { FB_PTRS[offscreen_idx] }
        } else {
            self.fb_a_ptr
        }
    }

    /// Framebuffer size in bytes. Same for both buffers in
    /// double-buffer mode.
    #[inline]
    pub fn framebuffer_len(&self) -> usize {
        self.fb_len
    }

    /// Address of the framebuffer the GDMA is currently scanning —
    /// **read-only / diagnostic**. Don't write to it; the GDMA is
    /// streaming it to the panel.
    pub fn scanning_framebuffer_addr(&self) -> *mut u8 {
        let scanning = SCANNING_FB.load(Ordering::Acquire);
        if scanning == 0xFF {
            return self.fb_a_ptr;
        }
        if cfg!(feature = "double-buffer") {
            // SAFETY: FB_PTRS[0..2] was fully initialised in `new`.
            unsafe { FB_PTRS[scanning as usize] }
        } else {
            self.fb_a_ptr
        }
    }

    /// The immutable [`BusConfig`] this bus was built with.
    #[inline]
    pub fn config(&self) -> &BusConfig {
        &self.cfg
    }

    /// Flush the offscreen buffer's dcache to PSRAM, then (in
    /// double-buffer mode) schedule a swap-and-display on the next
    /// VSYNC and **block until the ISR has performed the swap**.
    ///
    /// ## Single-buffer behaviour
    ///
    /// Writes back the entire framebuffer to PSRAM so the GDMA's
    /// next pass reads the pixels just drawn. Returns immediately;
    /// no ISR-side state changes. The behaviour of v0.1.0.
    ///
    /// ## Double-buffer behaviour (synchronous, v0.2.1+)
    ///
    /// 1. Writes back the *offscreen* buffer (the one the caller has
    ///    been drawing to) to PSRAM.
    /// 2. Snapshots the current [`SCANNING_FB`] index.
    /// 3. Sets [`PRESENT_PENDING`] = true.
    /// 4. **Spin-waits until [`SCANNING_FB`] toggles** — confirmation
    ///    that the next VSYNC ISR has fired and performed the swap.
    /// 5. Returns.
    ///
    /// Worst-case latency: one frame period (~33 ms at 30 Hz refresh,
    /// ~17 ms at 60 Hz). Best-case: less than one frame period if a
    /// VSYNC happened to be imminent. Bounded by an internal 100 ms
    /// watchdog so a stuck/disabled ISR cannot wedge the caller.
    ///
    /// ### Why this is synchronous (v0.2.0 was async)
    ///
    /// v0.2.0's `present` returned immediately after setting the
    /// pending flag. A consumer issuing N rapid `present()` calls in
    /// a row could race ahead of the ISR: while VSYNC #1 was still
    /// pending, the second `present()` would see the not-yet-swapped
    /// `SCANNING_FB` and treat the freshly-written buffer as
    /// offscreen — overwriting it before it was ever displayed.
    ///
    /// The synchronous wait fixes this by ensuring the caller's next
    /// [`framebuffer_addr`](Self::framebuffer_addr) read sees a
    /// buffer the GDMA is *not* scanning, because the swap has
    /// already happened. This mirrors LovyanGFX's LVGL-layer
    /// `flush_ready` callback, which synchronises with the upstream
    /// `pushImageDMA` for the same reason.
    ///
    /// ### Latency expectations
    ///
    /// For a 10-event push burst at 30 Hz refresh, the bound is
    /// 10 × 33 ms = ~330 ms total — well within the
    /// process-table render budget. Faster refresh rates lower this
    /// proportionally. If your draw cycle is naturally
    /// VSYNC-paced (the typical case), the wait is *nominally zero*:
    /// the swap completes before the next `present()` is even called.
    ///
    /// ### Watchdog
    ///
    /// 100 ms (~3 frame periods at 30 Hz). If the ISR has not fired
    /// in that window the wait returns anyway, leaving
    /// `PRESENT_PENDING = true` for the ISR to drain on the next
    /// VSYNC. This is a fail-soft: it lets the caller make progress
    /// and a missed swap will be observable as a stale frame, not a
    /// system hang. A persistent watchdog hit indicates the
    /// `Interrupt::LCD_CAM` vector is not being serviced — a
    /// separate bug.
    ///
    /// ## Cache-coherency
    ///
    /// The ESP32-S3 dcache is write-back: a CPU `*ptr = px;` lands
    /// in the cache and does not reach PSRAM until flushed.
    /// LovyanGFX handles this implicitly via
    /// `Panel_FrameBufferBase`'s own writebacks; this crate's
    /// surface is more bare-metal, so we expose it directly.
    ///
    /// The `Cache_Suspend_DCache_Autoload` / `Cache_Resume_*` pair
    /// guards against a known ROM-side autoload race that produces
    /// intermittent torn pixels on PSRAM.
    pub fn present(&mut self) {
        // ROM cache helpers — declared `extern "C"` because they are
        // part of the ESP32-S3 mask ROM and not re-exported through
        // esp-hal.
        unsafe extern "C" {
            fn rom_Cache_WriteBack_Addr(addr: u32, size: u32);
            fn Cache_Suspend_DCache_Autoload() -> u32;
            fn Cache_Resume_DCache_Autoload(value: u32);
        }

        // Flush the offscreen buffer (in single-buffer mode that's
        // the same as scanning — what single-buffer's `present` was
        // doing). `framebuffer_addr()` already accounts for which
        // mode we're in.
        let off = self.framebuffer_addr();

        // SAFETY: `off .. off + self.fb_len` is a valid PSRAM
        // framebuffer region; the suspend/resume pair is balanced.
        unsafe {
            let autoload = Cache_Suspend_DCache_Autoload();
            rom_Cache_WriteBack_Addr(off as u32, self.fb_len as u32);
            Cache_Resume_DCache_Autoload(autoload);
        }

        // ── Schedule the swap + block until ISR confirms (v0.2.1) ──
        //
        // Single-buffer mode: skip entirely — no swap to schedule.
        // Double-buffer mode: snapshot scanning index, set the
        // pending flag, then spin until the ISR has flipped
        // SCANNING_FB. The flip is gated through a `Release` store
        // inside the ISR; the `Acquire` load below synchronises with
        // it, so all the ISR's prior register pokes (out_rst,
        // outlink_addr, outlink_start) are visible by the time the
        // wait returns.
        //
        // The 100 ms watchdog is a fail-soft: if the LCD_CAM
        // interrupt vector is somehow not being serviced, we return
        // anyway and the consumer can make progress. Hitting this in
        // practice indicates a separate bug (priority inversion,
        // interrupt globally masked, etc.). With Priority3 +
        // `#[ram]` placement the ISR completes in ~5 register
        // writes; the only realistic source of >100 ms latency would
        // be the application disabling interrupts globally for
        // longer than a frame period, which is itself an anti-
        // pattern.
        #[cfg(feature = "double-buffer")]
        {
            // Snapshot BEFORE the pending flag — so any ISR fire
            // between the snapshot and the store finds the flag
            // still false, defers the swap to the next fire, and
            // does not race past our wait below.
            let before = SCANNING_FB.load(Ordering::Acquire);

            // Schedule the swap.
            PRESENT_PENDING.store(true, Ordering::Release);

            // Spin-wait until SCANNING_FB toggles (= ISR performed
            // the swap). Watchdog at 100 ms (~3 frame periods at
            // 30 Hz refresh).
            let deadline =
                esp_hal::time::Instant::now() + esp_hal::time::Duration::from_millis(100);
            while SCANNING_FB.load(Ordering::Acquire) == before {
                if esp_hal::time::Instant::now() >= deadline {
                    // Fail-soft. Leave PRESENT_PENDING = true; the
                    // ISR will drain it on its next fire and the
                    // missed frame appears as one stale frame, not a
                    // hang.
                    break;
                }
                core::hint::spin_loop();
            }
            // The pre-init `0xFF` sentinel cannot be observed here:
            // `BusRgb::new` stores `0` to SCANNING_FB before
            // returning, and only `&mut self` callers reach this
            // method.
        }
    }
}

// SAFETY: `BusRgb` keeps the PSRAM framebuffer(s) and `'static`
// descriptor storage alive for the program's lifetime; the raw
// pointers are the GDMA's read view, not Rust references.
unsafe impl Send for BusRgb {}

// ── PSRAM framebuffer allocation helper ──────────────────────────────

fn alloc_psram_fb(fb_len: usize) -> Result<*mut u8, BusError> {
    const CACHE_LINE: usize = 64;
    let layout = match Layout::from_size_align(fb_len, CACHE_LINE) {
        Ok(l) => l,
        Err(_) => return Err(BusError::PsramAllocationFailed),
    };
    // SAFETY: `layout` is non-zero (fb_len > 0 on any real panel).
    let raw = unsafe { esp_alloc::HEAP.alloc_caps(MemoryCapability::External.into(), layout) };
    if raw.is_null() {
        return Err(BusError::PsramAllocationFailed);
    }
    Ok(raw)
}

// ── DmaTxBuffer adapter over the pre-built ring ──────────────────────
//
// esp-hal's `Dpi::send` takes any `DmaTxBuffer`. We don't want
// esp-hal building its own descriptor chain (it would not produce a
// LovyanGFX-compatible one), so this thin wrapper just re-exports the
// chain we already built and tells `Dpi::send` "this is PSRAM, burst
// 64 bytes, don't auto-writeback owner bits". Those `Preparation`
// fields are the Rust analog of Bus_RGB.cpp:179-191's direct writes
// to `GDMA.channel[].out.conf0` / `out.conf1`.
//
// In double-buffer mode we only hand `Dpi::send` ring A; from that
// point on the VSYNC ISR redirects the out-link. esp-hal never
// touches ring B.

struct RingBuffer {
    descriptors: &'static mut [DmaDescriptor],
    #[allow(dead_code)]
    fb_ptr: *mut u8,
    #[allow(dead_code)]
    fb_len: usize,
}

// SAFETY: the framebuffer + descriptor ring are `'static`-lived.
unsafe impl DmaTxBuffer for RingBuffer {
    type View = Self;
    type Final = Self;

    fn prepare(&mut self) -> Preparation {
        // Re-assert owner bits — idempotent. The ring's structure
        // (chain, suc_eof, next pointers) was built once in
        // `build_circular_chain` and never gets reset; only the
        // owner bit is what the GDMA "consumes" per descriptor.
        for d in self.descriptors.iter_mut() {
            d.set_owner(Owner::Dma);
        }

        // Last descriptor of the ring carries suc_eof = true (matches
        // Bus_RGB.cpp:213 `0xC0000000`); intermediates suc_eof = false.
        // Re-affirm to be safe across re-prepares.
        let n = self.descriptors.len();
        for (i, d) in self.descriptors.iter_mut().enumerate() {
            d.set_suc_eof(i == n - 1);
        }

        Preparation {
            start: &mut self.descriptors[0],
            direction: TransferDirection::Out,
            // Bus_RGB.cpp:190 `conf1.out_ext_mem_bk_size = GDMA_LL_EXT_MEM_BK_SIZE_64B;`
            //   → external-memory burst size = 64 bytes (one cache line).
            // Bus_RGB.cpp:184-185
            //   conf0.outdscr_burst_en = 1;
            //   conf0.out_data_burst_en = 1;
            //   → descriptor + data burst on.
            // esp-hal collapses these into `BurstConfig`.
            burst_transfer: BurstConfig {
                external_memory: ExternalBurstConfig::Size64,
                internal_memory: InternalBurstConfig::Disabled,
            },
            // The framebuffer is in PSRAM.
            accesses_psram: true,
            // Owner bits are set once and never handed back to CPU;
            // the GDMA owns the ring for the program lifetime.
            check_owner: Some(false),
            // The VSYNC ISR re-points the out-link every frame —
            // descriptor write-back is unneeded.
            auto_write_back: false,
        }
    }

    fn into_view(self) -> Self::View {
        self
    }

    fn from_view(view: Self::View) -> Self::Final {
        view
    }
}

// Note: the `MAX_DMA_LEN` re-export keeps the descriptor module's
// constant available downstream (e.g. for diagnostic builds that
// want to print the descriptor count).
#[allow(dead_code)]
const _: usize = MAX_DMA_LEN;
