//! WeaveLogic cog repository format and verification (COG-008).
//!
//! A repository is a `registry.json` plus per-arch binaries. Every binary is Ed25519-signed by
//! the WeaveLogic release key over the binary bytes, and the registry records its sha256 and
//! signature. Consumers (this tool's `install`, and the on-device `cogrepo` cog) pin the public
//! key and **refuse** anything unsigned, signed by another key, or whose sha256 does not match.
//! Signed-only, always (COG-008 decision).

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

mod revoked;
pub use revoked::{RevokedKeys, SUBJECTS_FILE_NAME};

/// The pinned WeaveLogic release public key (raw Ed25519, hex). The matching private key lives
/// in the dashboard/CI secret `WEAVELOGIC_RELEASE_KEY`, never in this repo.
pub const WEAVELOGIC_PUBKEY_HEX: &str = "6aae63e067488f1e5414ad4a6b9536bef0407db210fb33a3b378e8d6d12eca15";

pub const SCHEMA: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Registry {
    pub schema: u32,
    pub repo: String,
    #[serde(default)]
    pub updated: String,
    pub cogs: Vec<CogEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CogEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub hardware_requirement: Vec<String>,
    /// arch ("arm" | "arm64") -> artifact
    pub artifacts: BTreeMap<String, Artifact>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    /// Path within the repository, e.g. `cogs/arm/cog-bridge-arm`.
    pub path: String,
    pub size: u64,
    /// sha256 of the binary, hex.
    pub sha256: String,
    /// Ed25519 signature over the binary bytes, hex (128 chars).
    pub sig: String,
    /// The agent manifest.json for this cog (shipped to the Seed on install).
    pub manifest_path: String,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Payload prefix reserved for documents the same key signs that are not cogs:
/// `weaver update` verifies `"weftos-release-v1\n" + weftos-release.json`.
/// The cog signer refuses any payload that starts with it, so no cog
/// signature can ever pass as a release signature.
pub const RESERVED_PREFIX: &[u8] = b"weftos-release-";

/// Executable magic numbers a cog binary may start with: ELF, wasm, and
/// Mach-O (32/64-bit, both byte orders, and fat). Published cogs are ELF
/// (`arm` / `arm64` Seed binaries); the others are allowed for local builds.
const COG_MAGICS: [&[u8]; 7] = [
    b"\x7fELF",
    b"\0asm",
    &[0xfe, 0xed, 0xfa, 0xce],
    &[0xfe, 0xed, 0xfa, 0xcf],
    &[0xce, 0xfa, 0xed, 0xfe],
    &[0xcf, 0xfa, 0xed, 0xfe],
    &[0xca, 0xfe, 0xba, 0xbe],
];

/// Checked by every cog signing path before signing: refuses a payload in the
/// reserved release namespace and anything that is not an executable.
pub fn check_signable(bytes: &[u8]) -> Result<(), String> {
    if bytes.starts_with(RESERVED_PREFIX) {
        return Err("payload starts with the reserved \"weftos-release-\" prefix; refusing to sign it as a cog".into());
    }
    if !COG_MAGICS.iter().any(|m| bytes.starts_with(m)) {
        return Err("payload is not an ELF, Mach-O or wasm binary; refusing to sign it as a cog".into());
    }
    Ok(())
}

/// The pinned verifying key.
pub fn weavelogic_key() -> VerifyingKey {
    let raw: [u8; 32] = hex::decode(WEAVELOGIC_PUBKEY_HEX).expect("pinned pubkey hex").try_into().expect("32 bytes");
    VerifyingKey::from_bytes(&raw).expect("pinned pubkey is a valid Ed25519 key")
}

#[derive(Debug, PartialEq)]
pub enum VerifyError {
    SizeMismatch { want: u64, got: u64 },
    Sha256Mismatch { want: String, got: String },
    BadSignatureHex(String),
    SignatureRejected,
    /// The verifying key is on the operator's signer-key revocation list.
    KeyRevoked,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::SizeMismatch { want, got } => write!(f, "size {got} != registry {want}"),
            VerifyError::Sha256Mismatch { want, got } => write!(f, "sha256 {got} != registry {want}"),
            VerifyError::BadSignatureHex(e) => write!(f, "signature not valid hex/length: {e}"),
            VerifyError::SignatureRejected => write!(f, "Ed25519 signature rejected (not signed by a key pinned for this repository)"),
            VerifyError::KeyRevoked => write!(f, "the signing key is revoked"),
        }
    }
}

