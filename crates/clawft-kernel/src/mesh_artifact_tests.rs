//! Acceptance tests for mesh-placement-11 (swarm-ready artifact transfer).
//!
//! Nodes are paired in-process over `mesh_test_support` streams. Every node
//! gets its own in-memory `ChainManager`, never the operator chain. The
//! multi-GB test lives in `mesh_artifact_large_tests` and is `#[ignore]`d.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use serde_json::Value;
use tokio::task::JoinHandle;

use crate::artifact_store::ArtifactStore;
use crate::chain::{
    ChainManager, EVENT_KIND_ARTIFACT_FETCH, EVENT_KIND_ARTIFACT_PIECE_REJECTED,
    EVENT_KIND_ARTIFACT_SERVE,
};
use crate::mesh::{MeshError, MeshStream};
use crate::mesh_artifact::{ArtifactExchange, ExchangeConfig, ExchangeError};
use crate::mesh_artifact_transfer::{FetchError, PeerLink, PeerSet, ServeStats};
use crate::mesh_artifact_wire::{ArtifactKey, ArtifactMsg, MAX_ARTIFACT_FRAME, WireError};
use crate::mesh_test_support::{InMemoryStream, connected_pair};
use crate::workload_pkg::codec::hex_encode;
use crate::workload_pkg::{
    CogPackInput, KeyOrigin, PackageSource, TrustAnchors, key_id_for, pack_cog, sign_envelope,
    write_manifest,
};

// ── fixtures ─────────────────────────────────────────────────────

pub(crate) struct Node {
    pub ex: Arc<ArtifactExchange>,
    pub chain: Arc<ChainManager>,
}

pub(crate) fn node_with(id: &str, store: ArtifactStore, cfg: ExchangeConfig) -> Node {
    // Isolated in-memory chain per node: never the operator's chain.rvf.
    let chain = Arc::new(ChainManager::new(0, 1000));
    let mut ex = ArtifactExchange::new(id, Arc::new(store), cfg).unwrap();
    ex.set_chain_manager(chain.clone());
    Node {
        ex: Arc::new(ex),
        chain,
    }
}

fn small_cfg(piece: u64, block: usize) -> ExchangeConfig {
    ExchangeConfig {
        piece_size: piece,
        block_size: block,
        ..Default::default()
    }
}

pub(crate) fn node(id: &str) -> Node {
    node_with(
        id,
        ArtifactStore::new_memory(),
        small_cfg(1024 * 1024, 256 * 1024),
    )
}

pub(crate) fn events(chain: &ChainManager, kind: &str) -> Vec<Value> {
    chain
        .tail(chain.len())
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.payload.unwrap_or(Value::Null))
        .collect()
}

/// Start `server` serving one connection from `client_id`; return the
/// client's stream and the server task (yields its stats on close).
pub(crate) async fn connect(
    server: &Node,
    client_id: &str,
) -> (
    InMemoryStream,
    JoinHandle<Result<ServeStats, ExchangeError>>,
) {
    let (client, mut srv) = connected_pair().await.unwrap();
    let ex = server.ex.clone();
    let peer = client_id.to_string();
    let task = tokio::spawn(async move { ex.serve(&mut srv, &peer).await });
    (client, task)
}

pub(crate) fn link(peer: &str, s: impl MeshStream) -> PeerSet {
    PeerSet::single(PeerLink::new(peer, Box::new(s)))
}

pub(crate) fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

pub(crate) fn anchors_for(k: &SigningKey) -> TrustAnchors {
    let mut a = TrustAnchors::default();
    let pk = k.verifying_key().to_bytes();
    a.push_signer(&key_id_for(&pk), &hex_encode(&pk), KeyOrigin::Operator)
        .unwrap();
    a
}

const COG_TOML: &str = "[cog]\nid = \"mesh-fetch-probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\n";

