//! Small, strict hex and base64 codecs (the kernel carries no hex/base64
//! crates, and these inputs are all short, bounded key material).

/// Lower-case hex encoding.
pub fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    s
}

/// Decode lower-case hex of exactly `N` bytes. Upper-case is refused so each
/// value has one spelling.
pub fn hex_decode_exact<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    let bytes = s.as_bytes();
    for (i, slot) in out.iter_mut().enumerate() {
        let hi = hex_val(bytes[2 * i])?;
        let lo = hex_val(bytes[2 * i + 1])?;
        *slot = (hi << 4) | lo;
    }
    Some(out)
}

/// True when `s` is exactly `len` lower-case hex characters.
pub fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| hex_val(b).is_some())
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// Decode base64 in either the standard or the URL-safe alphabet, with or
/// without `=` padding. Whitespace is refused. Non-canonical trailing bits
/// are refused so an encoding cannot be malleated.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let trimmed = s.trim_end_matches('=');
    if s.len() - trimmed.len() > 2 || trimmed.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in trimmed.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    if acc != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_and_strictness() {
        let bytes = [0u8, 1, 0xab, 0xff];
        let s = hex_encode(&bytes);
        assert_eq!(s, "0001abff");
        assert_eq!(hex_decode_exact::<4>(&s), Some(bytes));
        assert_eq!(hex_decode_exact::<4>("0001ABFF"), None);
        assert_eq!(hex_decode_exact::<4>("0001ab"), None);
        assert!(is_lower_hex("ab01", 4));
        assert!(!is_lower_hex("ab0g", 4));
    }

    #[test]
    fn base64_both_alphabets() {
        assert_eq!(base64_decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("-_8").unwrap(), vec![0xfb, 0xff]);
        assert_eq!(base64_decode("+/8").unwrap(), vec![0xfb, 0xff]);
        assert!(base64_decode("a").is_none());
        assert!(base64_decode("aGVsbG8===").is_none());
        assert!(base64_decode("aGVs bG8").is_none());
        // Non-canonical trailing bits ("aGVsbG9" ends with a stray bit).
        assert!(base64_decode("aGVsbG9=").is_none());
    }
}
