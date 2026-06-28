//! Integration tests for the LRU glyph cache.
//!
//! Mirrors the spec acceptance: hit/miss counters, size cap, eviction
//! order. Unit tests in `src/glyph_cache.rs` cover the same surface;
//! these duplicate the public-API path to make sure the cache works
//! through the crate's re-exports too.

use weftos_leaf_renderer::{Glyph, GlyphCache, GlyphKey};
use weftos_leaf_scene::{BuiltinFont, FontFace};

fn key(face: BuiltinFont, ch: char, size: u16) -> GlyphKey {
    GlyphKey {
        face: FontFace::Builtin(face),
        ch: ch as u32,
        size_q8: size,
    }
}

fn glyph(bytes: usize) -> Glyph {
    Glyph {
        w: 6,
        h: 10,
        advance_q8: 6 << 8,
        bitmap: vec![0u8; bytes],
    }
}

#[test]
fn empty_cache_misses() {
    let mut c = GlyphCache::new(1024);
    assert!(c.get(&key(BuiltinFont::Mono6x10, 'a', 256)).is_none());
    assert_eq!(c.stats().misses, 1);
    assert_eq!(c.stats().hits, 0);
}

#[test]
fn hit_after_insert() {
    let mut c = GlyphCache::new(1024);
    c.insert(key(BuiltinFont::Mono6x10, 'a', 256), glyph(60));
    let _ = c.get(&key(BuiltinFont::Mono6x10, 'a', 256)).expect("hit");
    assert_eq!(c.stats().hits, 1);
    assert_eq!(c.stats().misses, 0);
}

#[test]
fn cache_keys_by_face_and_size() {
    let mut c = GlyphCache::new(1024);
    c.insert(key(BuiltinFont::Mono6x10, 'a', 256), glyph(60));
    // Same char, different face -> miss.
    assert!(c.get(&key(BuiltinFont::Mono10x20, 'a', 256)).is_none());
    // Same char, different size_q8 -> miss.
    assert!(c.get(&key(BuiltinFont::Mono6x10, 'a', 512)).is_none());
    // Same key -> hit.
    assert!(c.get(&key(BuiltinFont::Mono6x10, 'a', 256)).is_some());
}

#[test]
fn size_cap_enforced() {
    // Fit ~3 entries (60 bytes + 6 overhead each = 66; cap = 200 -> ~3).
    let mut c = GlyphCache::new(200);
    for ch in 'a'..='z' {
        c.insert(key(BuiltinFont::Mono6x10, ch, 256), glyph(60));
    }
    assert!(c.bytes_used() <= 200);
    assert!(c.len() <= 3, "{} entries fit in 200 bytes", c.len());
}

#[test]
fn lru_eviction_order_is_actually_lru() {
    let mut c = GlyphCache::new(200);
    c.insert(key(BuiltinFont::Mono6x10, 'a', 256), glyph(60));
    c.insert(key(BuiltinFont::Mono6x10, 'b', 256), glyph(60));
    c.insert(key(BuiltinFont::Mono6x10, 'c', 256), glyph(60));
    // Touch 'a' and 'c' -> 'b' is now LRU.
    let _ = c.get(&key(BuiltinFont::Mono6x10, 'a', 256));
    let _ = c.get(&key(BuiltinFont::Mono6x10, 'c', 256));
    // Insert 'd' -> 'b' evicted.
    c.insert(key(BuiltinFont::Mono6x10, 'd', 256), glyph(60));
    assert!(c.get(&key(BuiltinFont::Mono6x10, 'b', 256)).is_none());
    assert!(c.get(&key(BuiltinFont::Mono6x10, 'a', 256)).is_some());
    assert!(c.get(&key(BuiltinFont::Mono6x10, 'c', 256)).is_some());
    assert!(c.get(&key(BuiltinFont::Mono6x10, 'd', 256)).is_some());
}

#[test]
fn vector_font_face_key_distinct_from_builtin() {
    let mut c = GlyphCache::new(1024);
    c.insert(key(BuiltinFont::Mono6x10, 'a', 256), glyph(60));
    let vector_key = GlyphKey {
        face: FontFace::Vector {
            family: String::from("Inter"),
            style: weftos_leaf_scene::FontStyle::Normal,
        },
        ch: 'a' as u32,
        size_q8: 256,
    };
    // Different face -> different bucket.
    assert!(c.get(&vector_key).is_none());
}