/// A signed cog package whose aarch64 binary is `binary_len` bytes.
pub(crate) fn signed_package(root: &Path, binary_len: usize, k: &SigningKey) -> PathBuf {
    let cog_dir = root.join("cog");
    std::fs::create_dir_all(&cog_dir).unwrap();
    std::fs::write(cog_dir.join("cog.toml"), COG_TOML).unwrap();
    let bin: Vec<u8> = (0..binary_len).map(|i| (i * 31 % 251) as u8).collect();
    std::fs::write(root.join("bin"), &bin).unwrap();
    let input = CogPackInput {
        cog_dir,
        binaries: vec![("aarch64".into(), root.join("bin"))],
        source: PackageSource {
            repo: None,
            commit: Some("8970f99".into()),
            release_url: None,
        },
        cognitum_record: None,
    };
    let pkg = root.join("pkg");
    let mut env = pack_cog(&input, &pkg).unwrap();
    sign_envelope(&mut env, k, &key_id_for(&k.verifying_key().to_bytes())).unwrap();
    write_manifest(&pkg, &env).unwrap();
    pkg
}

/// Test stream wrapper: flips a byte in chosen `piece` frames and/or
/// drops the connection after a number of `piece` frames.
pub(crate) struct Tamper {
    inner: Option<InMemoryStream>,
    pieces_seen: u32,
    corrupt_nth: Option<u32>,
    cut_after: Option<u32>,
}

impl Tamper {
    pub(crate) fn new(inner: InMemoryStream) -> Self {
        Self {
            inner: Some(inner),
            pieces_seen: 0,
            corrupt_nth: None,
            cut_after: None,
        }
    }
    fn corrupt_first_piece(mut self) -> Self {
        self.corrupt_nth = Some(1);
        self
    }
    pub(crate) fn cut_after(mut self, pieces: u32) -> Self {
        self.cut_after = Some(pieces);
        self
    }
}

#[async_trait]
impl MeshStream for Tamper {
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
        let s = self.inner.as_mut().ok_or(MeshError::ConnectionClosed)?;
        s.send(data).await
    }
    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        if self.cut_after.is_some_and(|n| self.pieces_seen >= n) {
            self.inner.take();
            return Err(MeshError::ConnectionClosed);
        }
        let s = self.inner.as_mut().ok_or(MeshError::ConnectionClosed)?;
        let mut raw = s.recv().await?;
        let is_piece = matches!(ArtifactMsg::from_wire(&raw), Ok(ArtifactMsg::Piece { .. }));
        if is_piece {
            self.pieces_seen += 1;
            if self.corrupt_nth == Some(self.pieces_seen) {
                let last = raw.len() - 1;
                raw[last] ^= 0xff; // data is the frame's tail
            }
        }
        Ok(raw)
    }
    async fn close(&mut self) -> Result<(), MeshError> {
        self.inner.take();
        Ok(())
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        None
    }
}

// ── acceptance ───────────────────────────────────────────────────

