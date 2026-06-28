//! Integration tests for `CapabilityMask` flag composition + display.

use weftos_leaf_renderer::CapabilityMask;

#[test]
fn empty_is_dpi_baseline() {
    let m = CapabilityMask::empty();
    assert!(!m.has_alpha());
    assert!(!m.has_blend_modes());
    assert!(!m.is_antialiased());
    assert!(!m.contains(CapabilityMask::SUBPIXEL));
}

#[test]
fn sim_default_composition() {
    let m = CapabilityMask::ALPHA
        | CapabilityMask::SUBPIXEL
        | CapabilityMask::ANTIALIASED
        | CapabilityMask::BLEND_MODES;
    assert!(m.has_alpha());
    assert!(m.has_blend_modes());
    assert!(m.is_antialiased());
}

#[test]
fn canvas_default_composition_adds_png() {
    let m = CapabilityMask::ALPHA
        | CapabilityMask::SUBPIXEL
        | CapabilityMask::ANTIALIASED
        | CapabilityMask::BLEND_MODES
        | CapabilityMask::BITMAP_PNG;
    assert!(m.contains(CapabilityMask::BITMAP_PNG));
}

#[test]
fn all_returns_every_flag() {
    let m = CapabilityMask::all();
    assert!(m.contains(CapabilityMask::ALPHA));
    assert!(m.contains(CapabilityMask::SUBPIXEL));
    assert!(m.contains(CapabilityMask::ANTIALIASED));
    assert!(m.contains(CapabilityMask::VECTOR_FONTS));
    assert!(m.contains(CapabilityMask::BITMAP_QOI));
    assert!(m.contains(CapabilityMask::BITMAP_PNG));
    assert!(m.contains(CapabilityMask::BLEND_MODES));
    assert!(m.contains(CapabilityMask::ANIMATION));
    assert!(m.contains(CapabilityMask::HIT_TEST_PATH));
}

#[test]
fn flags_are_disjoint_bits() {
    // Each flag's bit pattern is a power of two — Phase B relies on
    // composition via bitwise OR, so this is a load-bearing invariant.
    for f in [
        CapabilityMask::ALPHA,
        CapabilityMask::SUBPIXEL,
        CapabilityMask::ANTIALIASED,
        CapabilityMask::VECTOR_FONTS,
        CapabilityMask::BITMAP_QOI,
        CapabilityMask::BITMAP_PNG,
        CapabilityMask::BLEND_MODES,
        CapabilityMask::ANIMATION,
        CapabilityMask::HIT_TEST_PATH,
    ] {
        let bits = f.bits();
        assert!(bits.is_power_of_two(), "{f:?} bits {bits:#x} not pow2");
    }
}

#[test]
fn debug_shows_names() {
    let m = CapabilityMask::ALPHA | CapabilityMask::SUBPIXEL;
    let s = format!("{m:?}");
    assert!(s.contains("ALPHA"));
    assert!(s.contains("SUBPIXEL"));
}
