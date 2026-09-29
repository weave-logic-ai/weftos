//! Adversarial tests for mesh-placement-11: a peer that lies about what
//! an artifact's pieces assemble to. Nodes are paired in-process; every
//! node has its own in-memory chain, never the operator's.

use tokio::task::JoinHandle;

use crate::chain::{EVENT_KIND_ARTIFACT_FETCH, EVENT_KIND_ARTIFACT_SERVE};
use crate::mesh::MeshStream;
use crate::mesh_artifact_tests::{
    Node, anchors_for, connect, events, key, link, node, signed_package,
};
use crate::mesh_artifact_transfer::{FetchError, PeerLink, PeerSet};
use crate::mesh_artifact_wire::{ArtifactDescriptor, ArtifactKey, ArtifactMsg, Bitfield};
use crate::mesh_test_support::{InMemoryStream, connected_pair};
use crate::workload_pkg::codec::{hex_decode_exact, hex_encode};

const JUNK: [u8; 1024] = [0xAA; 1024];

/// A peer that answers any meta request with a self-consistent descriptor
/// claiming `claimed` as its content hash, whose one piece is junk.
async fn spawn_liar(claimed: [u8; 32]) -> (InMemoryStream, JoinHandle<()>) {
    let (client, mut srv) = connected_pair().await.unwrap();
    let task = tokio::spawn(async move {
        let d = ArtifactDescriptor {
            piece_size: 1024,
            total_size: JUNK.len() as u64,
            content_hash: claimed,
            pieces: vec![*blake3::hash(&JUNK).as_bytes()],
        };
        let id = d.id();
        let tx = |m: ArtifactMsg| m.to_wire().unwrap();
        while let Ok(raw) = srv.recv().await {
            let reply = match ArtifactMsg::from_wire(&raw) {
                Ok(ArtifactMsg::MetaRequest { .. }) => {
                    let mut have = Bitfield::new(1);
                    have.set(0, true);
                    vec![
                        tx(ArtifactMsg::Meta {
                            descriptor: d.clone(),
                        }),
                        tx(ArtifactMsg::Announce { id, have }),
                    ]
                }
                Ok(ArtifactMsg::Request { .. }) => vec![tx(ArtifactMsg::Piece {
                    id,
                    index: 0,
                    offset: 0,
                    data: JUNK.to_vec(),
                })],
                _ => vec![],
            };
            for frame in reply {
                if srv.send(&frame).await.is_err() {
                    return;
                }
            }
        }
    });
    (client, task)
}

/// A seeded package at `a`; returns (manifest hash, binary content hash).
fn seeded(a: &Node, tmp: &std::path::Path) -> (String, [u8; 32]) {
    let k = key(1);
    let pkg = signed_package(tmp, 4096, &k);
    let seeded = a.ex.seed_package_dir(&pkg, &anchors_for(&k)).unwrap();
    let bin = &seeded.verified.body.binaries["aarch64"];
    let h = hex_decode_exact::<32>(&bin.blake3).unwrap();
    (seeded.manifest_hash, h)
}

#[tokio::test]
async fn lying_descriptor_is_discarded_never_served_and_does_not_block_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b, c) = (node("node-a"), node("node-b"), node("node-c"));
    let (manifest_hash, h) = seeded(&a, tmp.path());
    let anchors = anchors_for(&key(1));
    // B already trusts h (as if its manifest verified), then asks a liar.
    b.ex.grant(h, "pkg");
    let (s, _liar) = spawn_liar(h).await;
    let err =
        b.ex.fetch(&mut link("rogue", s), ArtifactKey::Content(h))
            .await
            .unwrap_err();
    assert!(matches!(err, FetchError::Verification(_)), "{err:?}");
    assert!(b.ex.resolve(&ArtifactKey::Content(h)).is_none());
    let junk_key = hex_encode(blake3::hash(&JUNK).as_bytes());
    assert!(!b.ex.store().contains(&junk_key), "junk piece discarded");
    let failed = events(&b.chain, EVENT_KIND_ARTIFACT_FETCH);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["result"], "failed");
    assert_eq!(failed[0]["descriptors_rejected"][0]["peer"], "rogue");

    // C asks B for h: B holds nothing verified, so it refuses.
    let (s, b_serves_c) = connect(&b, "node-c").await;
    let mut peers = link("node-b", s);
    let err =
        c.ex.fetch(&mut peers, ArtifactKey::Content(h))
            .await
            .unwrap_err();
    drop(peers);
    assert!(err.to_string().contains("not servable"), "{err}");
    assert!(b_serves_c.await.unwrap().unwrap().pieces_served.is_empty());
    assert!(events(&b.chain, EVENT_KIND_ARTIFACT_SERVE).is_empty());

    // B recovers from the honest origin, then serves C the real bytes.
    let (s, _a) = connect(&a, "node-b").await;
    b.ex.fetch_package(&mut link("node-a", s), &manifest_hash, &anchors)
        .await
        .expect("honest origin still works after the liar");
    let (s, _b) = connect(&b, "node-c").await;
    let got =
        c.ex.fetch(&mut link("node-b", s), ArtifactKey::Content(h))
            .await
            .unwrap();
    assert_eq!(
        c.ex.read_all(&got.id).unwrap(),
        a.ex.read_all(&got.id).unwrap()
    );
}

#[tokio::test]
async fn liar_then_honest_peer_in_one_fetch_recovers() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (node("node-a"), node("node-b"));
    let (_, h) = seeded(&a, tmp.path());
    let (s_liar, _liar) = spawn_liar(h).await;
    let (s_a, _a) = connect(&a, "node-b").await;
    let mut peers = PeerSet::new();
    peers.push(PeerLink::new("rogue", Box::new(s_liar)));
    peers.push(PeerLink::new("node-a", Box::new(s_a)));

    let out =
        b.ex.fetch(&mut peers, ArtifactKey::Content(h))
            .await
            .unwrap();
    assert!(peers.links_mut()[0].is_dead(), "liar dropped");
    assert_eq!(out.sources, ["node-a"]);
    assert_eq!(out.descriptor.content_hash, h);
    assert_eq!(b.ex.resolve(&ArtifactKey::Content(h)).unwrap().id(), out.id);

    let fetches = events(&b.chain, EVENT_KIND_ARTIFACT_FETCH);
    assert_eq!(fetches.len(), 1);
    assert_eq!(fetches[0]["result"], "verified");
    assert_eq!(fetches[0]["source_peer"], "node-a");
    assert_eq!(fetches[0]["descriptors_rejected"][0]["peer"], "rogue");
    assert_eq!(fetches[0]["bytes"], 4096);
}