#[tokio::test]
async fn b_fetches_a_signed_10mb_package_from_a_by_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(1);
    let pkg = signed_package(tmp.path(), 10 * 1024 * 1024, &k);
    let anchors = anchors_for(&k);
    let (a, b) = (node("node-a"), node("node-b"));
    let seeded = a.ex.seed_package_dir(&pkg, &anchors).unwrap();

    let (s, server) = connect(&a, "node-b").await;
    let mut peers = link("node-a", s);
    let got =
        b.ex.fetch_package(&mut peers, &seeded.manifest_hash, &anchors)
            .await
            .expect("B fetches and verifies the package");
    drop(peers);
    let stats = server.await.unwrap().unwrap();

    assert_eq!(got.package_id, seeded.package_id);
    let bin = &got.verified.body.binaries["aarch64"];
    assert_eq!(bin.size, 10 * 1024 * 1024);
    // The 10 MB binary is held whole (and as 10 verified 1 MiB pieces).
    assert_eq!(
        b.ex.store().load(&bin.blake3).unwrap(),
        std::fs::read(pkg.join(&bin.path)).unwrap()
    );
    assert!(stats.bytes_served >= 10 * 1024 * 1024);

    let fetches = events(&b.chain, EVENT_KIND_ARTIFACT_FETCH);
    assert_eq!(fetches.len(), 3, "manifest, cog.toml, binary: {fetches:?}");
    assert!(fetches.iter().all(|f| f["result"] == "verified"));
    assert!(fetches.iter().all(|f| f["source_peer"] == "node-a"));
    let big = fetches
        .iter()
        .find(|f| f["total_size"] == 10 * 1024 * 1024)
        .unwrap();
    assert_eq!(big["bytes"], 10 * 1024 * 1024);
    // artifact.serve: once per artifact for this peer, not per piece.
    let serves = events(&a.chain, EVENT_KIND_ARTIFACT_SERVE);
    assert_eq!(serves.len(), 3);
    assert!(serves.iter().all(|s| s["peer"] == "node-b"));
}

#[tokio::test]
async fn unsigned_content_is_not_served_and_the_failure_is_chained() {
    let (a, b) = (node("node-a"), node("node-b"));
    let d =
        a.ex.seed_bytes(b"not listed by any signed manifest")
            .unwrap();
    let (s, _server) = connect(&a, "node-b").await;
    let err =
        b.ex.fetch(&mut link("node-a", s), ArtifactKey::Content(d.content_hash))
            .await
            .unwrap_err();
    assert!(err.to_string().contains("not servable"), "{err}");
    let fetches = events(&b.chain, EVENT_KIND_ARTIFACT_FETCH);
    assert_eq!(fetches.len(), 1);
    assert_eq!(fetches[0]["result"], "failed");
    assert!(events(&a.chain, EVENT_KIND_ARTIFACT_SERVE).is_empty());

    // A package signed by an unpinned key cannot even be seeded.
    let tmp = tempfile::tempdir().unwrap();
    let pkg = signed_package(tmp.path(), 4096, &key(2));
    assert!(a.ex.seed_package_dir(&pkg, &anchors_for(&key(3))).is_err());
}

#[tokio::test]
async fn corrupted_piece_is_rejected_chained_and_requested_again() {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(1);
    let pkg = signed_package(tmp.path(), 3 * 1024 * 1024 + 17, &k);
    let (a, b) = (node("node-a"), node("node-b"));
    let seeded = a.ex.seed_package_dir(&pkg, &anchors_for(&k)).unwrap();
    let bin_id = seeded
        .files
        .iter()
        .find(|(p, _)| p.starts_with("aarch64/"))
        .unwrap()
        .1;
    let d = a.ex.descriptor(&bin_id).unwrap();

    let (s, server) = connect(&a, "node-b").await;
    let mut peers = link("node-a", Tamper::new(s).corrupt_first_piece());
    let out =
        b.ex.fetch(&mut peers, ArtifactKey::Root(bin_id))
            .await
            .unwrap();
    drop(peers);
    let stats = server.await.unwrap().unwrap();

    assert_eq!(out.pieces_rejected, 1);
    assert_eq!(out.pieces_fetched, d.piece_count());
    assert_eq!(
        b.ex.read_all(&bin_id).unwrap(),
        a.ex.read_all(&bin_id).unwrap()
    );
    // Piece 0 went over the wire twice: the bad copy and the re-request.
    assert_eq!(stats.pieces_served.iter().filter(|&&i| i == 0).count(), 2);
    let rejected = events(&b.chain, EVENT_KIND_ARTIFACT_PIECE_REJECTED);
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0]["piece_index"], 0);
    assert_eq!(rejected[0]["peer"], "node-a");
    assert_eq!(rejected[0]["artifact_id"], bin_id.to_string());
    let fetches = events(&b.chain, EVENT_KIND_ARTIFACT_FETCH);
    assert_eq!(fetches.len(), 1);
    assert_eq!(fetches[0]["result"], "verified");
    assert_eq!(fetches[0]["pieces_rejected"], 1);
}

