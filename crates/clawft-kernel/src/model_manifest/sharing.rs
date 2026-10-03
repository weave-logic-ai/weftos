//! Model weights over the artifact exchange, redistribution fail-closed
//! (ADR-099 section 6; ADR-101 section 3).
//!
//! A model's shards enter the exchange only through [`seed_model`], and only
//! when the attested manifest says `redistributable = true` **and** the
//! exchange's [`RedistributionPolicy`] allows every shard for
//! [`Audience::Seed`]. The grant carries [`GrantOrigin::OptIn`] in that case
//! and [`GrantOrigin::NotFlagged`] otherwise, so the default
//! [`crate::mesh_swarm_state::ManifestPolicy`] vetoes an un-opted-in model
//! for every audience. Models have no Cognitum provenance.
//!
//! Seeding reads the adopted files and stores their pieces in the artifact
//! store, so it is the one place weights are duplicated on disk. It is an
//! explicit, opted-in act; adoption itself never copies.

use std::fs::File;

use super::adopt::{allowed_roots, contained};
use super::body::{ModelError, ModelPackageBody, VerifiedModel, verify_model};
use super::locality::Sharing;
use super::registry::{ModelRegistry, ModelTrust};
use crate::mesh_artifact::ArtifactExchange;
use crate::mesh_artifact_types::{ArtifactId, ExchangeError};
use crate::mesh_swarm_state::{Audience, GrantInfo, GrantOrigin, RedistributionPolicy};
use crate::workload_pkg::TrustAnchors;
use crate::workload_pkg::codec::{hex_decode_exact, hex_encode};

/// Licence standing of a model, from its signed manifest. Fails closed.
pub fn grant_origin(body: &ModelPackageBody) -> GrantOrigin {
    if body.redistributable { GrantOrigin::OptIn } else { GrantOrigin::NotFlagged }
}

fn grant_info(package_id: &str, body: &ModelPackageBody, signers: Vec<String>) -> GrantInfo {
    GrantInfo {
        package_id: package_id.to_string(),
        signers,
        origin: grant_origin(body),
    }
}

/// The sharing stance the policy takes for this model: `Allowed` only when
/// the policy allows every shard to be seeded.
pub fn sharing_for(
    policy: &dyn RedistributionPolicy,
    package_id: &str,
    body: &ModelPackageBody,
) -> Sharing {
    let grants = [grant_info(package_id, body, Vec::new())];
    let all = body.shards.iter().all(|s| {
        hex_decode_exact::<32>(&s.blake3)
            .is_some_and(|h| policy.allows(&h, &grants, &Audience::Seed))
    });
    if all { Sharing::Allowed } else { Sharing::Refused }
}

/// A model seeded into the exchange.
#[derive(Debug, Clone)]
pub struct SeededModel {
    /// Package id.
    pub package_id: String,
    /// BLAKE3 hex of the manifest bytes seeded (peers fetch it by this).
    pub manifest_hash: String,
    /// Each shard path with its artifact id.
    pub shards: Vec<(String, ArtifactId)>,
}

/// Why a model was not seeded.
#[derive(Debug, thiserror::Error)]
pub enum SeedModelError {
    /// Model state, signature or hash problem.
    #[error(transparent)]
    Model(#[from] ModelError),
    /// The manifest or the exchange policy does not allow sharing.
    #[error("model {0} is not shareable: not opted in, or vetoed by the redistribution policy")]
    NotShareable(String),
    /// Package, signer or manifest is on the revocation list.
    #[error("model {0} (or its signer or manifest) is revoked")]
    Revoked(String),
    /// Exchange failure.
    #[error(transparent)]
    Exchange(#[from] ExchangeError),
}

fn signer_keys(verified: &VerifiedModel, anchors: &TrustAnchors) -> Vec<String> {
    verified
        .signers
        .iter()
        .filter_map(|s| anchors.signers.iter().find(|k| k.key_id == s.key_id))
        .map(|k| hex_encode(&k.public_key))
        .collect()
}

/// Seed an adopted model into `exchange`, if and only if sharing is allowed.
///
/// The model is resolved first (lazy hash check, so a mismatching or
/// detached model is refused), its signature is re-verified against the
/// current `anchors` (a since-removed operator key stops sharing), the
/// revocation list is consulted, and the exchange's redistribution policy
/// must allow every shard. Each shard is then streamed and verified against
/// its attested hash while seeding.
pub fn seed_model(
    exchange: &ArtifactExchange,
    registry: &ModelRegistry,
    id_or_name: &str,
    anchors: &TrustAnchors,
) -> Result<SeededModel, SeedModelError> {
    let resolved = registry.resolve_with(id_or_name, &ModelTrust::new(anchors.clone()))?;
    let (_, entry) = registry.get(id_or_name)?;
    let verified = verify_model(&entry.envelope, anchors)?;
    let package_id = verified.package_id.clone();
    let body = &verified.body;
    if sharing_for(exchange.config().redistribution.as_ref(), &package_id, body) != Sharing::Allowed {
        return Err(SeedModelError::NotShareable(package_id));
    }
    let signers = signer_keys(&verified, anchors);
    let manifest = entry.envelope.to_pretty_json().map_err(ModelError::from)?;
    let manifest_hash = *blake3::hash(&manifest).as_bytes();
    let shard_hashes = body
        .shards
        .iter()
        .filter_map(|s| hex_decode_exact::<32>(&s.blake3));
    for h in std::iter::once(manifest_hash).chain(shard_hashes) {
        if exchange.is_revoked_subject(&package_id, &signers, &h) {
            return Err(SeedModelError::Revoked(package_id));
        }
    }
    let allowed = allowed_roots(&resolved.root);
    let origin = grant_origin(body);
    let mut shards = Vec::new();
    for (shard, path) in body.shards.iter().zip(&resolved.shards) {
        let expect = (
            hex_decode_exact::<32>(&shard.blake3).expect("validated hash"),
            shard.size,
        );
        // Containment again, immediately before the open, on the resolved
        // path: a link swapped since the check must not be read.
        let real = contained(path, &allowed)?;
        let mut f = File::open(real).map_err(|e| ModelError::Io {
            path: shard.path.clone(),
            msg: e.to_string(),
        })?;
        let d = exchange.seed_reader(&mut f, Some(expect))?;
        exchange.grant_with(d.content_hash, &package_id, signers.clone(), origin.clone());
        shards.push((shard.path.clone(), d.id()));
    }
    let m = exchange.seed_bytes(&manifest)?;
    exchange.grant_with(m.content_hash, &package_id, signers, origin);
    Ok(SeededModel {
        package_id,
        manifest_hash: hex_encode(&manifest_hash),
        shards,
    })
}
