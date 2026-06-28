//! LRU glyph cache — see
//! [vector-leaf-display.md §6 Renderer Trait](../../../docs/design/vector-leaf-display.md).
//!
//! Keyed on `(FontFace, char, size_q8)`. In v1 the cache backs the
//! Phase A built-ins (`Mono6x10`, `Mono10x20`), whose glyphs are
//! immutable `&'static` data from `embedded-graphics::mono_font`; the
//! cache mostly acts as a pre-rasterized witness so v1.1's vector font
//! pipeline can drop in without reshaping the renderer.
//!
//! ## LRU mechanics
//!
//! - Bounded by a configurable `capacity_bytes`. Each cached glyph
//!   reports `Glyph::byte_size()`; insertion + lookup evict the
//!   least-recently-used entries until total ≤ capacity.
//! - `hashbrown::HashMap` for the index, plus a single `u64` access
//!   counter per entry. Linear scan to find the LRU victim — cache
//!   size is bounded (target ≤ 64 KiB) so the scan is O(small).
//! - User-approved per the spec: "size-bounded (e.g. 64 KiB cap
//!   configurable), LRU eviction when exceeded".
//!
//! For v1's mono fonts, glyphs are loaded once and never evicted in
//! practice (8 KiB worth of ASCII fits well under 64 KiB). The cache
//! shape is what matters; v1.1's vector pipeline will fill it.

use alloc::vec::Vec;

use hashbrown::HashMap;
use weftos_leaf_scene::FontFace;

/// Cache key — `(face, char, size_q8)`. `face` is the wire-format
/// `FontFace`; for mono fonts the `size_q8` field is informative-only
/// (the bitmap is fixed-size in `embedded-graphics`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub face: FontFace,
    pub ch: u32,
    pub size_q8: u16,
}

/// One cached glyph.
///
/// `bitmap` is one byte per pixel (alpha mask; `0..=255`). v1's mono
/// fonts emit `0` or `255` only; v1.1's grayscale rasterizer fills
/// intermediate values for AA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glyph {
    pub w: u16,
    pub h: u16,
    /// Advance to the next glyph in Q24.8 px (mono fonts: `w << 8`).
    pub advance_q8: i32,
    pub bitmap: Vec<u8>,
}

impl Glyph {
    /// Bytes this glyph occupies in the cache. Used by LRU accounting.
    #[inline]
    pub fn byte_size(&self) -> usize {
        // 6 bytes overhead + bitmap.
        6 + self.bitmap.len()
    }
}

/// Entry stored alongside each glyph in the cache.
#[derive(Debug, Clone)]
struct Entry {
    glyph: Glyph,
    /// Monotonic touch counter — higher = more recently used.
    last_used: u64,
}

/// Size-bounded LRU glyph cache.
///
/// ```ignore
/// use weftos_leaf_renderer::glyph_cache::{GlyphCache, GlyphKey, Glyph};
/// let mut cache = GlyphCache::new(64 * 1024); // 64 KiB
/// let key = GlyphKey { face: my_face, ch: 'a' as u32, size_q8: 256 };
/// if let Some(g) = cache.get(&key) { /* hit */ }
/// cache.insert(key, my_glyph);
/// ```
#[derive(Debug)]
pub struct GlyphCache {
    map: HashMap<GlyphKey, Entry>,
    /// Monotonic counter incremented on every hit + insert. Wrap-around
    /// is acceptable: a 64-bit counter ticking once per glyph would take
    /// ~5 billion years at 100 MHz.
    tick: u64,
    /// Running sum of every entry's `byte_size`.
    bytes_used: usize,
    /// Hard cap. Insertions evict LRU entries until `bytes_used <= capacity_bytes`.
    capacity_bytes: usize,
    /// Diagnostics. Public via accessor methods.
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl GlyphCache {
    /// Create an empty cache with `capacity_bytes` upper bound.
    ///
    /// `capacity_bytes = 0` disables caching entirely (every `get` is
    /// a miss; every `insert` immediately evicts). Useful for tests
    /// that want to exercise the miss path repeatedly.
    pub fn new(capacity_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            tick: 0,
            bytes_used: 0,
            capacity_bytes,
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    /// Lookup. Touches the entry to update LRU order on hit.
    pub fn get(&mut self, key: &GlyphKey) -> Option<&Glyph> {
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        if let Some(entry) = self.map.get_mut(key) {
            entry.last_used = tick;
            self.hits += 1;
            Some(&entry.glyph)
        } else {
            self.misses += 1;
            None
        }
    }

    /// Insert a glyph. May evict zero or more LRU entries to make
    /// room. Returns the number of entries evicted.
    pub fn insert(&mut self, key: GlyphKey, glyph: Glyph) -> usize {
        let size = glyph.byte_size();
        // If the glyph itself exceeds the cap, refuse the insert (and
        // evict any prior entry under this key).
        if size > self.capacity_bytes {
            if let Some(prev) = self.map.remove(&key) {
                self.bytes_used -= prev.glyph.byte_size();
            }
            return 0;
        }

        // If replacing an existing entry, subtract its size first.
        let mut evicted = 0;
        if let Some(prev) = self.map.remove(&key) {
            self.bytes_used -= prev.glyph.byte_size();
        }

        // Evict LRU entries until there's room.
        while self.bytes_used + size > self.capacity_bytes {
            if !self.evict_one() {
                // Empty cache and still no room — would only happen
                // if a glyph larger than the cap snuck through; the
                // guard above catches that. Bail safely.
                break;
            }
            evicted += 1;
        }

        self.tick = self.tick.wrapping_add(1);
        self.map.insert(
            key,
            Entry {
                glyph,
                last_used: self.tick,
            },
        );
        self.bytes_used += size;
        self.evictions += evicted as u64;
        evicted
    }

    /// Evict the single least-recently-used entry. No-op + `false` if
    /// the cache is empty.
    fn evict_one(&mut self) -> bool {
        let Some((victim_key, _)) = self
            .map
            .iter()
            .min_by_key(|(_, e)| e.last_used)
            .map(|(k, e)| (k.clone(), e.last_used))
        else {
            return false;
        };
        if let Some(entry) = self.map.remove(&victim_key) {
            self.bytes_used -= entry.glyph.byte_size();
            true
        } else {
            false
        }
    }

    /// Cached entry count.
    #[inline]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Current resident bytes. Strictly ≤ `capacity_bytes`.
    #[inline]
    pub fn bytes_used(&self) -> usize {
        self.bytes_used
    }

    /// Drop everything. Counters survive — they describe cache *use*,
    /// not contents.
    pub fn clear(&mut self) {
        self.map.clear();
        self.bytes_used = 0;
    }

    /// Cumulative hits, misses, evictions. Useful for diagnostics +
    /// the renderer's `render_damage` return value.
    #[inline]
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits,
            misses: self.misses,
            evictions: self.evictions,
            entries: self.map.len(),
            bytes_used: self.bytes_used,
        }
    }
}

