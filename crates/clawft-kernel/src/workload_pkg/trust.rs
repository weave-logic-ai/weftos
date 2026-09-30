//! Trust anchors: the pinned WeftOS signer set plus operator-pinned keys
//! (ADR-099 section 8.1), and optionally pinned Cognitum release keys
//! (ADR-100 section 6.2).

use serde::{Deserialize, Serialize};

use super::codec::{base64_decode, hex_decode_exact};
use super::manifest::valid_token;

/// Schema id of an operator trust file.
pub const TRUST_FILE_SCHEMA: &str = "weftos.workload-trust.v1";

/// Compiled-in WeftOS release signer set, as `(key_id, public_key_hex)`.
///
/// Empty until the WeftOS package-signing key is provisioned and its public
/// half is pinned here (a code change reviewed like any other). Until then
/// every accepted signature comes from an operator-pinned key.
pub const WEFTOS_PINNED_SIGNERS: &[(&str, &str)] = &[];

/// Which anchor set a key belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyOrigin {
    /// Compiled-in WeftOS signer set.
    Weftos,
    /// Operator-pinned key from governance config.
    Operator,
    /// Cognitum release-registry key (only used by the optional verifier).
    CognitumRelease,
}

/// A pinned Ed25519 public key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedKey {
    /// Key id the signature entry must carry.
    pub key_id: String,
    /// Raw public key.
    pub public_key: [u8; 32],
    /// Anchor set.
    pub origin: KeyOrigin,
}

/// Trust anchors used by the verifier.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustAnchors {
    /// Keys whose signatures satisfy the manifest signature requirement.
    pub signers: Vec<PinnedKey>,
    /// Cognitum release-record keys, consulted only when the policy enables
    /// the Cognitum verifier.
    pub cognitum: Vec<PinnedKey>,
}

/// One key entry in a trust file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustFileKey {
    /// Key id.
    pub key_id: String,
    /// Public key: 64 lower-case hex chars, or an Ed25519 SPKI PEM block.
    pub public_key: String,
}

/// Operator trust file (JSON), as held in governance config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustFile {
    /// Must be [`TRUST_FILE_SCHEMA`].
    pub schema: String,
    /// Operator-pinned package signers.
    #[serde(default)]
    pub operator_keys: Vec<TrustFileKey>,
    /// Pinned Cognitum release-registry keys.
    #[serde(default)]
    pub cognitum_release_keys: Vec<TrustFileKey>,
}

impl TrustAnchors {
    /// The compiled-in WeftOS signer set only.
    pub fn weftos_default() -> Result<Self, String> {
        let mut anchors = Self::default();
        for (id, hex) in WEFTOS_PINNED_SIGNERS {
            anchors.push_signer(id, hex, KeyOrigin::Weftos)?;
        }
        Ok(anchors)
    }

    /// WeftOS signer set plus the keys in an operator trust file.
    pub fn from_trust_file(file: &TrustFile) -> Result<Self, String> {
        if file.schema != TRUST_FILE_SCHEMA {
            return Err(format!("trust file schema must be {TRUST_FILE_SCHEMA}"));
        }
        let mut anchors = Self::weftos_default()?;
        for k in &file.operator_keys {
            anchors.push_signer(&k.key_id, &k.public_key, KeyOrigin::Operator)?;
        }
        for k in &file.cognitum_release_keys {
            let key = parse_key(&k.key_id, &k.public_key, KeyOrigin::CognitumRelease)?;
            anchors.cognitum.push(key);
        }
        Ok(anchors)
    }

    /// Parse a trust file from JSON bytes.
    pub fn from_trust_json(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > 256 * 1024 {
            return Err("trust file too large".into());
        }
        let file: TrustFile =
            serde_json::from_slice(bytes).map_err(|e| format!("trust file: {e}"))?;
        Self::from_trust_file(&file)
    }

