//! Adversarial tests for mesh-placement-11: a peer that lies about what
//! an artifact's pieces assemble to. Nodes are paired in-process; every
//! node has its own in-memory chain, never the operator's.

use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::artifact_store::{ArtifactStore, ArtifactType};
use crate::chain::ChainManager;
use crate::mesh_artifact::{ArtifactExchange, ExchangeConfig};

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

/// A peer that answers any meta request with `d` (announcing every
/// piece) and any request with `pieces[i]` as piece `i`, in one block.
async fn spawn_rogue(
    d: ArtifactDescriptor,
    pieces: Vec<Vec<u8>>,
) -> (InMemoryStream, JoinHandle<()>) {
    let (client, mut srv) = connected_pair().await.unwrap();
    let task = tokio::spawn(async move {
        let id = d.id();
        let tx = |m: ArtifactMsg| m.to_wire().unwrap();
        while let Ok(raw) = srv.recv().await {
            let reply = match ArtifactMsg::from_wire(&raw) {
                Ok(ArtifactMsg::MetaRequest { .. }) => {
                    let mut have = Bitfield::new(d.piece_count());
                    (0..d.piece_count()).for_each(|i| have.set(i, true));
                    vec![
                        tx(ArtifactMsg::Meta {
                            descriptor: d.clone(),
                        }),
                        tx(ArtifactMsg::Announce { id, have }),
                    ]
                }
                Ok(ArtifactMsg::Request { pieces: want, .. }) => want
                    .into_iter()
                    .map(|index| {
                        tx(ArtifactMsg::Piece {
                            id,
                            index,
                            offset: 0,
                            data: pieces[index as usize].clone(),
                        })
                    })
                    .collect(),
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

/// A peer claiming `claimed` as the content hash of one junk piece.
async fn spawn_liar(claimed: [u8; 32]) -> (InMemoryStream, JoinHandle<()>) {
    let d = ArtifactDescriptor {
        piece_size: 1024,
        total_size: JUNK.len() as u64,
        content_hash: claimed,
        pieces: vec![*blake3::hash(&JUNK).as_bytes()],
    };
    spawn_rogue(d, vec![JUNK.to_vec()]).await
}

/// An exchange over a caller-owned store, with its own in-memory chain.
fn exchange_over(id: &str, store: &Arc<ArtifactStore>) -> ArtifactExchange {
    let cfg = ExchangeConfig {
        piece_size: 1024 * 1024,
        block_size: 256 * 1024,
        ..Default::default()
    };
    let mut ex = ArtifactExchange::new(id, store.clone(), cfg).unwrap();
    ex.set_chain_manager(Arc::new(ChainManager::new(0, 1000)));
    ex
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

// ── a liar must not delete blobs it did not send (round-2 findings) ──

#[tokio::test]
async fn liar_cannot_delete_an_unrelated_store_blob() {
    let store = Arc::new(ArtifactStore::new_memory());
    let b = exchange_over("node-b", &store);
    // Another subsystem's blob, in the same store.
    let blob = vec![0x5A; 700];
    let k = store.store(&blob, ArtifactType::Generic).unwrap();
    // The liar names it as its only piece, with a false content hash, and
    // never sends a byte (B already "has" everything).
    let d = ArtifactDescriptor {
        piece_size: 1024,
        total_size: 700,
        content_hash: [7; 32],
        pieces: vec![*blake3::hash(&blob).as_bytes()],
    };
    let (s, _rogue) = spawn_rogue(d, vec![blob.clone()]).await;
    let err = b
        .fetch(&mut link("rogue", s), ArtifactKey::Content([7; 32]))
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Verification(_)), "{err:?}");
    assert!(
        store.contains(&k),
        "liar deleted an unrelated blob it never sent"
    );
    assert_eq!(store.load(&k).unwrap(), blob);
}

#[tokio::test]
async fn liar_cannot_delete_pieces_held_before_a_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let a = node("node-a");
    let (_, h) = seeded(&a, tmp.path());
    let store = Arc::new(ArtifactStore::new_memory());
    let b1 = exchange_over("node-b", &store);
    let (s, _a) = connect(&a, "node-b").await;
    let real = b1
        .fetch(&mut link("node-a", s), ArtifactKey::Content(h))
        .await
        .unwrap()
        .descriptor;
    drop(b1);

    // Restart: a fresh exchange over the same store knows no descriptors.
    let b2 = exchange_over("node-b", &store);
    let forged = ArtifactDescriptor {
        content_hash: [9; 32],
        ..real.clone()
    };
    let (s, _rogue) = spawn_rogue(forged, vec![]).await;
    let err = b2
        .fetch(&mut link("rogue", s), ArtifactKey::Content([9; 32]))
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Verification(_)), "{err:?}");
    for p in &real.pieces {
        assert!(
            store.contains(&hex_encode(p)),
            "liar deleted an honest piece"
        );
    }
    assert!(store.contains(&real.content_hex()), "whole blob kept");
}

#[tokio::test]
async fn root_key_liar_cannot_reuse_the_honest_id_or_wipe_resume_state() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (node("node-a"), node("node-b"));
    let (_, h) = seeded(&a, tmp.path());
    let real = a.ex.resolve(&ArtifactKey::Content(h)).unwrap();
    // B holds every genuine piece but has not verified the artifact yet.
    b.ex.note_pending(&real).unwrap();
    for i in 0..real.piece_count() {
        let data = a.ex.load_piece(&real, i).unwrap();
        b.ex.store_piece(&data, &real.pieces[i as usize]).unwrap();
    }
    // The liar answers Root(real id) with the real pieces, false hash.
    let forged = ArtifactDescriptor {
        content_hash: [3; 32],
        ..real.clone()
    };
    assert_ne!(forged.id(), real.id(), "the id binds the content hash");
    let (s, _rogue) = spawn_rogue(forged, vec![]).await;
    let err =
        b.ex.fetch(&mut link("rogue", s), ArtifactKey::Root(real.id()))
            .await
            .unwrap_err();
    assert!(err.to_string().contains("does not match"), "{err}");
    for p in &real.pieces {
        assert!(b.ex.store().contains(&hex_encode(p)), "resume state wiped");
    }
    // Resume from the honest origin: nothing is fetched again.
    let (s, _a) = connect(&a, "node-b").await;
    let out =
        b.ex.fetch(&mut link("node-a", s), ArtifactKey::Root(real.id()))
            .await
            .unwrap();
    assert_eq!(out.pieces_fetched, 0);
    assert_eq!(
        b.ex.read_all(&out.id).unwrap(),
        a.ex.read_all(&out.id).unwrap()
    );
}

#[tokio::test]
async fn an_idle_fetcher_does_not_hold_a_serve_session_open() {
    let cfg = ExchangeConfig {
        serve_idle_timeout: std::time::Duration::from_millis(100),
        ..Default::default()
    };
    let a = ArtifactExchange::new("node-a", Arc::new(ArtifactStore::new_memory()), cfg).unwrap();
    let (_client, mut srv) = connected_pair().await.unwrap();
    // The client connects, sends nothing and never closes.
    let served = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        a.serve(&mut srv, "idle-peer"),
    )
    .await
    .expect("serve returned on its own");
    let err = served.unwrap_err();
    assert!(err.to_string().contains("idle"), "{err}");
}
