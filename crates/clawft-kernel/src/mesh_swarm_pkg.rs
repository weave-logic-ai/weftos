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
use crate::mesh_swarm_fetch::{Expect, PeerDialer, SwarmFetchOptions};
use crate::mesh_swarm_picker::PeerCandidate;
use crate::workload_kind::KindRegistry;
use crate::workload_pkg::codec::{hex_decode_exact, hex_encode};
use crate::workload_pkg::manifest::MAX_MANIFEST_BYTES;
use crate::workload_pkg::verify::{VerifyError, verify_manifest_signatures_in};
use crate::workload_pkg::{TrustAnchors, VerifyPolicy, verify_stored_in};

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
        self.swarm_fetch_package_in(
            dialer,
            candidates,
            manifest_hash,
            anchors,
            opts,
            &KindRegistry::builtin(),
        )
        .await
    }

    /// [`Self::swarm_fetch_package`] against a caller-supplied kind registry.
    pub async fn swarm_fetch_package_in(
        self: &Arc<Self>,
        dialer: Arc<dyn PeerDialer>,
        candidates: &[PeerCandidate],
        manifest_hash: &str,
        anchors: &TrustAnchors,
        opts: &SwarmFetchOptions,
        kinds: &KindRegistry,
    ) -> Result<ExchangedPackage, PackageExchangeError> {
        let mh = hex_decode_exact::<32>(manifest_hash).ok_or_else(|| {
            VerifyError::Manifest("manifest hash must be 64 lower-case hex".into())
        })?;
        let cache = self.swarm.cache();
        if let Some(c) = &cache {
            c.pin_content(mh);
        }
        let mopts = SwarmFetchOptions {
            expect: Expect {
                max_size: Some(MAX_MANIFEST_BYTES as u64),
                ..opts.expect.clone()
            },
            ..opts.clone()
        };
        let m = self
            .swarm_fetch(
                dialer.clone(),
                candidates.to_vec(),
                ArtifactKey::Content(mh),
                &mopts,
            )
            .await?;
        if m.descriptor.total_size > MAX_MANIFEST_BYTES as u64 {
            return Err(VerifyError::Manifest("manifest too large".into()).into());
        }
        let manifest = self.read_all(&m.id)?;
        let verified = verify_manifest_signatures_in(&manifest, anchors, kinds)?;
        self.refuse_if_revoked(&verified, anchors, &mh)?;
        let grant = self.authorize(verified.clone(), mh, Vec::new(), anchors);

        if let Some(c) = &cache {
            for file in verified.body.files() {
                c.pin_content(pinned(file)?.0);
            }
        }
        let mut files = Vec::new();
        let mut all_small = true;
        for file in verified.body.files() {
            let (hash, size) = pinned(file)?;
            let fopts = SwarmFetchOptions {
                expect: Expect {
                    size: Some(size),
                    ..opts.expect.clone()
                },
                ..opts.clone()
            };
            let got = self
                .swarm_fetch(
                    dialer.clone(),
                    candidates.to_vec(),
                    ArtifactKey::Content(hash),
                    &fopts,
                )
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
            verify_stored_in(
                self.store(),
                &hex_encode(&mh),
                anchors,
                &VerifyPolicy::default(),
                kinds,
            )?;
        }
        Ok(ExchangedPackage { files, ..grant })
    }
}
