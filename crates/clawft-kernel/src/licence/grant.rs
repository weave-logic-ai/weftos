//! Checkout grant (ADR-106 section 4): the Seed's signed statement that one
//! mesh may share one cog version.

use std::collections::BTreeSet;

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use super::{
    GRANT_DOMAIN, LicenceError, MAX_GRANT_TTL_SECS, MeshId, SignedEnvelope, envelope_key, key_id,
    parse_canonical, sha256_hex, sign_envelope, valid_hex32, valid_token, verify_envelope,
};

/// Most architectures one grant lists.
pub const MAX_GRANT_ARTIFACTS: usize = 8;

/// One checked-out binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantArtifact {
    /// Target architecture, e.g. `aarch64`.
    pub arch: String,
    /// Size in bytes.
    pub size: u64,
    /// Registry sha256, lower-case hex.
    pub sha256: String,
    /// BLAKE3 (the swarm content hash), lower-case hex.
    pub blake3: String,
}

/// The licence the grant rests on: a hash of the reference, no account label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LicenceRef {
    /// sha256 of the licence reference.
    pub ref_sha256: String,
    /// Licence expiry, unix seconds.
    pub expires: u64,
}

/// The signed grant payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutGrant {
    /// Always 1.
    pub v: u32,
    /// sha256 of this payload with `grant_id` empty ([`CheckoutGrant::compute_id`]).
    pub grant_id: String,
    /// The mesh, as [`MeshId::to_hex`].
    pub mesh_id: String,
    /// The Seed's device id.
    pub seed_device_id: String,
    /// `ed25519:` plus 16 hex, the grant key's id ([`key_id`]).
    pub grant_key_id: String,
    /// Always `cognitum`.
    pub source: String,
    /// Registry the binaries came from.
    pub registry: String,
    /// Cog id.
    pub cog_id: String,
    /// Cog version.
    pub version: String,
    /// Checked-out binaries, sorted by arch.
    pub artifacts: Vec<GrantArtifact>,
    /// sha256 of the registry entry used.
    pub manifest_sha256: String,
    /// The licence behind the grant.
    pub licence: LicenceRef,
    /// Per (mesh, cog, version); the highest wins.
    pub seq: u64,
    /// Signing time, unix seconds.
    pub issued_at: u64,
    /// End of validity; `<= issued_at` is a withdrawal.
    pub expires_at: u64,
}

impl CheckoutGrant {
    /// The `grant_id` for this grant's content.
    pub fn compute_id(&self) -> String {
        let mut g = self.clone();
        g.grant_id = String::new();
        sha256_hex(serde_json::to_string(&g).unwrap_or_default().as_bytes())
    }

    /// A withdrawal is a renewal with `expires_at <= issued_at`.
    pub fn is_withdrawal(&self) -> bool {
        self.expires_at <= self.issued_at
    }

    /// The artifact for `arch`.
    pub fn artifact(&self, arch: &str) -> Option<&GrantArtifact> {
        self.artifacts.iter().find(|a| a.arch == arch)
    }

    /// The artifact whose BLAKE3 is `blake3`.
    pub fn artifact_by_blake3(&self, blake3: &str) -> Option<&GrantArtifact> {
        self.artifacts.iter().find(|a| a.blake3 == blake3)
    }

    /// The set of architectures carried.
    pub fn arches(&self) -> BTreeSet<&str> {
        self.artifacts.iter().map(|a| a.arch.as_str()).collect()
    }
}

/// A signed [`CheckoutGrant`].
pub type SignedGrant = SignedEnvelope;

/// Sign `grant` with the grant key. `grant_key_id`, the artifact order and
/// `grant_id` are filled in so the result is canonical.
pub fn sign_grant(grant: &CheckoutGrant, key: &SigningKey) -> Result<SignedGrant, LicenceError> {
    let mut g = grant.clone();
    g.artifacts.sort_by(|a, b| a.arch.cmp(&b.arch));
    g.grant_key_id = key_id(&key.verifying_key().to_bytes());
    g.grant_id = g.compute_id();
    sign_envelope(GRANT_DOMAIN, &g, key)
}

/// Verify `signed` under the bound `grant_pubkey` and require it is for
/// `local`. The key comparison is the cheap first check.
pub fn verify_grant(
    signed: &SignedGrant,
    grant_pubkey: &[u8; 32],
    local: &MeshId,
) -> Result<CheckoutGrant, LicenceError> {
    let g = verify_grant_signature(signed, grant_pubkey)?;
    if g.mesh_id != local.to_hex() {
        return Err(LicenceError::WrongMesh);
    }
    Ok(g)
}

/// Signature and shape only, with no mesh check (store load).
pub(crate) fn verify_grant_signature(
    signed: &SignedGrant,
    grant_pubkey: &[u8; 32],
) -> Result<CheckoutGrant, LicenceError> {
    if envelope_key(signed)? != *grant_pubkey {
        return Err(LicenceError::UntrustedKey);
    }
    verify_envelope(GRANT_DOMAIN, signed, grant_pubkey)?;
    let g: CheckoutGrant = parse_canonical(&signed.payload)?;
    check_shape(&g, grant_pubkey)?;
    Ok(g)
}

fn check_shape(g: &CheckoutGrant, pk: &[u8; 32]) -> Result<(), LicenceError> {
    let bad = |what: &str| Err(LicenceError::Malformed(format!("grant {what}")));
    if g.v != 1 || g.source != "cognitum" {
        return bad("version or source");
    }
    if g.grant_key_id != key_id(pk) {
        return bad("key id");
    }
    if g.grant_id != g.compute_id() {
        return bad("id");
    }
    let tokens = [&g.seed_device_id, &g.cog_id, &g.version];
    if !tokens.iter().all(|s| valid_token(s)) || g.registry.is_empty() || g.registry.len() > 256 {
        return bad("token");
    }
    let hexes = [&g.mesh_id, &g.manifest_sha256, &g.licence.ref_sha256];
    if !hexes.iter().all(|s| valid_hex32(s)) {
        return bad("hash");
    }
    if g.artifacts.is_empty() || g.artifacts.len() > MAX_GRANT_ARTIFACTS {
        return bad("artifact count");
    }
    let sorted_unique = g.artifacts.windows(2).all(|w| w[0].arch < w[1].arch);
    let each_ok = g
        .artifacts
        .iter()
        .all(|a| valid_token(&a.arch) && valid_hex32(&a.sha256) && valid_hex32(&a.blake3));
    if !sorted_unique || !each_ok {
        return bad("artifacts");
    }
    if g.expires_at.saturating_sub(g.issued_at) > MAX_GRANT_TTL_SECS {
        return Err(LicenceError::TtlTooLong);
    }
    Ok(())
}
