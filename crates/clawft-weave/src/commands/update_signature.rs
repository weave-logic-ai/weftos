//! Release authenticity for `weaver update`: an Ed25519 signature by the
//! WeaveLogic release key over a per-release hash list.
//!
//! Release CI (`scripts/release/sign-release.sh`) writes two assets:
//!
//! - `weftos-release.json`: `{"schema":1,"kind":"weftos-release","tag":"v…",
//!   "assets":{"<file>":"<sha256 hex>",…}}`, listing every file uploaded to the
//!   release, `dist-manifest.json` included.
//! - `weftos-release.json.sig`: the hex Ed25519 signature over
//!   `"weftos-release-v1\n"` followed by the exact bytes of that file. The
//!   prefix keeps a signature made for a cog binary (COG-008 signs raw bytes
//!   with the same key) from ever verifying here, and the reverse.
//!
//! The verifying key is compiled in: the COG-008 pinned key
//! ([`weftos_cog_repo::WEAVELOGIC_PUBKEY_HEX`]). There is no file, variable or
//! flag that swaps it; tests hand a throwaway key to [`Trust::Pinned`] through
//! the update context. The one runtime escape is `--insecure-skip-signature`,
//! which skips this check (sha256 checks still run) and says so loudly.

use std::collections::BTreeMap;

use anyhow::{Context, anyhow, bail};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The signed hash list's asset name.
pub const SIGNED_DOC: &str = "weftos-release.json";
/// The detached signature's asset name.
pub const SIGNATURE: &str = "weftos-release.json.sig";
/// Domain-separation prefix prepended to the document before signing.
pub const DOMAIN: &[u8] = b"weftos-release-v1\n";
/// The manifest's entry in the signed hash list.
pub const MANIFEST: &str = "dist-manifest.json";

/// Which releases `weaver update` accepts.
#[derive(Debug, Clone)]
pub enum Trust {
    /// Only releases whose hash list verifies under this key.
    Pinned(VerifyingKey),
    /// `--insecure-skip-signature`: authenticity is not checked.
    Skip,
}

impl Trust {
    /// The compiled-in WeaveLogic release key (the COG-008 key).
    pub fn pinned() -> Self {
        Trust::Pinned(weftos_cog_repo::weavelogic_key())
    }
}

/// A verified hash list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRelease {
    pub tag: String,
    pub assets: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Doc {
    schema: u32,
    kind: String,
    tag: String,
    assets: BTreeMap<String, String>,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Verify `sig_text` over `doc` with `key`, then parse the hash list. Nothing
/// in `doc` is read before the signature checks out.
pub fn verify(doc: &[u8], sig_text: &str, key: &VerifyingKey) -> anyhow::Result<SignedRelease> {
    let raw = hex::decode(sig_text.trim()).map_err(|_| anyhow!("{SIGNATURE} is not hex"))?;
    let raw: [u8; 64] = raw.try_into().map_err(|_| anyhow!("{SIGNATURE} is not a 64-byte Ed25519 signature"))?;
    let mut msg = DOMAIN.to_vec();
    msg.extend_from_slice(doc);
    key.verify(&msg, &Signature::from_bytes(&raw))
        .map_err(|_| anyhow!("release signature rejected: {SIGNED_DOC} is not signed by the WeaveLogic release key"))?;
    let d: Doc = serde_json::from_slice(doc).with_context(|| format!("{SIGNED_DOC} is signed but malformed"))?;
    if d.schema != 1 || d.kind != "weftos-release" {
        bail!("{SIGNED_DOC} has schema {} kind {:?}; this weaver understands schema 1 weftos-release", d.schema, d.kind);
    }
    if let Some((name, h)) = d.assets.iter().find(|(_, h)| !is_hash(h)) {
        bail!("{SIGNED_DOC} lists a malformed sha256 {h:?} for {name}");
    }
    Ok(SignedRelease { tag: d.tag, assets: d.assets })
}

impl SignedRelease {
    /// Require the signed list to name `name` with exactly `sha256`.
    pub fn check(&self, name: &str, sha256: &str) -> anyhow::Result<()> {
        match self.assets.get(name) {
            Some(h) if h == sha256 => Ok(()),
            Some(h) => bail!("{name} does not match the signed release: signed {h}, downloaded {sha256}"),
            None => bail!("{name} is not listed in the signed release {SIGNED_DOC}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn key(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }

    fn signed(k: &SigningKey, doc: &str) -> String {
        let mut m = DOMAIN.to_vec();
        m.extend_from_slice(doc.as_bytes());
        hex::encode(k.sign(&m).to_bytes())
    }

    const DOC: &str = r#"{"schema":1,"kind":"weftos-release","tag":"v1.0.0","assets":{"dist-manifest.json":"0000000000000000000000000000000000000000000000000000000000000000"}}"#;

    #[test]
    fn good_signature_parses_and_checks() {
        let k = key(1);
        let s = verify(DOC.as_bytes(), &format!("{}\n", signed(&k, DOC)), &k.verifying_key()).unwrap();
        assert_eq!(s.tag, "v1.0.0");
        assert!(s.check(MANIFEST, &"0".repeat(64)).is_ok());
        assert!(s.check(MANIFEST, &"1".repeat(64)).is_err());
        assert!(s.check("other", &"0".repeat(64)).is_err());
    }

    #[test]
    fn wrong_key_edited_doc_and_garbage_are_rejected() {
        let k = key(1);
        let sig = signed(&k, DOC);
        assert!(verify(DOC.as_bytes(), &sig, &key(2).verifying_key()).is_err());
        let edited = DOC.replace("v1.0.0", "v9.0.0");
        assert!(verify(edited.as_bytes(), &sig, &k.verifying_key()).is_err());
        assert!(verify(DOC.as_bytes(), "zz", &k.verifying_key()).is_err());
        assert!(verify(DOC.as_bytes(), "abcd", &k.verifying_key()).is_err());
    }

    #[test]
    fn a_signature_without_the_domain_prefix_is_rejected() {
        // What a COG-008 signature over raw bytes looks like.
        let k = key(1);
        let raw = hex::encode(k.sign(DOC.as_bytes()).to_bytes());
        assert!(verify(DOC.as_bytes(), &raw, &k.verifying_key()).is_err());
    }

    #[test]
    fn signed_but_malformed_docs_are_rejected() {
        let k = key(1);
        for doc in [
            DOC.replace("\"schema\":1", "\"schema\":2"),
            DOC.replace("weftos-release\"", "cog\""),
            DOC.replace(&"0".repeat(64), &"A".repeat(64)),
            DOC.replace("\"tag\"", "\"extra\":1,\"tag\""),
        ] {
            assert!(verify(doc.as_bytes(), &signed(&k, &doc), &k.verifying_key()).is_err(), "{doc}");
        }
    }

    #[test]
    fn production_trust_is_the_pinned_cog008_key() {
        let Trust::Pinned(k) = Trust::pinned() else { panic!() };
        assert_eq!(hex::encode(k.to_bytes()), weftos_cog_repo::WEAVELOGIC_PUBKEY_HEX);
    }
}
