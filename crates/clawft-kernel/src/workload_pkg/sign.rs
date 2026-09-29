//! Ed25519 signing of manifest envelopes (ADR-025 key type).

use ed25519_dalek::{Signer, SigningKey};

use super::codec::{hex_decode_exact, hex_encode};
use super::manifest::{ManifestEnvelope, ManifestError, SignatureEntry, valid_token};

/// Default key id for a public key: `ed25519:` plus the first 16 hex chars
/// of its BLAKE3 hash.
pub fn key_id_for(public_key: &[u8; 32]) -> String {
    let h = blake3::hash(public_key).to_hex();
    format!("ed25519:{}", &h[..16])
}

/// Load a signing key from a 64-char lower-case hex seed (surrounding
/// whitespace allowed, as written by `weaver workload keygen`).
pub fn signing_key_from_hex(s: &str) -> Result<SigningKey, String> {
    let seed = hex_decode_exact::<32>(s.trim())
        .ok_or_else(|| "signing key must be 64 lower-case hex chars".to_string())?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Append a signature by `key` under `key_id`. Re-signing with a key that
/// already signed replaces that entry, so the operation is idempotent.
pub fn sign_envelope(
    envelope: &mut ManifestEnvelope,
    key: &SigningKey,
    key_id: &str,
) -> Result<(), ManifestError> {
    if !valid_token(key_id, 128) {
        return Err(ManifestError::Invalid(format!("bad key id {key_id:?}")));
    }
    let msg = envelope.signed_statement()?;
    let sig = key.sign(&msg);
    let public_key = hex_encode(key.verifying_key().as_bytes());
    envelope.signatures.retain(|s| s.public_key != public_key);
    envelope.signatures.push(SignatureEntry {
        algorithm: "ed25519".to_string(),
        key_id: key_id.to_string(),
        public_key,
        signature: hex_encode(&sig.to_bytes()),
    });
    Ok(())
}
