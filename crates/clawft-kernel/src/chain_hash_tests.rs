//! Hash-commitment property tests for `compute_event_hash` (ADR-103 A7).

use chrono::{TimeZone, Utc};

use crate::chain::compute_event_hash;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn bytes32(&mut self) -> [u8; 32] {
        let mut b = [0u8; 32];
        for c in b.chunks_mut(8) {
            c.copy_from_slice(&self.next().to_le_bytes());
        }
        b
    }
    fn text(&mut self) -> String {
        let n = (self.next() % 12) as usize;
        (0..n)
            .map(|_| match self.next() % 5 {
                0 => '\0',
                1 => '.',
                _ => (b'a' + (self.next() % 26) as u8) as char,
            })
            .collect()
    }
}

struct Fields {
    seq: u64,
    chain_id: u32,
    prev: [u8; 32],
    source: String,
    kind: String,
    ts: i64,
    ph: [u8; 32],
}

fn fields(r: &mut Rng) -> Fields {
    Fields {
        seq: r.next() % 1_000_000,
        chain_id: (r.next() % 4) as u32,
        prev: r.bytes32(),
        source: r.text(),
        kind: r.text(),
        ts: 1_700_000_000 + (r.next() % 400_000_000) as i64,
        ph: r.bytes32(),
    }
}

fn hash(f: &Fields, rh: Option<&[u8; 32]>) -> [u8; 32] {
    compute_event_hash(
        f.seq,
        f.chain_id,
        &f.prev,
        &f.source,
        &f.kind,
        &Utc.timestamp_opt(f.ts, 0).unwrap(),
        &f.ph,
        rh,
    )
}

/// A `None` event never hashes like a `Some` event: same fields, and
/// independently random fields.
#[test]
fn none_and_some_never_collide() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let mut seen = std::collections::HashSet::new();
    for _ in 0..20_000 {
        let a = fields(&mut r);
        let b = fields(&mut r);
        let rh = r.bytes32();
        let none_a = hash(&a, None);
        let some_a = hash(&a, Some(&rh));
        let some_b = hash(&b, Some(&rh));
        assert_ne!(none_a, some_a);
        assert_ne!(none_a, some_b);
        assert!(seen.insert(none_a), "None hash repeated");
        assert!(seen.insert(some_a), "Some hash collided with another event");
    }
}

/// The rule hash is committed to byte by byte.
#[test]
fn every_rule_hash_byte_matters() {
    let mut r = Rng(42);
    let f = fields(&mut r);
    let rh = r.bytes32();
    let base = hash(&f, Some(&rh));
    for i in 0..32 {
        let mut t = rh;
        t[i] ^= 1;
        assert_ne!(hash(&f, Some(&t)), base, "byte {i}");
    }
}
