//! `LeafSurface` implementation for the CrowPanel DIS08070H 800×480
//! RGB-parallel TFT, driven by the ESP32-S3 `LCD_CAM` DPI peripheral.
//!
//! This module is the *sealed* hardware bus behind the
//! `weftos_leaf_display::LeafSurface` trait. Everything above the trait
//! — the layer compositor, the `LeafPush` dispatch — never sees a DMA
//! descriptor or a cache line. See `docs/leaf-push-protocol.md` §8.
//!
//! ## Frame-lock: the empirically-converged design
//!
//! Nine prior configs were attempted. Configs #1–#8 were all run on a
//! contention-poisoned firmware (PSRAM heap), which masked the actual
//! frame-lock behaviour with regional FIFO-underrun glitching. Fix B
//! (config #9: heap → internal SRAM via two-region `esp_alloc` heap,
//! framebuffer via `alloc_caps(External)`) eliminated the contention
//! and proved that **uncontended GDMA→PSRAM scanout is rock-steady**.
//! But config #9 also showed that a pure free-running circular ring,
//! even uncontended, does not frame-lock — the GDMA read pointer and
//! the LCD raster start drift apart, manifesting as a steady diagonal
//! wrap.
//!
//! Config #10 (this file) keeps fix B's contention fix verbatim and
//! adds the frame-lock that was always needed: **non-circular chain so
//! the GDMA parks at frame end, + VSYNC ISR re-arming the parked
//! channel each frame**. This is structurally config #8's architecture
//! — but config #8 was *never actually tested on hardware* (the
//! contention breakthrough arrived before its flash) and was designed
//! under sound register-level reasoning that still holds.
//!
//! ### The contention diagnostic (proved fix B was needed)
//!
//! A static 10 px test grid was drawn once into the framebuffer and
//! never rewritten — a pure DPI-scanout test, no draw path involved.
//! Two builds:
//!
//! - **WiFi + touch + mesh ON:** ~100–200 px blocks shifting
//!   independently up/down/left/right, blinking — all over the screen.
//! - **WiFi + touch + mesh ALL OFF** (only the GDMA touches PSRAM):
//!   the grid is **rock-steady**. Crisp, stable, zero movement.
//!
//! ### Root cause: PSRAM bandwidth contention
//!
//! The framebuffer is in PSRAM. The *heap* was **also** in PSRAM
//! (`psram_allocator!` in `main.rs`). Every CPU PSRAM access — WiFi
//! stack buffers, heap allocations, embassy / mesh work — contended
//! with the GDMA's hard-real-time read of the framebuffer (~24 MB/s to
//! feed the LCD at 12 MHz × 2 B/px). Contention starved the LCD_CAM's
//! async TX FIFO → underrun → the regional "blocks shifting / blinking"
//! artifact, landing wherever the GDMA happened to be fetching during a
//! CPU PSRAM burst.
//!
//! This is why none of configs #1–#8 worked: **the descriptor
//! structure was never the problem.** `suc_eof` count, circular vs
//! parked chain, ISR vs no-ISR — all irrelevant to a FIFO underrun
//! caused by a starved PSRAM read. The earlier "scroll", "rip" and
//! "drift" symptoms were all this *same* underrun, just shaped
//! differently by whatever descriptor structure was in place.
//!
//! ### The fix (config #9): remove PSRAM contention at the source
//!
//! Two fixes were on the table:
//!
//! - **A — internal-SRAM bounce buffer.** The GDMA reads small
//!   ping-pong buffers in fast, uncontended internal SRAM; an ISR
//!   refills them from the PSRAM framebuffer in chunks. Bulletproof
//!   regardless of PSRAM load, but ~200+ lines (a 2nd descriptor ring,
//!   a refill ISR with raster-tracking, bounce-sizing) and several
//!   hardware-iteration unknowns. esp-hal 1.0 has **no native
//!   bounce-buffer support** (verified across `lcd_cam/lcd/dpi.rs`,
//!   `dma/mod.rs`, `dma/buffers.rs` — the concept does not exist in the
//!   HAL), so A would be entirely hand-rolled.
//! - **B — move the heap to internal SRAM** so PSRAM holds *only* the
//!   framebuffer, touched *only* by the GDMA. The diagnostic *directly
//!   proves* an uncontended GDMA→PSRAM read is rock-steady, so B's
//!   end-state is the known-good state with no remaining unknown. Its
//!   failure mode (some allocation doesn't fit in SRAM) is a clean
//!   boot-time OOM panic — safe and obvious, not a subtle artifact.
//!
//! **Chosen: B.** It is the proven end-state, it fails safe, the SRAM
//! budget fits comfortably (the framebuffer stays in PSRAM regardless;
//! only the heap moves, and WiFi + embassy-net + mesh + crypto scratch
//! fit well inside the S3's 512 KB internal SRAM — this is the standard
//! architecture for this class of RGB-panel board), and crucially it
//! lets *this file* become the **simplest correct design** instead of
//! accreting bounce-buffer machinery.
//!
//! ### Required `main.rs` change (fix B)
//!
//! `esp_alloc::HEAP` is a multi-region heap. Fix B registers **two
//! regions with distinct capabilities** and orders them so the plain
//! global allocator (WiFi / embassy / mesh) only ever lands in
//! internal SRAM, while *this module* targets PSRAM explicitly via
//! `alloc_caps(External, …)`.
//!
//! Replace `esp_alloc::psram_allocator!(&peripherals.PSRAM,
//! esp_hal::psram)` with, in this order:
//!
//! ```ignore
//! // 1. Internal-SRAM region — the DEFAULT for the plain global
//! //    allocator. Registered FIRST so capability-less `alloc`
//! //    (WiFi/embassy/mesh) is served from SRAM, never PSRAM.
//! //    ~160 KB leaves room for static .bss + the working set.
//! esp_alloc::heap_allocator!(size: 160 * 1024);
//!
//! // 2. PSRAM region — tagged `External`. The framebuffer in this
//! //    module is the ONLY thing that requests it, via
//! //    `HEAP.alloc_caps(MemoryCapability::External.into(), …)`.
//! //    `psram_raw_parts` needs the `PSRAM` peripheral handle (that
//! //    is fine here in main.rs — it does NOT change DpiSurface's
//! //    public API).
//! let (psram_ptr, psram_len) =
//!     esp_hal::psram::psram_raw_parts(&peripherals.PSRAM);
//! unsafe {
//!     esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
//!         psram_ptr,
//!         psram_len,
//!         esp_alloc::MemoryCapability::External.into(),
//!     ));
//! }
//! ```
//!
//! The `psram` feature stays on so `esp_hal::init` maps PSRAM.
//! Crucially: a capability-less `alloc` (everything except this
//! module's framebuffer) is served from the `Internal` region, so
//! WiFi / embassy / mesh never touch PSRAM — the GDMA owns it.
//!
//! If 160 KB is too tight at boot (clean OOM panic — safe, obvious),
//! drop toward ~128 KB or trim WiFi/socket buffer sizing; if it still
//! will not fit, fall back to approach A.
//!
//! ### Why a plain circular ring drifts (config #9 result)
//!
//! Reading esp-hal 1.0's `lcd_cam/lcd/dpi.rs` and its own
//! `qa-test/src/bin/lcd_dpi.rs` example: `Dpi::send`'s `next_frame_en`
//! arg sets one register bit (`lcd_misc.lcd_next_frame_en`), which
//! makes the **LCD raster** re-run frame-to-frame — but it does
//! **not** re-anchor the GDMA. esp-hal's own example does **not**
//! free-run; it loops `send(false, buf) → wait() → refill → send`,
//! re-issuing the transfer every frame from software.
//!
//! So with a free-running circular GDMA ring + `next_frame_en = true`
//! (config #9), the LCD raster restarts every VSYNC but the GDMA keeps
//! walking the ring from wherever its read pointer happens to be. The
//! two free-run independently; a fixed offset accumulates per frame →
//! steady diagonal wrap. **An active per-frame GDMA re-anchor is
//! genuinely required on this HAL.**
//!
//! ### This module's design — config #10: parked chain + VSYNC re-arm
//!
//! 1. **Non-circular descriptor chain.** `FB_DESC[0..N-2]` each
//!    `next → i+1`; the **last descriptor's `next = null`**. esp-hal
//!    keys circular-vs-not off exactly this (`prepare_transfer`:
//!    `auto_write_back = !last.next.is_null()`). A null `next` makes
//!    the GDMA **stop** when it consumes the last descriptor — out-
//!    link parked, read pointer at end-of-framebuffer.
//! 2. **`suc_eof = true` on the last descriptor only** — it signals
//!    end-of-transfer to the GDMA so it parks cleanly at the frame
//!    boundary. Intermediates `suc_eof = false`.
//! 3. **`next_frame_en = true`** (`Dpi::send(true, ...)`) — the
//!    LCD_CAM raster keeps free-running frame-to-frame; it does not
//!    stop when the DMA parks (the LCD and the GDMA are independent
//!    state machines).
//! 4. **VSYNC ISR re-arms the parked GDMA every frame.** Pulse
//!    `out_conf0.out_rst` (flushes the now-**idle** FIFO — deterministic
//!    because the channel already stopped), set `out_link.addr =
//!    &FB_DESC[0]`, `out_link.start = 1`. The DMA re-walks the chain
//!    0..N-1, delivers exactly one framebuffer, parks again. One
//!    deterministic re-anchor per frame, against an idle channel.
//!
//! No `_dmadesc_restart` and no FIFO-pixel offset: those existed in
//! earlier porting attempts to compensate for FIFO contents while the
//! DMA was *running*. A genuinely parked DMA + `out_rst` leaves the
//! FIFO empty, so the re-arm points at the literal head `FB_DESC[0]`.
//!
//! ### Why not `send → wait → send`?
//!
//! esp-hal's `DpiTransfer::wait()` is `while !is_done() {
//! core::hint::spin_loop(); }` against a register — a hard busy-spin,
//! no `Future`, no async variant. Under embassy on a single core that
//! pegs ~98% CPU per frame (the active-frame busy-window) and starves
//! the executor — incompatible with the WiFi / embassy / mesh task
//! graph. The ISR approach (this config) is no-spin: ~5 register
//! writes per VSYNC interrupt, negligible CPU.
//!
//! ### Byte-count math (verified exact)
//!
//! The ring must deliver *exactly* `FB_W·FB_H·2 = 800·480·2 = 768000`
//! bytes per pass: `CHUNK_BYTES = ROWS_PER_DESC·FB_STRIDE_BYTES =
//! 2·1600 = 3200`, `FB_DESCRIPTORS = FB_H/ROWS_PER_DESC = 240`,
//! `240·3200 = 768000 = FB_BYTES` — exact, no rounding. `3200` ≤ the
//! 12-bit `size`/`length` field max (4095), and is a multiple of 4
//! (GDMA word alignment) and 64 (external-burst size). The
//! `debug_assert_eq!` in `new()` guards it.
//!
//! ### Recoverability — IMPORTANT
//!
//! This file is **not tracked in git**; prior configs are **not
//! `git checkout`-recoverable**. Config #10 vs config #9, what
//! changed (additive — all of fix B / `alloc_caps(External)` stays):
//!   (a) the descriptor chain is **non-circular again** —
//!       `FB_DESC[N-1].next = null`, `suc_eof` set on the last
//!       descriptor only. Config #9 had a circular ring (last `next`
//!       wrapped to head, `suc_eof` everywhere false).
//!   (b) the VSYNC ISR (`lcd_vsync_isr`) + its `HEAD_DESC_ADDR` /
//!       `GDMA_CH` statics are **re-introduced** (deleted in #9);
//!   (c) `DpiSurface::new` re-binds `Interrupt::LCD_CAM` and the
//!       `VsyncIrqBindFailed` error variant is **re-added**;
//!   (d) the `core::sync::atomic`, `handler`, `Priority`, `Interrupt`,
//!       `DMA`, `LCD_CAM` imports are **re-added**.
//! What did **not** change from #9: `core::alloc::Layout`,
//! `esp_alloc::MemoryCapability`, and the `alloc_caps(External)`
//! framebuffer allocation — fix B's PSRAM-only-for-framebuffer rule
//! stays, and `main.rs`'s two-region heap setup stays unchanged.
//!
//! To revert config #10 → config #9 (plain circular ring, no ISR): in
//! `CircularPsramFb::new` re-link the last descriptor's `next` back to
//! the head (`for i in 0..n { next = (i+1) % n }`), set `suc_eof =
//! false` on every descriptor (`new` AND `prepare`), delete the
//! `lcd_vsync_isr` fn + the `HEAD_DESC_ADDR` / `GDMA_CH` statics + the
//! `bind_interrupt`/`enable`/`lc_dma_int_ena` block in
//! `DpiSurface::new` + the `VsyncIrqBindFailed` variant + the
//! `core::sync::atomic`, `handler`, `Priority`, `Interrupt`, `DMA`,
//! `LCD_CAM` imports. Config #9 is the "uncontended-but-drifts"
//! baseline; do NOT revert past it (configs #1–#8 were all run on
//! the contention-poisoned firmware and are not meaningful baselines).
//!
//! ## Cache coherency
//!
//! The ESP32-S3 dcache is write-back. [`DpiSurface::present`] does an
//! explicit full-framebuffer `rom_Cache_WriteBack_Addr` (with the
//! autoload suspend/resume guard) so the GDMA's next pass reads the
//! pixels the compositor just drew. This was the config-#1 fix and is
//! kept verbatim. (Still required under fix B: the *framebuffer* is
//! still in PSRAM and still reached through the write-back dcache; only
//! the *heap* moved to SRAM.)
//!
//! ## Pixel format
//!
//! `LeafSurface::Frame` is `DrawTarget<Color = Rgb888>` but the DPI
//! panel is 16-bit RGB565. The framebuffer is stored RGB565 (768 KB)
//! and the `Frame` view converts `Rgb888 → Rgb565` on the fly. The
//! lossy narrowing happens once, at the bus boundary.