#[tokio::test]
async fn oversize_frame_is_refused_by_server_and_fetcher() {
    let a = node("node-a");
    // Server side: a peer that ignores the per-frame cap.
    let (mut s, server) = connect(&a, "rogue").await;
    let mut raw = ((MAX_ARTIFACT_FRAME + 2) as u32).to_be_bytes().to_vec();
    raw.push(0x0B);
    raw.extend(std::iter::repeat_n(0u8, MAX_ARTIFACT_FRAME + 1));
    s.send(&raw).await.unwrap();
    let err = server.await.unwrap().unwrap_err();
    assert!(
        matches!(err, ExchangeError::Wire(WireError::FrameTooLarge { .. })),
        "{err:?}"
    );

    // Fetcher side: a holder that answers with an oversize frame.
    let b = node("node-b");
    let (client, mut rogue) = connected_pair().await.unwrap();
    let rogue_task = tokio::spawn(async move {
        let _ = rogue.recv().await; // meta_request
        rogue.send(&raw).await.unwrap();
        let _ = rogue.recv().await; // wait for the fetcher to hang up
    });
    let err =
        b.ex.fetch(&mut link("rogue", client), ArtifactKey::Content([9; 32]))
            .await
            .unwrap_err();
    assert!(matches!(err, FetchError::Incomplete { .. }));
    assert!(err.to_string().contains("too large"), "{err}");
    rogue_task.await.unwrap();
    let fetches = events(&b.chain, EVENT_KIND_ARTIFACT_FETCH);
    assert_eq!(fetches[0]["result"], "failed");
}

#[tokio::test]
async fn interrupted_transfer_resumes_without_refetching_verified_pieces() {
    let (a, b) = (
        node_with(
            "node-a",
            ArtifactStore::new_memory(),
            small_cfg(64 * 1024, 64 * 1024),
        ),
        node_with(
            "node-b",
            ArtifactStore::new_memory(),
            small_cfg(64 * 1024, 64 * 1024),
        ),
    );
    let data: Vec<u8> = (0..20 * 64 * 1024 - 100).map(|i| (i % 253) as u8).collect();
    let d = a.ex.seed_bytes(&data).unwrap();
    a.ex.grant(d.content_hash, "test-grant");
    let id = d.id();

    // First attempt: the link drops after 7 piece frames.
    let (s, _first) = connect(&a, "node-b").await;
    let err =
        b.ex.fetch(
            &mut link("node-a", Tamper::new(s).cut_after(7)),
            ArtifactKey::Root(id),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, FetchError::Incomplete { missing: 13, .. }),
        "{err:?}"
    );
    let have = b.ex.have(&id).unwrap();
    assert_eq!(have.count(), 7);

    // Resume over a new link: only the 13 missing pieces are requested.
    let (s, second) = connect(&a, "node-b").await;
    let mut peers = link("node-a", s);
    let out = b.ex.fetch(&mut peers, ArtifactKey::Root(id)).await.unwrap();
    drop(peers);
    let stats = second.await.unwrap().unwrap();
    assert_eq!(out.pieces_fetched, 13);
    assert_eq!(stats.pieces_served, (7..20).collect::<Vec<u32>>());
    assert!(stats.pieces_served.iter().all(|&i| !have.get(i)));
    assert_eq!(b.ex.read_all(&id).unwrap(), data);

    let fetches = events(&b.chain, EVENT_KIND_ARTIFACT_FETCH);
    let results: Vec<&Value> = fetches.iter().map(|f| &f["result"]).collect();
    assert_eq!(results, ["failed", "verified"]);
    // Resumed from a have bitfield: the outcome counts only the new bytes.
    assert_eq!(fetches[1]["pieces_fetched"], 13);
}

