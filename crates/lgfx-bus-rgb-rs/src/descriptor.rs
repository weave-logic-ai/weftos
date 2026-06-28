// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-3-Clause
//
// Derived from LovyanGFX (https://github.com/lovyan03/LovyanGFX) —
// copyright lovyan03 et al., BSD-3-Clause (FreeBSD).
//
// Source: components/LovyanGFX-master/src/lgfx/v1/platforms/esp32s3/Bus_RGB.cpp

//! Circular GDMA descriptor chain + FIFO-skip restart descriptor.
//!
//! Direct port of Bus_RGB.cpp:195-225. The C++ allocates the
//! framebuffer + a single linear array of `dma_descriptor_t`, walks
//! the array assigning per-chunk `buffer` / `length` / `size`, links
//! them, then wraps the last `next` back to the head; finally it
//! builds a second descriptor (`_dmadesc_restart`) which is a copy of
//! the head with its buffer pointer advanced by
//! `(L2FIFO_BASE_SIZE + 1) * pixel_bytes` so VSYNC re-arms skip the
//! LCD_CAM async output FIFO's pre-fetch.
//!
//! ## Why the skip exists (do not delete)
//!
//! On every VSYNC the ISR pulses `out_conf0.out_rst` to force-stop
//! the GDMA, then re-points the out-link at the restart descriptor.
//! `out_rst` flushes the **GDMA-side L2 FIFO** but **NOT** the
//! downstream LCD_CAM-side async output FIFO. That FIFO carries ~64
//! bytes of pre-fetched data forward across the re-arm; if the
//! restart descriptor pointed at the literal head of the framebuffer
//! the panel would see pixels `64+1..N` of row 0 displayed where
//! pixels `0..N-65` should be, producing a fine pixel-phase shift.
//! Skipping `(L2FIFO_BASE_SIZE + 1) * pixel_bytes = 130` bytes (65
//! pixels at RGB565) on the restart compensates exactly for the
//! pre-fetched bytes the LCD_CAM FIFO already holds.
//!
//! `L2FIFO_BASE_SIZE = 64` is a private constant from ESP-IDF's
//! `hal/gdma_ll.h`; it is not in the public TRM.

use core::ptr;

use esp_hal::dma::DmaDescriptor;

/// `GDMA_LL_L2FIFO_BASE_SIZE` from ESP-IDF's `hal/gdma_ll.h`. The
/// LCD_CAM-side async output FIFO holds 64 bytes that survive an
/// `out_rst`; the restart descriptor must skip that many bytes plus
/// one pixel boundary.
///
/// Bus_RGB.cpp:221 `int skip_bytes = (GDMA_LL_L2FIFO_BASE_SIZE + 1) * pixel_bytes;`
pub const GDMA_LL_L2FIFO_BASE_SIZE: usize = 64;

/// Maximum bytes per GDMA descriptor.
///
/// Bus_RGB.cpp:198 `static constexpr size_t MAX_DMA_LEN = (4096-64);`
/// — the hardware allows 4095 (12 bits) but the LovyanGFX reference
/// uses `4096 - 64` to leave headroom for burst-aligned ends. We
/// preserve the value verbatim.
pub const MAX_DMA_LEN: usize = 4096 - 64;

/// Compute the number of descriptors required to tile a framebuffer.
///
/// Bus_RGB.cpp:199
/// `size_t dmadesc_size = (fb_len - 1) / MAX_DMA_LEN + 1;`
#[inline]
pub const fn descriptor_count(fb_len: usize) -> usize {
    if fb_len == 0 {
        0
    } else {
        (fb_len - 1) / MAX_DMA_LEN + 1
    }
}