#![allow(dead_code)]

use core::alloc::Layout;
use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::Pixel;
use esp_alloc::MemoryCapability;
use esp_hal::dma::{
    BurstConfig, DmaDescriptor, DmaTxBuffer, ExternalBurstConfig, InternalBurstConfig, Owner,
    Preparation, TransferDirection,
};
use esp_hal::handler;
use esp_hal::interrupt::Priority;
use esp_hal::peripherals::{Interrupt, DMA, LCD_CAM};

use weftos_leaf_display::LeafSurface;
use weftos_leaf_types::DisplaySinkCap;

// ── Panel geometry ───────────────────────────────────────────────────

/// Active pixel width of the CrowPanel DIS08070H.
pub const FB_W: usize = 800;
/// Active pixel height.
pub const FB_H: usize = 480;

const FB_PIXELS: usize = FB_W * FB_H;
const FB_BYTES: usize = FB_PIXELS * 2; // RGB565 = 2 bytes/pixel

// 800 px * 2 B = 1600 B per row — already a 4-byte multiple, so no
// stride padding is required for this panel.
const FB_STRIDE_BYTES: usize = FB_W * 2;
const _: () = assert!(
    FB_STRIDE_BYTES % 4 == 0,
    "row stride must be 4-byte aligned for GDMA"
);