#[tokio::test]
async fn c_fetches_from_b_which_is_not_the_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(1);
    let pkg = signed_package(tmp.path(), 2 * 1024 * 1024 + 5, &k);
    let anchors = anchors_for(&k);
    let (a, b, c) = (node("node-a"), node("node-b"), node("node-c"));
    let seeded = a.ex.seed_package_dir(&pkg, &anchors).unwrap();

    let (s, _a_serves_b) = connect(&a, "node-b").await;
    b.ex.fetch_package(&mut link("node-a", s), &seeded.manifest_hash, &anchors)
        .await
        .unwrap();

    // C only has a link to B.
    let (s, b_serves_c) = connect(&b, "node-c").await;
    let mut peers = link("node-b", s);
    let got =
        c.ex.fetch_package(&mut peers, &seeded.manifest_hash, &anchors)
            .await
            .expect("C fetches the package from B");
    drop(peers);
    let stats = b_serves_c.await.unwrap().unwrap();

    assert_eq!(got.package_id, seeded.package_id);
    assert!(stats.bytes_served > 2 * 1024 * 1024);
    let fetches = events(&c.chain, EVENT_KIND_ARTIFACT_FETCH);
    assert_eq!(fetches.len(), 3);
    assert!(
        fetches
            .iter()
            .all(|f| f["source_peer"] == "node-b" && f["result"] == "verified")
    );
    let b_serves = events(&b.chain, EVENT_KIND_ARTIFACT_SERVE);
    assert!(b_serves.iter().all(|s| s["peer"] == "node-c") && b_serves.len() == 3);
    assert!(
        events(&a.chain, EVENT_KIND_ARTIFACT_SERVE)
            .iter()
            .all(|s| s["peer"] != "node-c")
    );
}

#[tokio::test]
async fn partial_holder_does_not_serve_until_verified() {
    let cfg = || small_cfg(64 * 1024, 64 * 1024);
    let a = node_with("node-a", ArtifactStore::new_memory(), cfg());
    let b = node_with("node-b", ArtifactStore::new_memory(), cfg());
    let c = node_with("node-c", ArtifactStore::new_memory(), cfg());
    let data: Vec<u8> = (0..10 * 64 * 1024).map(|i| (i % 241) as u8).collect();
    let d = a.ex.seed_bytes(&data).unwrap();
    for n in [&a, &b] {
        n.ex.grant(d.content_hash, "test-grant");
    }
    // B holds only the first 4 pieces: its descriptor is still the
    // origin's unproven claim, so B must not serve it despite the grant.
    let (s, _t) = connect(&a, "node-b").await;
    let _ =
        b.ex.fetch(
            &mut link("node-a", Tamper::new(s).cut_after(4)),
            ArtifactKey::Root(d.id()),
        )
        .await;
    assert_eq!(b.ex.have(&d.id()).unwrap().count(), 4);
    assert!(!b.ex.is_verified(&d.id()) && !b.ex.is_servable(&d));

    let (sb, from_b) = connect(&b, "node-c").await;
    let (sa, from_a) = connect(&a, "node-c").await;
    let mut peers = PeerSet::new();
    peers.push(PeerLink::new("node-b", Box::new(sb)));
    peers.push(PeerLink::new("node-a", Box::new(sa)));
    let out =
        c.ex.fetch(&mut peers, ArtifactKey::Root(d.id()))
            .await
            .unwrap();
    drop(peers);
    assert_eq!(out.sources, ["node-a"]);
    assert!(from_b.await.unwrap().unwrap().pieces_served.is_empty());
    assert_eq!(
        from_a.await.unwrap().unwrap().pieces_served,
        (0..10).collect::<Vec<_>>()
    );
    assert!(events(&b.chain, EVENT_KIND_ARTIFACT_SERVE).is_empty());
    assert_eq!(c.ex.read_all(&d.id()).unwrap(), data);
}
