//! Multi-GB artifact transfer (mesh-placement-11). Off the default run:
//!
//! ```text
//! cargo test -p clawft-kernel --release --lib mesh_artifact_large_tests -- --ignored --nocapture
//! ```
//!
//! `WEFTOS_ARTIFACT_LARGE_BYTES` overrides the size (default just over
//! 2 GiB). Both nodes use file-backed stores in a tempdir that is removed
//! when the test ends; chains are in-memory.

use std::collections::BTreeMap;
use std::io::Read;

use crate::artifact_store::ArtifactStore;
use crate::chain::EVENT_KIND_ARTIFACT_FETCH;
use crate::mesh_artifact::ExchangeConfig;
use crate::mesh_artifact_tests::{Tamper, anchors_for, connect, events, key, link, node_with};
use crate::mesh_artifact_wire::ArtifactKey;
use crate::workload_pkg::codec::hex_encode;
use crate::workload_pkg::manifest::binary_path;
use crate::workload_pkg::{
    CogPackageBody, FileRef, KIND_COG, ManifestEnvelope, PackageSource, key_id_for, sign_envelope,
};

/// Deterministic pseudo-random content, generated on the fly.
struct Synthetic {
    state: u64,
    left: u64,
}

impl Synthetic {
    fn new(len: u64) -> Self {
        Self {
            state: 0x9e37_79b9_7f4a_7c15,
            left: len,
        }
    }
}

impl Read for Synthetic {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = (buf.len() as u64).min(self.left) as usize;
        for chunk in buf[..n].chunks_mut(8) {
            self.state ^= self.state << 13;
            self.state ^= self.state >> 7;
            self.state ^= self.state << 17;
            let bytes = self.state.to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
        self.left -= n as u64;
        Ok(n)
    }
}

fn hash_synthetic(len: u64) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    let mut r = Synthetic::new(len);
    let mut buf = vec![0u8; 8 * 1024 * 1024];
    loop {
        let n = r.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    *h.finalize().as_bytes()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "multi-GB transfer; run with --ignored (see module docs)"]
async fn multi_gb_file_transfer_is_interrupted_and_resumed() {
    let size: u64 = std::env::var("WEFTOS_ARTIFACT_LARGE_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2 * 1024 * 1024 * 1024 + 3 * 1024 * 1024 + 7);
    let tmp = tempfile::tempdir().unwrap();
    let cfg = ExchangeConfig::default(); // 64 MiB pieces, 1 MiB blocks
    let blocks_per_piece = (cfg.piece_size / cfg.block_size as u64) as u32;
    let a = node_with(
        "node-a",
        ArtifactStore::new_file(tmp.path().join("a")),
        cfg.clone(),
    );
    let b = node_with("node-b", ArtifactStore::new_file(tmp.path().join("b")), cfg);

    // A signed manifest pins the synthetic payload by size and BLAKE3.
    let id = "big-payload";
    let cog_toml = format!("[cog]\nid = \"{id}\"\nname = \"Big\"\nversion = \"0.1.0\"\n");
    let content = hash_synthetic(size);
    let bin = FileRef {
        path: binary_path("aarch64", id),
        size,
        blake3: hex_encode(&content),
    };
    let body = CogPackageBody {
        id: id.into(),
        version: "0.1.0".into(),
        cog_toml: FileRef {
            path: "cog.toml".into(),
            size: cog_toml.len() as u64,
            blake3: blake3::hash(cog_toml.as_bytes()).to_hex().to_string(),
        },
        binaries: BTreeMap::from([("aarch64".to_string(), bin.clone())]),
        source: PackageSource {
            repo: None,
            commit: Some("8970f99".into()),
            release_url: None,
        },
        attestations: vec![],
    };
    let k = key(7);
    let anchors = anchors_for(&k);
    let mut env = ManifestEnvelope::new(KIND_COG, serde_json::to_value(&body).unwrap());
    sign_envelope(&mut env, &k, &key_id_for(&k.verifying_key().to_bytes())).unwrap();
    let manifest = env.to_pretty_json().unwrap();

    let seeded =
        a.ex.seed_package(&manifest, &anchors, &mut |f| {
            Ok(if f.path == "cog.toml" {
                Box::new(std::io::Cursor::new(cog_toml.clone().into_bytes())) as Box<dyn Read>
            } else {
                Box::new(Synthetic::new(size))
            })
        })
        .unwrap();
    let bin_d = a.ex.resolve(&ArtifactKey::Content(content)).unwrap();
    let total_pieces = bin_d.piece_count();

    // First attempt drops the link about half way through the payload
    // (+2 frames for the manifest and cog.toml).
    let cut = 2 + (total_pieces / 2) * blocks_per_piece + blocks_per_piece / 2;
    let (s, _first) = connect(&a, "node-b").await;
    let err =
        b.ex.fetch_package(
            &mut link("node-a", Tamper::new(s).cut_after(cut)),
            &seeded.manifest_hash,
            &anchors,
        )
        .await
        .unwrap_err();
    eprintln!("interrupted: {err}");
    let held = b.ex.have(&bin_d.id()).unwrap();
    assert_eq!(held.count(), total_pieces / 2);

    // Resume from the have bitfield.
    let (s, second) = connect(&a, "node-b").await;
    let mut peers = link("node-a", s);
    let got =
        b.ex.fetch_package(&mut peers, &seeded.manifest_hash, &anchors)
            .await
            .expect("resumed fetch completes and verifies");
    drop(peers);
    let stats = second.await.unwrap().unwrap();

    assert_eq!(got.package_id, seeded.package_id);
    assert!(stats.pieces_served.iter().all(|&i| !held.get(i)));
    assert_eq!(
        stats.pieces_served.len() as u32,
        total_pieces - held.count()
    );
    assert!(b.ex.have(&bin_d.id()).unwrap().is_complete());
    let n = b.ex.read_to(&bin_d.id(), &mut |_| Ok(())).unwrap();
    assert_eq!(n, size);

    let fetches = events(&b.chain, EVENT_KIND_ARTIFACT_FETCH);
    let big: Vec<_> = fetches
        .iter()
        .filter(|f| {
            f["content_hash"] == hex_encode(&content)
                || f["key"]
                    .as_str()
                    .is_some_and(|k| k.contains(&hex_encode(&content)))
        })
        .collect();
    assert_eq!(big.len(), 2, "{big:?}");
    assert_eq!(big[0]["result"], "failed");
    assert_eq!(big[1]["result"], "verified");
    assert_eq!(big[1]["pieces_fetched"], total_pieces - held.count());
    drop(tmp); // removes both multi-GB stores
}
