//! Canonical JSON for signed project statements (certificates, anchors).
//!
//! This is the `serde_json` rendering with sorted keys, NOT RFC 8785 (JCS);
//! do not describe it as JCS. Signed string fields are restricted to ASCII
//! by their validators (hex digests, ULIDs, canonical timestamps), so escape
//! and normalisation differences between implementations do not arise.
//!
//! Rules (fixed so other implementations can reproduce the bytes): object
//! keys sorted by their UTF-8 bytes, no insignificant whitespace, strings
//! escaped exactly as `serde_json` does, integers in decimal. Signed
//! structures contain no floats; one is rendered by `serde_json` and is not
//! guaranteed portable, so do not put one in a signed statement.

use serde_json::Value;

/// Render `v` as canonical JSON.
pub fn canonical_json(v: &Value) -> String {
    let mut out = String::new();
    write(v, &mut out);
    out
}

fn write(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write(&map[k], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// Lowercase hex of `bytes`.
pub fn hex_encode(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 15) as usize] as char);
    }
    s
}

/// Decode exactly `N` bytes of **lowercase** hex. Uppercase is refused so a
/// hex string has one spelling and hashes of signed statements cannot fork.
pub fn hex_decode<const N: usize>(s: &str) -> Option<[u8; N]> {
    let b = s.as_bytes();
    if b.len() != N * 2 {
        return None;
    }
    let nib = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = (nib(b[2 * i])? << 4) | nib(b[2 * i + 1])?;
    }
    Some(out)
}

/// True when `s` is exactly `len` lowercase hex characters.
pub fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}