/// Build the circular descriptor chain over `fb_base..fb_base+fb_len`.
///
/// Mirrors Bus_RGB.cpp:203-217. After this function returns:
///
/// - `descriptors[0..n-1].next = &descriptors[i+1]`
/// - `descriptors[n-1].next   = &descriptors[0]` (circular)
/// - intermediate descriptors carry `(MAX_DMA_LEN, MAX_DMA_LEN, 0x80000000)`
///   — owner=DMA, suc_eof=false, size=length=MAX_DMA_LEN
/// - the final descriptor carries the trailing chunk size, with the
///   `0xC0000000` flag pattern from the C++ (owner=DMA + suc_eof=true)
///
/// The Rust HAL exposes `DmaDescriptor` as a struct of bit-fields
/// reached through `set_owner` / `set_suc_eof` / `set_length` /
/// `set_size`, not via the C++ `*(uint32_t*)dmadesc = ...` direct
/// dword write. Both encode the same bits.
///
/// # Safety
///
/// - `descriptors.len()` must equal [`descriptor_count(fb_len)`].
/// - `fb_base..fb_base+fb_len` must be a valid, `'static`-lived
///   buffer that outlives the GDMA transfer (typically PSRAM).
/// - The caller must keep `descriptors` in DRAM (esp-hal will panic
///   otherwise — `DmaDescriptor::next` must be a DRAM-resident
///   pointer).
pub unsafe fn build_circular_chain(
    descriptors: &mut [DmaDescriptor],
    fb_base: *mut u8,
    fb_len: usize,
) {
    use esp_hal::dma::Owner;

    let n = descriptors.len();
    debug_assert_eq!(n, descriptor_count(fb_len));

    // Bus_RGB.cpp:203-212 — walk all but the last descriptor, each
    // covering MAX_DMA_LEN bytes, linked head→tail.
    let mut data = fb_base;
    let mut remaining = fb_len;
    let mut i = 0;
    while remaining > MAX_DMA_LEN {
        remaining -= MAX_DMA_LEN;
        let d = &mut descriptors[i];
        d.set_owner(Owner::Dma);
        d.set_suc_eof(false); // 0x80000000 → owner only, no eof
        d.set_size(MAX_DMA_LEN);
        d.set_length(MAX_DMA_LEN);
        d.buffer = data;
        // next link assigned in the second pass below
        d.next = ptr::null_mut();
        // SAFETY: `data` is incremented within the bounded
        // framebuffer region; `remaining` tracks the remaining bytes.
        data = unsafe { data.add(MAX_DMA_LEN) };
        i += 1;
    }

    // Bus_RGB.cpp:213-215 — the trailing descriptor. Length is the
    // unrounded remainder; size is rounded up to 4 bytes for GDMA
    // word alignment. `0xC0000000` = owner=DMA + suc_eof=true in the
    // C++; we set the same via `set_suc_eof(true)`.
    {
        let d = &mut descriptors[i];
        d.set_owner(Owner::Dma);
        d.set_suc_eof(true);
        // Bus_RGB.cpp:213 `((len + 3) & (~3)) | len << 12 | 0xC0000000`
        //   ↑size in low 12      ↑length in next 12
        let size_aligned = (remaining + 3) & !3;
        d.set_size(size_aligned);
        d.set_length(remaining);
        d.buffer = data;
        d.next = ptr::null_mut();
    }

    // Bus_RGB.cpp:215 — circular link: every descriptor's `next`
    // points at the following one; the last wraps to the head.
    //
    //     dmadesc->next = _dmadesc;   // wrap
    //
    // The C++ assigns intermediate `next` pointers as it walks the
    // loop (`dmadesc->next = dmadesc + 1; dmadesc++;`); we do it
    // here in a separate pass so all `descriptors[]` addresses are
    // stable.
    for j in 0..n {
        let next_idx = (j + 1) % n;
        // SAFETY: `descriptors[next_idx]` is a live `&mut [_]`
        // element; `&mut descriptors[next_idx]` would alias, so we
        // take a raw pointer through addr_of_mut on the slice.
        let next_ptr: *mut DmaDescriptor =
            unsafe { descriptors.as_mut_ptr().add(next_idx) };
        descriptors[j].next = next_ptr;
    }
}