// Per-descriptor chunk: GDMA descriptors cap at 4095 bytes. A row-
// aligned chunk keeps the ring geometry obvious and avoids a descriptor
// boundary mid-row. 2 rows = 3200 B ≤ 4095.
const ROWS_PER_DESC: usize = 2;
const CHUNK_BYTES: usize = ROWS_PER_DESC * FB_STRIDE_BYTES; // 3200
const _: () = assert!(
    CHUNK_BYTES <= 4095,
    "GDMA descriptor chunk must be <= 4095 bytes"
);
const _: () = assert!(
    FB_H % ROWS_PER_DESC == 0,
    "FB_H must divide evenly into chunks"
);

/// Number of descriptors in the (non-circular) chain.
const FB_DESCRIPTORS: usize = FB_H / ROWS_PER_DESC; // 240

// GDMA descriptors must live in internal DRAM — `DmaDescriptor::next`
// "can only point to internal RAM" (esp-hal docs). A `static` lands in
// `.bss` (internal DRAM). `FB_DESC` is the non-circular chain (last
// `next = null` so the GDMA parks at framebuffer end).
//
// SAFETY: the only mutable access to this static is through
// `DpiSurface::new`, guarded call-once by `FB_TAKEN`. After that the
// GDMA owns it for the program's lifetime; no `&mut` is ever formed
// again. The VSYNC ISR re-arms the parked GDMA each frame.
static mut FB_DESC: [DmaDescriptor; FB_DESCRIPTORS] =
    [DmaDescriptor::EMPTY; FB_DESCRIPTORS];

