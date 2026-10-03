//! Signed packages over the swarm (ADR-099 section 6, card
//! mesh-placement-25): [`ArtifactExchange::swarm_fetch_package`] is
//! [`ArtifactExchange::fetch_package`] with every artifact pulled from many
//! holders at once.
//!
//! The manifest is fetched first and its signatures are checked before any
//! file is requested. A revoked package, signer or manifest is refused. Once
//! the manifest verifies, this node may seed each listed file it holds
//! verified; a peer's descriptor alone never makes content servable.

use std::sync::Arc;

use crate::mesh_artifact::ArtifactExchange;
use crate::mesh_artifact_pkg::{ExchangedPackage, PackageExchangeError, pinned};
use crate::mesh_artifact_types::ArtifactKey;
use crate::mesh_swarm_fetch::{PeerDialer, SwarmFetchOptions};
use crate::mesh_swarm_picker::PeerCandidate;
use crate::workload_pkg::codec::{hex_decode_exact, hex_encode};
use crate::workload_pkg::manifest::MAX_MANIFEST_BYTES;
use crate::workload_pkg::verify::{VerifyError, verify_manifest_signatures};
use crate::workload_pkg::{TrustAnchors, VerifyPolicy, verify_stored};

impl ArtifactExchange {
    /// Fetch a signed package by its manifest hash from `candidates`.
    pub async fn swarm_fetch_package(
        self: &Arc<Self>,
        dialer: Arc<dyn PeerDialer>,
        candidates: &[PeerCandidate],
        manifest_hash: &str,
        anchors: &TrustAnchors,
        opts: &SwarmFetchOptions,
    ) -> Result<ExchangedPackage, PackageExchangeError> {
        let mh = hex_decode_exact::<32>(manifest_hash).ok_or_else(|| {
            VerifyError::Manifest("manifest hash must be 64 lower-case hex".into())
        })?;
        let m = self
            .swarm_fetch(dialer.clone(), candidates.to_vec(), ArtifactKey::Content(mh), opts)
            .await?;
        if m.descriptor.total_size > MAX_MANIFEST_BYTES as u64 {
            return Err(VerifyError::Manifest("manifest too large".into()).into());
        }
        let manifest = self.read_all(&m.id)?;
        let verified = verify_manifest_signatures(&manifest, anchors)?;
        self.refuse_if_revoked(&verified, anchors, &mh)?;
        let grant = self.authorize(verified.clone(), mh, Vec::new(), anchors);

        let mut files = Vec::new();
        let mut all_small = true;
        for file in verified.body.files() {
            let (hash, size) = pinned(file)?;
            let got = self
                .swarm_fetch(dialer.clone(), candidates.to_vec(), ArtifactKey::Content(hash), opts)
                .await?;
            if got.descriptor.total_size != size {
                return Err(VerifyError::HashMismatch {
                    path: file.path.clone(),
                    expected: format!("{size} bytes"),
                    actual: format!("{} bytes", got.descriptor.total_size),
                }
                .into());
            }
            all_small &= size <= self.config().materialize_limit;
            files.push((file.path.clone(), got.id));
        }
        if all_small {
            verify_stored(
                self.store(),
                &hex_encode(&mh),
                anchors,
                &VerifyPolicy::default(),
            )?;
        }
        Ok(ExchangedPackage { files, ..grant })
    }
}
