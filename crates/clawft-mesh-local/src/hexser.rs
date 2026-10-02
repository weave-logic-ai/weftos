//! Lowercase-hex helpers and serde adapters for fixed-size byte arrays.

use serde::{Deserialize, Deserializer, Serializer};

/// Encode bytes as lowercase hex.
pub fn encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Decode exactly `N` bytes from lowercase hex; anything else is `None`.
pub fn decode<const N: usize>(s: &str) -> Option<[u8; N]> {
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
    for (i, pair) in b.chunks_exact(2).enumerate() {
        out[i] = (nib(pair[0])? << 4) | nib(pair[1])?;
    }
    Some(out)
}

fn ser<S: Serializer, const N: usize>(v: &[u8; N], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&encode(v))
}

fn de<'de, D: Deserializer<'de>, const N: usize>(d: D) -> Result<[u8; N], D::Error> {
    let s = String::deserialize(d)?;
    decode::<N>(&s).ok_or_else(|| {
        serde::de::Error::custom(format!("expected {} lowercase hex characters", N * 2))
    })
}

/// `#[serde(with = "crate::hexser::hex32")]` for `[u8; 32]`.
pub mod hex32 {
    use super::*;
    pub fn serialize<S: Serializer>(v: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        ser(v, s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        de(d)
    }
}

/// `#[serde(with = "crate::hexser::hex64")]` for `[u8; 64]`.
pub mod hex64 {
    use super::*;
    pub fn serialize<S: Serializer>(v: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        ser(v, s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        de(d)
    }
}