// Call-once guard for `DpiSurface::new` / `CircularPsramFb::new`: the
// descriptor `static` and the PSRAM framebuffer region can only be
// handed out once.
//
// SAFETY: written once in `CircularPsramFb::new` under single-threaded
// init; thereafter only read.
static mut FB_TAKEN: bool = false;

// ── VSYNC ISR state (CONFIG-10: re-introduced, see module doc) ───────
//
// The ISR is a free `#[handler]` fn, so the data it needs lives in
// lock-free atomics published by `DpiSurface::new`.

/// Physical address of `FB_DESC[0]` — what the VSYNC ISR loads into the
/// GDMA out-link to re-arm the parked channel. `0` = not yet armed
/// (ISR no-ops).
static HEAD_DESC_ADDR: AtomicU32 = AtomicU32::new(0);

/// GDMA channel index the DPI transfer runs on (`main.rs` wires
/// `DMA_CH2` → `2`). `0xFF` = unset.
static GDMA_CH: AtomicU8 = AtomicU8::new(0xFF);

/// VSYNC ISR — re-arms the **parked** GDMA out-link once per frame.
///
/// The GDMA is genuinely *stopped* when this fires: the non-circular
/// chain's last descriptor has `next = null`, so the GDMA parked itself
/// after delivering the previous framebuffer. This handler therefore
/// re-arms an *idle* channel and `out_rst`-flushes an *idle* FIFO —
/// deterministic, no running-DMA fight.
///
/// Register sequence mirrors LovyanGFX `Bus_RGB.cpp`
/// `lcd_default_isr_handler`'s `out_rst` pulse + `out.link.addr` +
/// `out.link.start` — but re-pointed at the literal head `FB_DESC[0]`
/// rather than a FIFO-offset `_dmadesc_restart`, because the parked
/// DMA leaves the FIFO empty after `out_rst`.
///
/// SAFETY / real-time: touches only the `LCD_CAM` + `DMA` register-
/// block singletons (no allocation, no locks); the `AtomicU*` reads
/// are lock-free.
#[handler]
fn lcd_vsync_isr() {
    let lcd_cam = LCD_CAM::regs();

    // Read raw LCD_CAM DMA-int status, then clear exactly the bits that
    // were set (write-1-to-clear). Bits 0..1 are LCD_VSYNC +
    // LCD_TRANS_DONE.
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

    if !vsync {
        return;
    }

    let head = HEAD_DESC_ADDR.load(Ordering::Relaxed);
    let ch = GDMA_CH.load(Ordering::Relaxed);
    if head == 0 || ch == 0xFF {
        return;
    }

    let dma_ch = DMA::regs().ch(ch as usize);

    // Pulse the out-channel reset. The GDMA is already parked (the
    // chain's last `next` was null), so this flushes an *idle* FIFO.
    dma_ch.out_conf0().modify(|_, w| w.out_rst().set_bit());
    dma_ch.out_conf0().modify(|_, w| w.out_rst().clear_bit());

    // Re-point the out-link at the literal head of the chain and start
    // it. The DMA re-walks `FB_DESC[0..N-1]`, delivers exactly one
    // framebuffer, and parks again at the null `next`.
    //
    // SAFETY: `head` is the address of `FB_DESC[0]`, a valid `'static`
    // DRAM descriptor; the chain it heads is fully built and immutable
    // after `DpiSurface::new`.
    dma_ch.out_link().modify(|_, w| {
        unsafe { w.outlink_addr().bits(head) };
        w.outlink_start().set_bit()
    });
}

// ── RGB565 helper ────────────────────────────────────────────────────

/// Encode an (R8, G8, B8) tuple into packed 16-bit RGB565.
#[inline]
pub const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    let r5 = (r as u16 >> 3) & 0x1F;
    let g6 = (g as u16 >> 2) & 0x3F;
    let b5 = (b as u16 >> 3) & 0x1F;
    (r5 << 11) | (g6 << 5) | b5
}

/// Narrow an `embedded-graphics` `Rgb888` to packed RGB565.
#[inline]
fn rgb888_to_565(c: Rgb888) -> u16 {
    rgb565(c.r(), c.g(), c.b())
}

// ── Errors ───────────────────────────────────────────────────────────

/// Errors surfaced by [`DpiSurface`].
#[derive(Debug)]
pub enum DpiSurfaceError {
    /// `DpiSurface::new` was called more than once — the descriptor
    /// `static` + the PSRAM framebuffer can only be handed out once.
    AlreadyInitialised,
    /// `esp_alloc::HEAP.alloc_caps(External, …)` returned null — no
    /// `External` (PSRAM) heap region is registered, or it is too small
    /// for the 768 KB framebuffer. With fix B, PSRAM holds *only* the
    /// framebuffer, so on an 8 MB-PSRAM N8R8 part this means `main.rs`
    /// did not register the PSRAM region (`HEAP.add_region(...
    /// External ...)`) — see the fix-B section of the module doc.
    PsramTooSmall,
    /// The GDMA / DPI rejected the transfer at `Dpi::send` time.
    DmaSendFailed,
    /// The `Interrupt::LCD_CAM` vector could not be enabled for the
    /// VSYNC ISR — without it the GDMA parks after one frame and never
    /// re-arms (panel renders one frame then freezes / blacks), so
    /// `new` fails loud rather than shipping a dead display.
    VsyncIrqBindFailed,
}