    /// Add a signer key.
    pub fn push_signer(
        &mut self,
        key_id: &str,
        public_key: &str,
        origin: KeyOrigin,
    ) -> Result<(), String> {
        let key = parse_key(key_id, public_key, origin)?;
        if let Some(existing) = self.signers.iter().find(|k| k.public_key == key.public_key) {
            if existing.key_id != key.key_id {
                return Err(format!(
                    "public key pinned twice under {} and {}",
                    existing.key_id, key.key_id
                ));
            }
            return Ok(());
        }
        if self.signers.iter().any(|k| k.key_id == key.key_id) {
            return Err(format!(
                "key id {} pinned to two different keys",
                key.key_id
            ));
        }
        self.signers.push(key);
        Ok(())
    }

    /// Pinned signer matching this raw public key.
    pub fn signer(&self, public_key: &[u8; 32]) -> Option<&PinnedKey> {
        self.signers.iter().find(|k| &k.public_key == public_key)
    }
}

fn parse_key(key_id: &str, public_key: &str, origin: KeyOrigin) -> Result<PinnedKey, String> {
    if !valid_token(key_id, 128) {
        return Err(format!("bad key id {key_id:?}"));
    }
    let raw =
        parse_public_key(public_key).ok_or_else(|| format!("{key_id}: bad Ed25519 public key"))?;
    // Reject encodings that are not a valid curve point up front.
    ed25519_dalek::VerifyingKey::from_bytes(&raw).map_err(|e| format!("{key_id}: {e}"))?;
    Ok(PinnedKey {
        key_id: key_id.to_string(),
        public_key: raw,
        origin,
    })
}

/// Ed25519 SubjectPublicKeyInfo DER prefix (RFC 8410).
const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// Parse 64-char lower-case hex, or an Ed25519 `PUBLIC KEY` PEM block (the
/// form Cognitum trust registries publish).
pub fn parse_public_key(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if let Some(raw) = hex_decode_exact::<32>(s) {
        return Some(raw);
    }
    let body = s
        .strip_prefix("-----BEGIN PUBLIC KEY-----")?
        .strip_suffix("-----END PUBLIC KEY-----")?;
    let b64: String = body.split_whitespace().collect();
    let der = base64_decode(&b64)?;
    if der.len() != 44 || der[..12] != SPKI_PREFIX {
        return None;
    }
    der[12..].try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PK_HEX: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

    #[test]
    fn parses_hex_and_pem() {
        let raw = parse_public_key(PK_HEX).unwrap();
        let mut der = SPKI_PREFIX.to_vec();
        der.extend_from_slice(&raw);
        let b64 = standard_b64(&der);
        let pem = format!("-----BEGIN PUBLIC KEY-----\n{b64}\n-----END PUBLIC KEY-----\n");
        assert_eq!(parse_public_key(&pem), Some(raw));
        assert_eq!(parse_public_key("zz"), None);
    }

    #[test]
    fn trust_file_rejects_wrong_schema_and_conflicting_pins() {
        let bad = br#"{"schema":"nope","operator_keys":[]}"#;
        assert!(TrustAnchors::from_trust_json(bad).is_err());
        let conflict = format!(
            r#"{{"schema":"{TRUST_FILE_SCHEMA}","operator_keys":[{{"key_id":"a","public_key":"{PK_HEX}"}},{{"key_id":"b","public_key":"{PK_HEX}"}}]}}"#
        );
        assert!(TrustAnchors::from_trust_json(conflict.as_bytes()).is_err());
        let ok = format!(
            r#"{{"schema":"{TRUST_FILE_SCHEMA}","operator_keys":[{{"key_id":"a","public_key":"{PK_HEX}"}}]}}"#
        );
        let anchors = TrustAnchors::from_trust_json(ok.as_bytes()).unwrap();
        assert_eq!(anchors.signers.len(), 1);
        assert_eq!(anchors.signers[0].origin, KeyOrigin::Operator);
    }

    fn standard_b64(bytes: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for chunk in bytes.chunks(3) {
            let n =
                chunk.iter().fold(0u32, |acc, b| (acc << 8) | *b as u32) << (8 * (3 - chunk.len()));
            for i in 0..=chunk.len() {
                s.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
            }
        }
        while !s.len().is_multiple_of(4) {
            s.push('=');
        }
        s
    }
}
