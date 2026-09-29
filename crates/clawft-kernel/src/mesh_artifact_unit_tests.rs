//! Unit tests for [`crate::mesh_artifact`] (mesh-placement-11).

use super::*;
use crate::mesh_artifact_wire::MAX_ARTIFACT_FRAME;

fn exchange(piece: u64) -> ArtifactExchange {
    let cfg = ExchangeConfig {
        piece_size: piece,
        ..Default::default()
    };
    ArtifactExchange::new("n1", Arc::new(ArtifactStore::new_memory()), cfg).unwrap()
}

#[test]
fn seeding_splits_into_pieces_and_roundtrips() {
    let x = exchange(1024);
    let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    let d = x.seed_bytes(&data).unwrap();
    assert_eq!(d.piece_count(), 5);
    assert_eq!(d.piece_len(4), 5000 - 4096);
    assert_eq!(d.content_hash, *blake3::hash(&data).as_bytes());
    assert!(x.have(&d.id()).unwrap().is_complete());
    assert_eq!(x.read_all(&d.id()).unwrap(), data);
    assert_eq!(
        x.resolve(&ArtifactKey::Content(d.content_hash))
            .unwrap()
            .id(),
        d.id()
    );
}

#[test]
fn have_follows_the_store() {
    let x = exchange(1024);
    let d = x.seed_bytes(&[3u8; 3000]).unwrap();
    // Pieces 0 and 1 are identical content (one store entry); drop the
    // distinct last piece.
    x.store().remove(&hex_encode(&d.pieces[2])).unwrap();
    let have = x.have(&d.id()).unwrap();
    assert_eq!(have.missing().collect::<Vec<_>>(), vec![2]);
    assert!(x.read_all(&d.id()).is_err());
}

#[test]
fn seeding_with_wrong_expectation_registers_nothing() {
    let x = exchange(1024);
    let err = x.seed_reader(&mut &b"abc"[..], Some(([0; 32], 3)));
    assert!(matches!(err, Err(ExchangeError::Mismatch(_))));
    assert!(
        x.resolve(&ArtifactKey::Content(*blake3::hash(b"abc").as_bytes()))
            .is_none()
    );
}

#[test]
fn nothing_is_servable_without_a_grant() {
    let x = exchange(1024);
    let d = x.seed_bytes(b"payload").unwrap();
    assert!(!x.is_servable(&d));
    x.grant(d.content_hash, "pkg");
    assert!(x.is_servable(&d));
}

#[test]
fn config_limits_are_enforced() {
    let store = Arc::new(ArtifactStore::new_memory());
    let bad_block = ExchangeConfig {
        block_size: MAX_ARTIFACT_FRAME,
        ..Default::default()
    };
    assert!(ArtifactExchange::new("n", store.clone(), bad_block).is_err());
    let bad_piece = ExchangeConfig {
        piece_size: 10,
        ..Default::default()
    };
    assert!(ArtifactExchange::new("n", store, bad_piece).is_err());
}

/// A descriptor claiming `claimed` content but listing `junk`'s piece.
fn liar(claimed: [u8; 32], junk: &[u8]) -> ArtifactDescriptor {
    ArtifactDescriptor {
        piece_size: 1024,
        total_size: junk.len() as u64,
        content_hash: claimed,
        pieces: vec![*blake3::hash(junk).as_bytes()],
    }
}

#[test]
fn pending_descriptors_are_neither_resolvable_by_content_nor_servable() {
    let x = exchange(1024);
    let claimed = *blake3::hash(b"the real thing").as_bytes();
    x.grant(claimed, "pkg");
    let junk = [0xAA; 1024];
    let d = liar(claimed, &junk);
    x.note_pending(&d).unwrap();
    x.store_piece(&junk, &d.pieces[0]).unwrap();
    assert!(x.have(&d.id()).unwrap().is_complete());
    assert!(x.resolve(&ArtifactKey::Content(claimed)).is_none());
    assert!(x.resolve(&ArtifactKey::Root(d.id())).is_none());
    assert!(!x.is_servable(&d));
}

#[test]
fn failed_promotion_discards_the_descriptor_and_unshared_pieces() {
    let x = exchange(1024);
    let honest = x.seed_bytes(&[7u8; 1024]).unwrap();
    let junk = [0xAA; 1024];
    // Lies about content and also lists the honest piece (shared).
    let mut d = liar(honest.content_hash, &junk);
    d.pieces.push(honest.pieces[0]);
    d.total_size = 2048;
    x.note_pending(&d).unwrap();
    assert!(x.store_piece(&junk, &d.pieces[0]).unwrap(), "newly written");
    // This fetch wrote the junk piece; the honest piece was already held.
    let written = HashSet::from([d.pieces[0]]);

    assert!(matches!(
        x.promote(&d, &written),
        Err(ExchangeError::Mismatch(_))
    ));
    assert!(x.have(&d.id()).is_none(), "pending descriptor dropped");
    assert!(
        !x.store().contains(&hex_encode(&d.pieces[0])),
        "junk piece dropped"
    );
    // The honest artifact is untouched and still resolves.
    assert!(x.have(&honest.id()).unwrap().is_complete());
    let key = ArtifactKey::Content(honest.content_hash);
    assert_eq!(x.resolve(&key).unwrap().id(), honest.id());
}

#[test]
fn a_peer_descriptor_cannot_replace_a_verified_one() {
    let x = exchange(1024);
    let honest = x.seed_bytes(&[5u8; 2000]).unwrap();
    // Same piece list but a false content hash: a different id.
    let forged = ArtifactDescriptor {
        content_hash: [1; 32],
        ..honest.clone()
    };
    assert_ne!(forged.id(), honest.id());
    x.note_pending(&forged).unwrap();
    assert!(x.promote(&forged, &HashSet::new()).is_err());
    assert_eq!(x.descriptor(&honest.id()).unwrap(), honest);
    assert!(x.have(&honest.id()).unwrap().is_complete());
}

#[test]
fn failed_promotion_never_removes_blobs_it_did_not_write() {
    let x = exchange(1024);
    // Held before the fetch, owned by nobody the exchange knows about.
    let blob = [0x42u8; 900];
    let k = x.store().store(&blob, ArtifactType::Generic).unwrap();
    let d = liar([8; 32], &blob);
    x.note_pending(&d).unwrap();
    assert!(!x.store_piece(&blob, &d.pieces[0]).unwrap(), "already held");
    assert!(x.promote(&d, &HashSet::new()).is_err());
    assert!(x.have(&d.id()).is_none(), "pending descriptor dropped");
    assert!(x.store().contains(&k), "pre-existing blob kept");
}