// ── Circular PSRAM framebuffer (custom DmaTxBuffer) ──────────────────

/// A PSRAM-backed, multi-descriptor, **non-circular** DMA transmit
/// buffer.
///
/// esp-hal 1.0 has no stock buffer that is PSRAM-backed *and* large,
/// so this is a custom `DmaTxBuffer` impl: a chain of
/// [`FB_DESCRIPTORS`] descriptors over one PSRAM allocation. The last
/// descriptor's `next = null` and `suc_eof = true` — the GDMA parks
/// when it has delivered one framebuffer. The VSYNC ISR
/// ([`lcd_vsync_isr`]) re-arms the parked channel each frame.
///
/// Fix B (config #9, `main.rs`'s two-region heap): the framebuffer is
/// allocated from the PSRAM heap region via
/// `esp_alloc::HEAP.alloc_caps(External, …)`, **not** a plain `Vec` /
/// global `alloc`. The plain global allocator is served from internal
/// SRAM; only this `alloc_caps(External, …)` call targets PSRAM — so
/// PSRAM holds only this framebuffer, touched only by the GDMA. The
/// GDMA's real-time read is never contended (the static-grid
/// diagnostic proved an uncontended PSRAM read is rock-steady).
///
/// Frame-lock (config #10): a free-running circular ring would drift
/// (uncontended GDMA + uncontended LCD raster, but no coupling); the
/// non-circular parked chain + VSYNC ISR re-arm provides the per-frame
/// GDMA re-anchor esp-hal 1.0's DPI driver does not give for free.
/// One ~5-register-write ISR per VSYNC, no spin — embassy/WiFi/mesh
/// run unimpeded.
pub struct CircularPsramFb {
    /// Base of the PSRAM framebuffer, as `u16` for CPU pixel writes.
    fb_ptr: *mut u16,
    /// Pixel count (`FB_W * FB_H`).
    fb_len: usize,
    /// The (non-circular) descriptor chain (borrowed `'static` from
    /// `FB_DESC`).
    descriptors: &'static mut [DmaDescriptor],
}

impl CircularPsramFb {
    /// Allocate the framebuffer from the PSRAM heap region and build
    /// the non-circular descriptor chain.
    ///
    /// Call once, after `esp_hal::init` (which maps PSRAM when the
    /// `psram` feature is on).
    fn new() -> Result<Self, DpiSurfaceError> {
        // SAFETY: single-threaded init path (called from `main` before
        // any task that touches the display is spawned).
        unsafe {
            if FB_TAKEN {
                return Err(DpiSurfaceError::AlreadyInitialised);
            }
            FB_TAKEN = true;
        }

        // Allocate the framebuffer from the PSRAM heap region via a
        // capability-targeted allocation, NOT a plain
        // `Vec` / global `alloc` (which, under fix B, is internal
        // SRAM). `esp_alloc::HEAP` is a multi-region heap; `main.rs`'s
        // fix B registers an internal-SRAM region (`Internal`) AND a
        // PSRAM region (`External`). `alloc_caps(External, …)` carves
        // the framebuffer from the PSRAM region specifically — so the
        // framebuffer is in PSRAM, while every *other* allocation
        // (WiFi / embassy / mesh, which use the plain global allocator)
        // lands in the `Internal` region and never touches PSRAM. The
        // GDMA then owns PSRAM uncontended.
        //
        // `alloc_caps` needs no peripheral handle (unlike
        // `esp_hal::psram::psram_raw_parts`, which takes `&PSRAM` and
        // would force a `DpiSurface::new` signature change) — so the
        // public API is unchanged.
        //
        // 64-byte (cache-line) alignment: esp-hal's own
        // `prepare_transfer` rejects a PSRAM descriptor buffer whose
        // address or size is not dcache-line aligned
        // (`DmaError::InvalidAlignment`). `FB_BYTES` (768000) is
        // already a multiple of 64; request 64-byte alignment so the
        // base is too.
        const CACHE_LINE: usize = 64;
        let layout = match Layout::from_size_align(FB_BYTES, CACHE_LINE) {
            Ok(l) => l,
            Err(_) => return Err(DpiSurfaceError::PsramTooSmall),
        };
        // SAFETY: `layout` has non-zero size (`FB_BYTES` is 768000).
        // `alloc_caps` returns a pointer into the `External` (PSRAM)
        // heap region or null on failure.
        let raw =
            unsafe { esp_alloc::HEAP.alloc_caps(MemoryCapability::External.into(), layout) };
        if raw.is_null() {
            // No PSRAM region registered, or it is too small — under
            // fix B PSRAM holds only this framebuffer, so on an N8R8
            // part this means `main.rs` did not register the PSRAM
            // region (the `External` heap region) at all.
            return Err(DpiSurfaceError::PsramTooSmall);
        }
        // The framebuffer is leaked deliberately — never freed; the
        // GDMA scans it for the program's entire lifetime.
        let fb_ptr = raw as *mut u16;

        // SAFETY: call-once guarded above; this is the only `&mut` to
        // `FB_DESC`.
        let descriptors: &'static mut [DmaDescriptor] =
            unsafe { &mut *core::ptr::addr_of_mut!(FB_DESC) };

        // Build the chain. Each descriptor covers CHUNK_BYTES of the
        // PSRAM framebuffer; descriptor `i`'s `next` → `i+1`.
        //
        // CONFIG-10: the LAST descriptor's `next` is left **null** and
        // its `suc_eof` is set — this makes the GDMA park when it
        // finishes the framebuffer (esp-hal keys non-circular off a
        // null last `next`). Intermediates: `next → i+1`, `suc_eof =
        // false`. The VSYNC ISR re-arms the parked channel each frame.
        //
        // `owner = DMA` on every descriptor.
        let n = descriptors.len();
        for i in 0..n {
            let chunk_off = i * CHUNK_BYTES;
            // SAFETY: chunk_off + CHUNK_BYTES <= FB_BYTES by
            // construction (FB_DESCRIPTORS * CHUNK_BYTES == FB_BYTES),
            // and the framebuffer alloc was bounds-checked above.
            let chunk_ptr = unsafe { (fb_ptr as *mut u8).add(chunk_off) };

            let d = &mut descriptors[i];
            d.set_owner(Owner::Dma);
            // EOF only on the final descriptor — it parks the GDMA.
            d.set_suc_eof(i == n - 1);
            d.set_size(CHUNK_BYTES);
            d.set_length(CHUNK_BYTES);
            d.buffer = chunk_ptr;
            d.next = core::ptr::null_mut(); // linked in the next pass
        }
        for i in 0..n - 1 {
            // CONFIG-10: link 0..N-2 → next; descriptor N-1 keeps the
            // null `next` set above (non-circular → GDMA parks at EOF).
            let next_ptr: *mut DmaDescriptor = &mut descriptors[i + 1];
            descriptors[i].next = next_ptr;
        }

        // Sanity: the chain must exactly tile the framebuffer, and the
        // last descriptor must terminate the chain (null `next`).
        debug_assert_eq!(n * CHUNK_BYTES, FB_BYTES);
        debug_assert!(descriptors[n - 1].next.is_null());

        // Publish the head-descriptor address for the VSYNC ISR.
        // (DRAM is identity-mapped on the S3 — pointer value == addr.)
        HEAD_DESC_ADDR.store(
            (&raw const descriptors[0]) as u32,
            Ordering::Relaxed,
        );

        // Zero the framebuffer (PSRAM comes up undefined). Direct CPU
        // writes through the dcache; the explicit writeback below
        // pushes them to PSRAM before the GDMA's first read.
        // SAFETY: `fb_ptr .. fb_ptr + FB_PIXELS` is the just-carved,
        // bounds-checked PSRAM framebuffer region.
        unsafe {
            core::ptr::write_bytes(fb_ptr, 0, FB_PIXELS);
        }

        let me = Self {
            fb_ptr,
            fb_len: FB_PIXELS,
            descriptors,
        };
        me.writeback();
        Ok(me)
    }

