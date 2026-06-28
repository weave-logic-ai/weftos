//! CBOR encode / decode — see
//! [vector-leaf-display.md §4.1 wire format](../../../docs/design/vector-leaf-display.md).
//!
//! Canonical wire form: CBOR via [`ciborium`]. Bytes round-trip
//! identically on embedded (`no_std + alloc`) and browser (`wasm32`)
//! targets, by construction — every wire type is plain serde and
//! avoids tag-sensitive formats.
//!
//! The decoder rejects envelopes whose `version` byte doesn't match
//! [`WIRE_VERSION`](crate::envelope::WIRE_VERSION).

use alloc::vec::Vec;

use serde::{de::DeserializeOwned, Serialize};

use crate::envelope::{InputEnvelope, SceneEnvelope, WIRE_VERSION};

/// Codec error surface. Tiny by design — callers route to mesh-level
/// transport errors and don't need fine-grained distinction.
#[derive(Debug)]
pub enum CodecError {
    /// Encoder failed. Buffer-OOM on embedded, never on host.
    Encode,
    /// Decoder failed. Malformed CBOR or schema mismatch.
    Decode,
    /// `version` byte was not [`WIRE_VERSION`]. Carries the observed
    /// version so the caller can log it.
    VersionMismatch { found: u8, expected: u8 },
}

impl core::fmt::Display for CodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CodecError::Encode => write!(f, "scene codec: encode failure"),
            CodecError::Decode => write!(f, "scene codec: decode failure"),
            CodecError::VersionMismatch { found, expected } => write!(
                f,
                "scene codec: wire version mismatch (found {found}, expected {expected})"
            ),
        }
    }
}

/// Encode any serde-`Serialize` type to CBOR bytes.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::new();
    ciborium::into_writer(value, &mut out).map_err(|_| CodecError::Encode)?;
    Ok(out)
}

/// Decode any serde-`DeserializeOwned` type from CBOR bytes. **Does
/// not** validate `version`; use [`decode_scene_envelope`] /
/// [`decode_input_envelope`] for envelope types.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    ciborium::from_reader(bytes).map_err(|_| CodecError::Decode)
}

/// Decode a [`SceneEnvelope`] and verify its wire version. Returns
/// [`CodecError::VersionMismatch`] if the byte doesn't match.
pub fn decode_scene_envelope(bytes: &[u8]) -> Result<SceneEnvelope, CodecError> {
    let env: SceneEnvelope = decode(bytes)?;
    if env.version != WIRE_VERSION {
        return Err(CodecError::VersionMismatch {
            found: env.version,
            expected: WIRE_VERSION,
        });
    }
    Ok(env)
}

/// Decode an [`InputEnvelope`] and verify its wire version.
pub fn decode_input_envelope(bytes: &[u8]) -> Result<InputEnvelope, CodecError> {
    let env: InputEnvelope = decode(bytes)?;
    if env.version != WIRE_VERSION {
        return Err(CodecError::VersionMismatch {
            found: env.version,
            expected: WIRE_VERSION,
        });
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_mismatch_rejected() {
        // Hand-encode a SceneEnvelope with version=99.
        let env = SceneEnvelope {
            version: 99,
            display_id: 0,
            ops: alloc::vec::Vec::new(),
        };
        let bytes = encode(&env).unwrap();
        let err = decode_scene_envelope(&bytes).unwrap_err();
        match err {
            CodecError::VersionMismatch { found, expected } => {
                assert_eq!(found, 99);
                assert_eq!(expected, WIRE_VERSION);
            }
            other => panic!("expected VersionMismatch, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_empty_envelope() {
        let env = SceneEnvelope::new(0, alloc::vec::Vec::new());
        let bytes = encode(&env).unwrap();
        let back = decode_scene_envelope(&bytes).unwrap();
        assert_eq!(env, back);
    }
}