/// Build the FIFO-skip restart descriptor.
///
/// Direct port of Bus_RGB.cpp:220-225:
/// ```c
/// memcpy(&_dmadesc_restart, _dmadesc, sizeof(_dmadesc_restart));
/// int skip_bytes = (GDMA_LL_L2FIFO_BASE_SIZE + 1) * pixel_bytes;
/// auto p = (uint8_t*)(_dmadesc_restart.buffer);
/// _dmadesc_restart.buffer = &p[skip_bytes];
/// _dmadesc_restart.dw0.length -= skip_bytes;
/// _dmadesc_restart.dw0.size   -= skip_bytes;
/// ```
///
/// On return `restart` is a standalone descriptor pointing
/// `skip_bytes` into the framebuffer; its `next` is copied from the
/// head descriptor so the ring continues normally after the offset
/// chunk is delivered.
///
/// # Safety
///
/// - `descriptors[0]` must already be initialised by
///   [`build_circular_chain`]; this function reads its fields.
/// - `pixel_bytes` must match the value used to size the framebuffer.
pub unsafe fn build_restart_descriptor(
    restart: &mut DmaDescriptor,
    descriptors: &[DmaDescriptor],
    pixel_bytes: usize,
) {
    // Bus_RGB.cpp:220 `memcpy(&_dmadesc_restart, _dmadesc, sizeof(...))`
    //
    // esp-hal's `DmaDescriptor` is `Copy`; a direct assignment is the
    // structural equivalent of `memcpy`.
    *restart = descriptors[0];

    // Bus_RGB.cpp:221 `int skip_bytes = (GDMA_LL_L2FIFO_BASE_SIZE + 1) * pixel_bytes;`
    let skip_bytes = (GDMA_LL_L2FIFO_BASE_SIZE + 1) * pixel_bytes;

    // Bus_RGB.cpp:222-223
    //   auto p = (uint8_t*)(_dmadesc_restart.buffer);
    //   _dmadesc_restart.buffer = &p[skip_bytes];
    //
    // SAFETY: descriptors[0].buffer is the head of the framebuffer
    // and was sized to at least MAX_DMA_LEN > skip_bytes (130) bytes.
    restart.buffer = unsafe { restart.buffer.add(skip_bytes) };

    // Bus_RGB.cpp:224-225
    //   _dmadesc_restart.dw0.length -= skip_bytes;
    //   _dmadesc_restart.dw0.size   -= skip_bytes;
    //
    // esp-hal does not expose `length` / `size` as readable fields,
    // only setters that take the desired final value. The head
    // descriptor as built by `build_circular_chain` always covers
    // MAX_DMA_LEN bytes (since fb_len ≫ MAX_DMA_LEN on any real
    // panel) — assert and use that value.
    debug_assert!(
        descriptor_count_yields_head_of_max_dma_len(descriptors.len()),
        "restart descriptor assumes the head descriptor is MAX_DMA_LEN-sized"
    );
    restart.set_length(MAX_DMA_LEN - skip_bytes);
    restart.set_size(MAX_DMA_LEN - skip_bytes);

    // The `next` field was copied from descriptors[0] above and
    // already points at descriptors[1] — the ring continues normally
    // after the offset chunk. (Bus_RGB.cpp:220's `memcpy` carries it
    // implicitly; we are explicit.)
}

/// Sanity helper: `build_restart_descriptor` assumes the head
/// descriptor of the ring is the full `MAX_DMA_LEN`-sized one
/// (i.e. the ring has at least two descriptors). Single-descriptor
/// rings would mean `fb_len <= MAX_DMA_LEN`, which is below the
/// minimum framebuffer of any RGB-DPI panel we care about.
#[inline]
const fn descriptor_count_yields_head_of_max_dma_len(n: usize) -> bool {
    n >= 2
}