    /// Base address of the PSRAM framebuffer — for alignment
    /// diagnostics (`base_addr() % 64`).
    #[inline]
    fn base_addr(&self) -> usize {
        self.fb_ptr as usize
    }

    /// Mutable `u16` pixel view of the framebuffer.
    #[inline]
    fn pixels(&mut self) -> &mut [u16] {
        // SAFETY: `fb_ptr`/`fb_len` describe the valid `'static` PSRAM
        // framebuffer region carved in `new`.
        unsafe { core::slice::from_raw_parts_mut(self.fb_ptr, self.fb_len) }
    }

    /// Write one RGB565 pixel, bounds-checked.
    #[inline]
    fn put_pixel(&mut self, x: usize, y: usize, raw: u16) {
        if x < FB_W && y < FB_H {
            // SAFETY: bounds-checked; `y * FB_W + x < FB_PIXELS`.
            unsafe { self.fb_ptr.add(y * FB_W + x).write(raw) };
        }
    }

    /// Explicit dcache writeback of the entire framebuffer.
    ///
    /// The ESP32-S3 dcache is write-back: CPU pixel writes sit in cache
    /// and do not reach PSRAM until flushed. Mirrors
    /// `lcd_rgb.rs::Framebuffer::flush`; the autoload suspend/resume
    /// around the writeback is load-bearing. Still required under fix
    /// B — the framebuffer is still in PSRAM, still reached through the
    /// write-back dcache; only the heap moved to SRAM.
    fn writeback(&self) {
        unsafe extern "C" {
            fn rom_Cache_WriteBack_Addr(addr: u32, size: u32);
            fn Cache_Suspend_DCache_Autoload() -> u32;
            fn Cache_Resume_DCache_Autoload(value: u32);
        }
        // SAFETY: `fb_ptr .. fb_ptr + FB_BYTES` is the valid `'static`
        // PSRAM framebuffer region; the suspend/resume pair is balanced.
        unsafe {
            let autoload = Cache_Suspend_DCache_Autoload();
            rom_Cache_WriteBack_Addr(self.fb_ptr as u32, FB_BYTES as u32);
            Cache_Resume_DCache_Autoload(autoload);
        }
    }
}

// SAFETY: `CircularPsramFb` keeps the `'static` PSRAM framebuffer
// region and the `'static` descriptor ring alive for the program's
// entire lifetime.
unsafe impl DmaTxBuffer for CircularPsramFb {
    type View = CircularPsramFb;
    type Final = CircularPsramFb;