/// Verifies a downloaded binary against its artifact record using the pinned key. Checks size,
/// sha256, and the Ed25519 signature over the bytes. All three must pass — there is no bypass.
pub fn verify_artifact(bytes: &[u8], art: &Artifact, key: &VerifyingKey) -> Result<(), VerifyError> {
    if bytes.len() as u64 != art.size {
        return Err(VerifyError::SizeMismatch { want: art.size, got: bytes.len() as u64 });
    }
    let got = sha256_hex(bytes);
    if got != art.sha256 {
        return Err(VerifyError::Sha256Mismatch { want: art.sha256.clone(), got });
    }
    let sig_bytes: [u8; 64] = hex::decode(&art.sig)
        .map_err(|e| VerifyError::BadSignatureHex(e.to_string()))?
        .try_into()
        .map_err(|_| VerifyError::BadSignatureHex("not 64 bytes".into()))?;
    let sig = Signature::from_bytes(&sig_bytes);
    key.verify(bytes, &sig).map_err(|_| VerifyError::SignatureRejected)
}

/// [`verify_artifact`], but refuses first when `key` is revoked. A revoked key never verifies,
/// whatever it signed.
pub fn verify_artifact_unrevoked(bytes: &[u8], art: &Artifact, key: &VerifyingKey, revoked: &RevokedKeys) -> Result<(), VerifyError> {
    if revoked.contains(&hex::encode(key.to_bytes())) {
        return Err(VerifyError::KeyRevoked);
    }
    verify_artifact(bytes, art, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn test_pair() -> (SigningKey, VerifyingKey) {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let vk = sk.verifying_key();
        (sk, vk)
    }

    fn artifact_for(sk: &SigningKey, bytes: &[u8]) -> Artifact {
        Artifact {
            path: "cogs/arm/cog-x-arm".into(),
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
            sig: hex::encode(sk.sign(bytes).to_bytes()),
            manifest_path: "cogs/arm/manifest.json".into(),
        }
    }

    #[test]
    fn the_signer_refuses_release_prefixed_and_non_executable_payloads() {
        let e = check_signable(b"weftos-release-v1\n{\"schema\":1}").unwrap_err();
        assert!(e.contains("reserved"), "{e}");
        assert!(check_signable(b"weftos-release-v2 anything").is_err());
        assert!(check_signable(b"#!/bin/sh\necho hi").unwrap_err().contains("not an ELF"));
        assert!(check_signable(b"").is_err());
        for ok in [&b"\x7fELF\x02\x01"[..], b"\0asm\x01\0\0\0", &[0xcf, 0xfa, 0xed, 0xfe, 7], &[0xca, 0xfe, 0xba, 0xbe, 0]] {
            assert!(check_signable(ok).is_ok(), "{ok:?}");
        }
    }

    #[test]
    fn pinned_key_is_valid() {
        let _ = weavelogic_key();
    }

    #[test]
    fn a_correctly_signed_binary_verifies_and_tampering_is_caught() {
        let (sk, vk) = test_pair();
        let bytes = b"ELF...cog binary...".to_vec();
        let art = artifact_for(&sk, &bytes);
        assert!(verify_artifact(&bytes, &art, &vk).is_ok());

        // one flipped byte -> sha256 mismatch
        let mut bad = bytes.clone();
        bad[3] ^= 1;
        assert!(matches!(verify_artifact(&bad, &art, &vk), Err(VerifyError::SizeMismatch { .. }) | Err(VerifyError::Sha256Mismatch { .. })));

        // right bytes, but signed by a different key -> rejected (even if we also fix sha)
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let forged = Artifact { sig: hex::encode(other.sign(&bytes).to_bytes()), ..art.clone() };
        assert_eq!(verify_artifact(&bytes, &forged, &vk), Err(VerifyError::SignatureRejected));

        // a tampered binary whose sha we "fix" in the record still fails the signature
        let fixed_sha = Artifact { size: bad.len() as u64, sha256: sha256_hex(&bad), ..art };
        assert_eq!(verify_artifact(&bad, &fixed_sha, &vk), Err(VerifyError::SignatureRejected));
    }

    #[test]
    fn a_revoked_key_never_verifies() {
        let (sk, vk) = test_pair();
        let bytes = b"ELF...".to_vec();
        let art = artifact_for(&sk, &bytes);
        assert!(verify_artifact_unrevoked(&bytes, &art, &vk, &RevokedKeys::none()).is_ok());
        let revoked = RevokedKeys::from_keys([hex::encode(vk.to_bytes())]);
        assert_eq!(verify_artifact_unrevoked(&bytes, &art, &vk, &revoked), Err(VerifyError::KeyRevoked));
    }

    #[test]
    fn registry_round_trips_json() {
        let (sk, _) = test_pair();
        let reg = Registry {
            schema: SCHEMA,
            repo: "weavelogic".into(),
            updated: "2026-10-02".into(),
            cogs: vec![CogEntry {
                id: "bridge".into(),
                name: "Sensor Bridge".into(),
                version: "0.1.0".into(),
                category: "network".into(),
                description: "d".into(),
                hardware_requirement: vec!["pi-zero-2w".into(), "v0-appliance".into()],
                artifacts: BTreeMap::from([("arm".into(), artifact_for(&sk, b"x"))]),
            }],
        };
        let s = serde_json::to_string(&reg).unwrap();
        assert_eq!(serde_json::from_str::<Registry>(&s).unwrap(), reg);
    }
}