/// Snapshot of cache diagnostics.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub entries: usize,
    pub bytes_used: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use weftos_leaf_scene::{BuiltinFont, FontFace};

    fn key(ch: char, size: u16) -> GlyphKey {
        GlyphKey {
            face: FontFace::Builtin(BuiltinFont::Mono6x10),
            ch: ch as u32,
            size_q8: size,
        }
    }

    fn glyph(size: usize) -> Glyph {
        Glyph {
            w: 6,
            h: 10,
            advance_q8: 6 << 8,
            bitmap: alloc::vec![0u8; size],
        }
    }

    #[test]
    fn miss_then_hit() {
        let mut c = GlyphCache::new(1024);
        assert!(c.get(&key('a', 256)).is_none());
        c.insert(key('a', 256), glyph(60));
        let _ = c.get(&key('a', 256)).expect("present");
        let s = c.stats();
        assert_eq!(s.hits, 1);
        assert_eq!(s.misses, 1);
    }

    #[test]
    fn lru_evicts_oldest_under_pressure() {
        let mut c = GlyphCache::new(200);
        // Insert three glyphs of ~66 bytes each (60 + 6 overhead). Total
        // resident = 198, room for none more.
        c.insert(key('a', 256), glyph(60));
        c.insert(key('b', 256), glyph(60));
        c.insert(key('c', 256), glyph(60));
        // Touch 'a' so it becomes most-recent.
        let _ = c.get(&key('a', 256));
        // Insert 'd' — should evict 'b' (oldest untouched).
        let evicted = c.insert(key('d', 256), glyph(60));
        assert_eq!(evicted, 1);
        assert!(c.get(&key('a', 256)).is_some(), "'a' should survive");
        assert!(c.get(&key('b', 256)).is_none(), "'b' should be evicted");
        assert!(c.get(&key('c', 256)).is_some(), "'c' should survive");
        assert!(c.get(&key('d', 256)).is_some(), "'d' should be present");
    }

    #[test]
    fn size_cap_respected_after_insert() {
        let mut c = GlyphCache::new(200);
        for ch in 'a'..='z' {
            c.insert(key(ch, 256), glyph(60));
        }
        assert!(c.bytes_used() <= 200);
    }

    #[test]
    fn glyph_larger_than_cap_is_rejected() {
        let mut c = GlyphCache::new(100);
        let prior = c.bytes_used();
        c.insert(key('A', 256), glyph(200)); // 206 byte > 100 cap
        assert_eq!(c.bytes_used(), prior);
        assert!(c.get(&key('A', 256)).is_none());
    }

    #[test]
    fn duplicate_insert_replaces_in_place() {
        let mut c = GlyphCache::new(1024);
        c.insert(key('x', 256), glyph(60));
        assert_eq!(c.len(), 1);
        let bytes_before = c.bytes_used();
        c.insert(key('x', 256), glyph(60));
        assert_eq!(c.len(), 1);
        // Bytes_used stays equal — old entry was subtracted before adding new.
        assert_eq!(c.bytes_used(), bytes_before);
    }

    #[test]
    fn zero_capacity_means_no_caching() {
        let mut c = GlyphCache::new(0);
        c.insert(key('a', 256), glyph(60));
        assert_eq!(c.len(), 0);
        assert!(c.get(&key('a', 256)).is_none());
    }

    #[test]
    fn clear_drops_contents_preserves_counters() {
        let mut c = GlyphCache::new(1024);
        c.insert(key('a', 256), glyph(60));
        let _ = c.get(&key('a', 256));
        c.clear();
        assert_eq!(c.len(), 0);
        assert_eq!(c.bytes_used(), 0);
        // Counters survive — they describe lifetime usage.
        let s = c.stats();
        assert_eq!(s.hits, 1);
        assert_eq!(s.entries, 0);
    }
}