    fn prepare(&mut self) -> Preparation {
        // Re-arm (idempotent per the trait contract). Re-assert
        // owner/eof/length so a re-`prepare` is well-defined.
        //
        // CONFIG-10: `suc_eof = true` on the LAST descriptor only — it
        // parks the GDMA at the framebuffer boundary. The `next`
        // linkage (including the null last `next`) was built in `new()`
        // and is left untouched here.
        let n = self.descriptors.len();
        for i in 0..n {
            let d = &mut self.descriptors[i];
            d.set_owner(Owner::Dma);
            d.set_suc_eof(i == n - 1);
            d.set_size(CHUNK_BYTES);
            d.set_length(CHUNK_BYTES);
        }

        Preparation {
            start: &mut self.descriptors[0],
            direction: TransferDirection::Out,
            // The framebuffer is in PSRAM — the GDMA must enable its
            // external-memory access path.
            accesses_psram: true,
            // Burst over PSRAM, 64-byte external burst (cache-line
            // sized, divides CHUNK_BYTES 3200/64 = 50 cleanly). The
            // wide burst is what lets the GDMA sustain the LCD's
            // ~24 MB/s now that PSRAM is uncontended.
            burst_transfer: BurstConfig {
                external_memory: ExternalBurstConfig::Size64,
                internal_memory: InternalBurstConfig::Disabled,
            },
            // Owner bits set once and never handed back to the CPU;
            // the GDMA must not wrap-check.
            check_owner: Some(false),
            // The ISR re-points the out-link at a freshly-built head
            // every frame, so descriptor write-back is neither needed
            // nor wanted — keep it off.
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

// ── Frame view: the Rgb888 DrawTarget the compositor draws into ──────

/// The back-buffer draw handle returned by [`DpiSurface::frame`].
///
/// Implements `DrawTarget<Color = Rgb888>`; every pixel is narrowed
/// `Rgb888 → Rgb565` on the way into the PSRAM framebuffer. Borrows the
/// surface for the frame's duration — `present()` cannot be called
/// while a `DpiFrame` is alive.
pub struct DpiFrame<'a> {
    fb: &'a mut CircularPsramFb,
}

impl OriginDimensions for DpiFrame<'_> {
    fn size(&self) -> Size {
        Size::new(FB_W as u32, FB_H as u32)
    }
}

impl DrawTarget for DpiFrame<'_> {
    type Color = Rgb888;
    type Error = DpiSurfaceError;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels {
            if let (Ok(x), Ok(y)) =
                (usize::try_from(coord.x), usize::try_from(coord.y))
            {
                self.fb.put_pixel(x, y, rgb888_to_565(color));
            }
        }
        Ok(())
    }

    fn fill_solid(
        &mut self,
        area: &Rectangle,
        color: Self::Color,
    ) -> Result<(), Self::Error> {
        let raw = rgb888_to_565(color);
        let area = area.intersection(&self.bounding_box());
        for y in area.rows() {
            for x in area.columns() {
                self.fb.put_pixel(x as usize, y as usize, raw);
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        let raw = rgb888_to_565(color);
        self.fb.pixels().fill(raw);
        Ok(())
    }
}

// ── The LeafSurface implementation ───────────────────────────────────

/// `LeafSurface` for the CrowPanel 800×480 RGB-parallel panel.
///
/// Owns the PSRAM framebuffer + non-circular descriptor chain; the
/// live DMA transfer is `forget`-leaked inside [`DpiSurface::new`].
/// The GDMA parks after each framebuffer; the VSYNC ISR re-arms it
/// once per frame — locking the GDMA read pointer to the LCD frame
/// start. Fix B (`main.rs`, two-region heap) ensures PSRAM is
/// uncontended so the GDMA can sustain the LCD's real-time read.
pub struct DpiSurface {
    /// The PSRAM framebuffer + circular ring. After `Dpi::send` the
    /// `DmaTxBuffer::View` (which *is* `CircularPsramFb`) is inside the
    /// leaked `DpiTransfer`; this is a draw-only handle over the same
    /// PSRAM framebuffer region with an empty descriptor slice — both
    /// alias the one region (deliberate single-buffering).
    fb: CircularPsramFb,
    /// Brightness backlight on-time, last value set (diagnostic only).
    backlight_on_us: u32,
}

impl DpiSurface {
    /// Build the surface and start the circular GDMA scan.
    ///
    /// `dpi` must already be fully pin-wired and configured — the
    /// caller (`main.rs`) owns the `DpiConfig` + `.with_data*()` chain.
    /// This function carves the PSRAM framebuffer out of the PSRAM
    /// region, builds the circular ring, hands it to `dpi.send(true /*
    /// next_frame_en */, ...)`, and leaks the transfer.
    ///
    /// The VSYNC ISR re-arms the parked GDMA each frame — providing
    /// the per-frame re-anchor esp-hal 1.0's DPI driver does not give
    /// for free. Fix B (`main.rs`'s two-region heap) leaves PSRAM
    /// uncontended so the GDMA can sustain the LCD's real-time read.
    ///
    /// `Dm` is the `Dpi`'s driver mode. Confirmed against esp-hal
    /// 1.0.0: `Dpi<'d, Dm: DriverMode>`, `esp_hal::DriverMode` is
    /// re-exported at the crate root.
    pub fn new<'d, Dm>(
        dpi: esp_hal::lcd_cam::lcd::dpi::Dpi<'d, Dm>,
    ) -> Result<Self, DpiSurfaceError>
    where
        Dm: esp_hal::DriverMode,
    {
        // 1. Carve the PSRAM framebuffer + build the circular ring.
        let fb_for_dma = CircularPsramFb::new()?;
        let fb_ptr = fb_for_dma.fb_ptr;
        let fb_len = fb_for_dma.fb_len;

        // 2. Kick the transfer with `next_frame_en = true` — the
        //    LCD_CAM raster keeps free-running frame-to-frame. The
        //    GDMA delivers one framebuffer then parks at the null last
        //    `next`; the VSYNC ISR (armed below) re-arms it each frame.
        //    Leak the transfer — dropping it cancels the DMA.
        match dpi.send(true, fb_for_dma) {
            Ok(transfer) => {
                core::mem::forget(transfer);
            }
            Err(_) => {
                // `Dpi::send` → `(DmaError, Dpi, TX)` on failure; all
                // dropped here. `FB_TAKEN` stays set.
                return Err(DpiSurfaceError::DmaSendFailed);
            }
        }

        // 3. Arm the VSYNC ISR.
        //
        //    `main.rs` wires the DPI to `peripherals.DMA_CH2`, so the
        //    GDMA out-link the ISR re-arms is channel 2.
        //
        //    VERIFY: that `DMA_CH2` is GDMA channel index 2. On the S3
        //    the `DMA_CHn` peripheral singletons map 1:1 to GDMA
        //    channel indices. esp-hal 1.0 does not expose the channel
        //    index off a constructed `Dpi`, so this is an explicit
        //    constant that must track `main.rs`.
        const GDMA_CHANNEL: u8 = 2;
        GDMA_CH.store(GDMA_CHANNEL, Ordering::Relaxed);

        // Bind the handler to `Interrupt::LCD_CAM`. esp-hal's own
        // `LcdCam::set_interrupt_handler` does the identical
        // `bind_interrupt → enable` dance for this vector.
        //
        // SAFETY: `bind_interrupt` installs `lcd_vsync_isr` (a
        // `#[handler]`-wrapped `extern "C" fn`) for `Interrupt::LCD_CAM`.
        // The handler only touches stolen register-block singletons and
        // lock-free atomics.
        unsafe {
            esp_hal::interrupt::bind_interrupt(
                Interrupt::LCD_CAM,
                lcd_vsync_isr.handler(),
            );
        }
        if esp_hal::interrupt::enable(Interrupt::LCD_CAM, Priority::Priority2)
            .is_err()
        {
            return Err(DpiSurfaceError::VsyncIrqBindFailed);
        }

        // Enable the LCD_VSYNC interrupt source in the LCD_CAM block.
        // Done last, so the ISR only fires once `HEAD_DESC_ADDR` +
        // `GDMA_CH` are both published.
        LCD_CAM::regs()
            .lc_dma_int_ena()
            .modify(|_, w| w.lcd_vsync_int_ena().set_bit());

        // 4. Draw-only handle over the same PSRAM framebuffer region.
        //    Does NOT own a descriptor ring (the live transfer does);
        //    `descriptors` is an empty slice so nothing aliases
        //    `FB_DESC`. Only `fb_ptr`/`fb_len` are used by the draw
        //    path.
        //
        // SAFETY: `fb_ptr`/`fb_len` describe the `'static` PSRAM
        // framebuffer region; it outlives `DpiSurface`. The empty
        // descriptor slice is a valid `&'static mut [_]` (length 0).
        let fb = CircularPsramFb {
            fb_ptr,
            fb_len,
            descriptors: &mut [],
        };

        Ok(Self {
            fb,
            backlight_on_us: 0,
        })
    }

    /// Base address of the PSRAM framebuffer — for the `align%64`
    /// diagnostic line `main.rs` likes to print.
    #[inline]
    pub fn framebuffer_addr(&self) -> usize {
        self.fb.base_addr()
    }
}

impl LeafSurface for DpiSurface {
    type Frame<'a> = DpiFrame<'a>;
    type Error = DpiSurfaceError;

    fn capability(&self) -> DisplaySinkCap {
        DisplaySinkCap {
            width: FB_W as u32,
            height: FB_H as u32,
            // RGB565 is what the panel actually consumes; the compositor
            // draws Rgb888 and the `Frame` narrows on the fly.
            pixel_format: alloc::string::String::from("rgb565"),
            // Single-buffered hardware surface: one composited layer
            // stack, blended by the compositor above this trait.
            layers: 1,
            blend_modes: alloc::vec::Vec::new(),
        }
    }

    fn frame(&mut self) -> Self::Frame<'_> {
        DpiFrame { fb: &mut self.fb }
    }

    fn present(&mut self) -> Result<(), Self::Error> {
        // Single-buffered direct-scan: "present" is pushing the dcache
        // to PSRAM so the GDMA's next pass reads the pixels the
        // compositor just drew. The non-circular chain + the VSYNC ISR
        // keep the panel fed and frame-locked on their own — there is
        // no `send` to re-kick from here. Returns immediately; tearing
        // is the accepted single-buffer tradeoff.
        //
        // Production fix (not done — spike scope): double-buffer (two
        // PSRAM framebuffers + two rings, swap which ring `Dpi::send`
        // scans per frame).
        self.fb.writeback();
        Ok(())
    }

    fn set_brightness(&mut self, on_us: u32) -> Result<(), Self::Error> {
        // The backlight is GPIO 2 (`board::LCD_BACKLIGHT`), driven by
        // `main.rs`. This surface does not own that pin, so: record the
        // request for a future PWM hookup and accept it (the trait's
        // default is also a no-op `Ok`).
        self.backlight_on_us = on_us;
        Ok(())
    }
}
