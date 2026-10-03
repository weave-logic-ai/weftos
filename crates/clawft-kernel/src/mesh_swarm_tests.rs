//! Acceptance tests for mesh-placement-25 (swarm artifact distribution).
//!
//! Four in-process nodes over `mesh_test_support` streams. Every node has
//! its own in-memory store and isolated `ChainManager`, never the
//! operator's. Link speeds are injected per peer (the serving side paces
//! `piece` frames) and the tests run under paused tokio time, so elapsed
//! times are exact and deterministic: they model the injected rates, not a
//! real network.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::artifact_store::ArtifactStore;
use crate::chain::{
    EVENT_KIND_ARTIFACT_EVICT, EVENT_KIND_ARTIFACT_FETCH, EVENT_KIND_ARTIFACT_PEER_BAN,
    EVENT_KIND_ARTIFACT_PIECE_REJECTED, EVENT_KIND_ARTIFACT_REVOKE, EVENT_KIND_ARTIFACT_SEED,
};
use crate::mesh::{MeshError, MeshStream};
use crate::mesh_artifact::{ArtifactExchange, ExchangeConfig, ExchangeError};
use crate::mesh_artifact_pkg::ExchangedPackage;
use crate::mesh_artifact_tests::{
    Node, anchors_for, connect, events, key, node_with, signed_package,
};
use crate::mesh_artifact_types::ArtifactKey;
use crate::mesh_swarm_fetch::{PeerDialer, SwarmFetchOptions};
use crate::mesh_swarm_picker::PeerCandidate;
use crate::mesh_test_support::{InMemoryStream, connected_pair};
use crate::revocation::{RevocationKind, RevocationList};
use crate::workload_pkg::TrustAnchors;
use crate::workload_pkg::codec::hex_encode;

const MIB: u64 = 1024 * 1024;
/// Binary size of the test package: twelve 1 MiB pieces.
const BIN: usize = 12 * 1024 * 1024;

// ── fixtures ─────────────────────────────────────────────────────

fn cfg() -> ExchangeConfig {
    ExchangeConfig {
        piece_size: MIB,
        block_size: 256 * 1024,
        ..Default::default()
    }
}

fn swarm_node(id: &str, c: ExchangeConfig) -> Node {
    node_with(id, ArtifactStore::new_memory(), c)
}

/// How one simulated peer behaves when served from.
#[derive(Clone, Default)]
struct Spec {
    /// Upload speed injected on this peer's link, bytes per second.
    rate: Option<u64>,
    /// Flip a byte in the first `piece` frame this peer sends.
    corrupt_first: bool,
    /// Drop the connection after this many `piece` frames (blocks).
    die_after_blocks: Option<u32>,
}

/// Serving-side stream wrapper: paces, corrupts or kills `piece` frames.
struct Behaving {
    inner: Option<InMemoryStream>,
    spec: Spec,
    blocks: u32,
    corrupted: bool,
}

fn is_piece_frame(raw: &[u8]) -> bool {
    raw.len() > 5 && raw[4] == 0x0C && raw[5] == 0x12
}

#[async_trait]
impl MeshStream for Behaving {
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
        let mut data = data.to_vec();
        if is_piece_frame(&data) {
            if self.spec.die_after_blocks.is_some_and(|n| self.blocks >= n) {
                self.inner.take();
                return Err(MeshError::ConnectionClosed);
            }
            self.blocks += 1;
            if self.spec.corrupt_first && !self.corrupted {
                self.corrupted = true;
                let last = data.len() - 1;
                data[last] ^= 0xff;
            }
            if let Some(rate) = self.spec.rate {
                tokio::time::sleep(Duration::from_secs_f64(data.len() as f64 / rate as f64)).await;
            }
        }
        self.inner
            .as_mut()
            .ok_or(MeshError::ConnectionClosed)?
            .send(&data)
            .await
    }
    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        self.inner
            .as_mut()
            .ok_or(MeshError::ConnectionClosed)?
            .recv()
            .await
    }
    async fn close(&mut self) -> Result<(), MeshError> {
        self.inner.take();
        Ok(())
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        None
    }
}

/// In-process mesh: `dial(peer)` connects to that peer's serve loop.
struct Net {
    /// Id the dialing node presents to the servers.
    client: String,
    peers: HashMap<String, (Arc<ArtifactExchange>, Spec)>,
    dials: Mutex<Vec<String>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Net {
    fn new(client: &str) -> Self {
        Self {
            client: client.into(),
            peers: HashMap::new(),
            dials: Mutex::new(Vec::new()),
            tasks: Mutex::new(Vec::new()),
        }
    }
    fn with(mut self, n: &Node, spec: Spec) -> Self {
        self.peers
            .insert(n.ex.node_id().to_string(), (n.ex.clone(), spec));
        self
    }
    fn dial_log(&self) -> Vec<String> {
        self.dials.lock().unwrap().clone()
    }
    fn dialed(&self, peer: &str) -> usize {
        self.dials.lock().unwrap().iter().filter(|p| *p == peer).count()
    }
}

#[async_trait]
impl PeerDialer for Net {
    async fn dial(&self, peer: &str) -> Result<Box<dyn MeshStream>, MeshError> {
        self.dials.lock().unwrap().push(peer.to_string());
        let (ex, spec) = self
            .peers
            .get(peer)
            .cloned()
            .ok_or_else(|| MeshError::PeerNotConnected(peer.into()))?;
        let (client, server) = connected_pair().await?;
        let me = self.client.clone();
        let task = tokio::spawn(async move {
            let mut s = Behaving {
                inner: Some(server),
                spec,
                blocks: 0,
                corrupted: false,
            };
            let _ = ex.serve(&mut s, &me).await;
        });
        self.tasks.lock().unwrap().push(task);
        Ok(Box::new(client))
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    pkg: PathBuf,
    anchors: TrustAnchors,
}

fn fixture(binary_len: usize) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(1);
    let pkg = signed_package(tmp.path(), binary_len, &k);
    Fixture {
        anchors: anchors_for(&k),
        pkg,
        _tmp: tmp,
    }
}

/// `n` seeders named `seed-0..`, each holding the package.
fn seeders(n: usize, fx: &Fixture) -> (Vec<Node>, ExchangedPackage) {
    let nodes: Vec<Node> = (0..n).map(|i| swarm_node(&format!("seed-{i}"), cfg())).collect();
    let mut pkg = None;
    for s in &nodes {
        pkg = Some(s.ex.seed_package_dir(&fx.pkg, &fx.anchors).unwrap());
    }
    (nodes, pkg.unwrap())
}

fn cands(nodes: &[Node]) -> Vec<PeerCandidate> {
    nodes
        .iter()
        .map(|n| PeerCandidate::new(n.ex.node_id()))
        .collect()
}

fn binary_hash(p: &ExchangedPackage) -> [u8; 32] {
    let b = &p.verified.body.binaries["aarch64"];
    crate::workload_pkg::codec::hex_decode_exact::<32>(&b.blake3).unwrap()
}

fn opts() -> SwarmFetchOptions {
    SwarmFetchOptions::default()
}

// ── acceptance ───────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn three_seeders_serve_one_fetch_faster_than_one() {
    let fx = fixture(BIN);
    let rate = 4 * MIB; // injected per-peer upload speed
    let mut elapsed = Vec::new();
    for n in 1..=4usize {
        let (seeds, pkg) = seeders(n, &fx);
        let mut net = Net::new("leech");
        for s in &seeds {
            net = net.with(s, Spec { rate: Some(rate), ..Default::default() });
        }
        let net = Arc::new(net);
        let leech = swarm_node("leech", cfg());
        let t0 = Instant::now();
        let out = leech
            .ex
            .swarm_fetch(net.clone(), cands(&seeds), ArtifactKey::Content(binary_hash(&pkg)), &opts())
            .await
            .unwrap();
        let took = t0.elapsed();
        assert_eq!(out.bytes_fetched, BIN as u64);
        assert_eq!(out.sources.len(), n.min(4), "every seeder contributes: {:?}", out.sources);
        assert_eq!(leech.ex.read_all(&out.id).unwrap().len(), BIN);
        println!(
            "swarm throughput: {n} seeder(s) x {} MiB/s -> {} MiB in {:.2} s = {:.1} MiB/s",
            rate / MIB,
            BIN as u64 / MIB,
            took.as_secs_f64(),
            (BIN as u64 / MIB) as f64 / took.as_secs_f64()
        );
        elapsed.push(took);
    }
    // Twelve pieces over 4 MiB/s links: 3 s from one source, 1 s from three.
    assert!(elapsed[2] * 2 < elapsed[0], "3 seeders {:?} vs 1 {:?}", elapsed[2], elapsed[0]);
    assert!(elapsed[1] < elapsed[0] && elapsed[3] <= elapsed[2]);
}

#[tokio::test(start_paused = true)]
async fn a_seeder_killed_mid_transfer_does_not_fail_the_fetch() {
    let fx = fixture(BIN);
    let (seeds, pkg) = seeders(3, &fx);
    let net = Arc::new(
        Net::new("leech")
            .with(&seeds[0], Spec { rate: Some(4 * MIB), ..Default::default() })
            // dies after 6 blocks: one and a half pieces, i.e. mid-piece
            .with(&seeds[1], Spec { rate: Some(4 * MIB), die_after_blocks: Some(6), ..Default::default() })
            .with(&seeds[2], Spec { rate: Some(4 * MIB), ..Default::default() }),
    );
    let leech = swarm_node("leech", cfg());
    let out = leech
        .ex
        .swarm_fetch(net, cands(&seeds), ArtifactKey::Content(binary_hash(&pkg)), &opts())
        .await
        .expect("the other seeders finish the transfer");
    assert_eq!(out.bytes_fetched, BIN as u64);
    assert_eq!(
        leech.ex.read_all(&out.id).unwrap(),
        std::fs::read(fx.pkg.join(&pkg.verified.body.binaries["aarch64"].path)).unwrap()
    );
    let fetch = &events(&leech.chain, EVENT_KIND_ARTIFACT_FETCH)[0];
    assert_eq!(fetch["result"], "verified");
    let lost = fetch["peers_lost"].as_array().unwrap();
    assert!(lost.iter().any(|l| l["peer"] == "seed-1"), "seed-1 lost: {lost:?}");
}

#[tokio::test(start_paused = true)]
async fn a_lost_source_is_replaced_from_remaining_candidates() {
    // max_sources = 1: only one seeder at a time; it dies, the next takes over.
    let fx = fixture(BIN);
    let (seeds, pkg) = seeders(2, &fx);
    let net = Arc::new(
        Net::new("leech")
            .with(&seeds[0], Spec { die_after_blocks: Some(5), ..Default::default() })
            .with(&seeds[1], Spec::default()),
    );
    let leech = swarm_node("leech", ExchangeConfig { max_sources: 1, ..cfg() });
    let out = leech
        .ex
        .swarm_fetch(net.clone(), cands(&seeds), ArtifactKey::Content(binary_hash(&pkg)), &opts())
        .await
        .unwrap();
    assert_eq!(out.sources, vec!["seed-0".to_string(), "seed-1".to_string()]);
    assert_eq!(net.dialed("seed-1"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_node_that_finished_seeds_a_fourth() {
    let fx = fixture(BIN);
    let origin = swarm_node("origin", cfg());
    let origin_pkg = origin.ex.seed_package_dir(&fx.pkg, &fx.anchors).unwrap();
    let b = swarm_node("node-b", cfg());
    let c = swarm_node("node-c", cfg());

    // B fetches the signed package from the origin and starts seeding it.
    let net_b = Arc::new(Net::new("node-b").with(&origin, Spec::default()));
    let got = b
        .ex
        .swarm_fetch_package(net_b, &cands(&[origin]), &origin_pkg.manifest_hash, &fx.anchors, &opts())
        .await
        .unwrap();
    assert_eq!(got.package_id, origin_pkg.package_id);
    let seeded = events(&b.chain, EVENT_KIND_ARTIFACT_SEED);
    assert_eq!(seeded.len(), 3, "manifest, cog.toml, binary: {seeded:?}");
    assert!(seeded.iter().all(|s| s["package_id"] == origin_pkg.package_id.as_str()));

    // The origin is gone; C fetches the same package from B alone.
    let net_c = Arc::new(Net::new("node-c").with(&b, Spec::default()));
    let got_c = c
        .ex
        .swarm_fetch_package(net_c, &cands(std::slice::from_ref(&b)), &origin_pkg.manifest_hash, &fx.anchors, &opts())
        .await
        .expect("C fetches from B, which is not the origin");
    assert_eq!(got_c.package_id, origin_pkg.package_id);
    for e in events(&c.chain, EVENT_KIND_ARTIFACT_FETCH) {
        assert_eq!(e["result"], "verified");
        assert_eq!(e["source_peer"], "node-b");
    }
    // ... and C is now a seeder too.
    assert_eq!(events(&c.chain, EVENT_KIND_ARTIFACT_SEED).len(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_corrupt_piece_bans_its_sender_and_is_fetched_elsewhere() {
    let fx = fixture(BIN);
    let (seeds, pkg) = seeders(3, &fx);
    let net = Arc::new(
        Net::new("leech")
            .with(&seeds[0], Spec { corrupt_first: true, ..Default::default() })
            .with(&seeds[1], Spec::default())
            .with(&seeds[2], Spec::default()),
    );
    let leech = swarm_node("leech", cfg());
    let out = leech
        .ex
        .swarm_fetch(net.clone(), cands(&seeds), ArtifactKey::Content(binary_hash(&pkg)), &opts())
        .await
        .unwrap();
    assert_eq!(out.pieces_rejected, 1);
    assert!(!out.sources.contains(&"seed-0".to_string()), "banned peer supplied nothing: {:?}", out.sources);
    assert!(leech.ex.is_banned("seed-0"));
    assert_eq!(leech.ex.banned_peers().len(), 1);
    assert_eq!(leech.ex.read_all(&out.id).unwrap().len(), BIN);

    let rejected = events(&leech.chain, EVENT_KIND_ARTIFACT_PIECE_REJECTED);
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0]["peer"], "seed-0");
    let bans = events(&leech.chain, EVENT_KIND_ARTIFACT_PEER_BAN);
    assert_eq!(bans.len(), 1);
    assert_eq!(bans[0]["peer"], "seed-0");

    // A banned peer is not dialed again ...
    let again = leech
        .ex
        .swarm_fetch(net.clone(), vec![PeerCandidate::new("seed-0")], ArtifactKey::Content([9; 32]), &opts())
        .await;
    assert!(again.is_err());
    // ... and is refused when it asks us for pieces.
    let (s, server) = connect(&leech, "seed-0").await;
    drop(s);
    assert_eq!(
        server.await.unwrap().unwrap_err(),
        ExchangeError::Banned("seed-0".into())
    );
}

#[tokio::test(start_paused = true)]
async fn per_node_bandwidth_caps_are_enforced() {
    let fx = fixture(4 * 1024 * 1024);

    // Download cap: 4 MiB at 1 MiB/s takes at least 4 s however fast the peer.
    let (seeds, pkg) = seeders(1, &fx);
    let net = Arc::new(Net::new("leech").with(&seeds[0], Spec::default()));
    let capped = swarm_node("leech", ExchangeConfig { download_bytes_per_sec: Some(MIB), ..cfg() });
    let t0 = Instant::now();
    capped
        .ex
        .swarm_fetch(net, cands(&seeds), ArtifactKey::Content(binary_hash(&pkg)), &opts())
        .await
        .unwrap();
    assert!(t0.elapsed() >= Duration::from_secs(4), "download cap: {:?}", t0.elapsed());

    // Upload cap on the seeder: serving 4 MiB at 2 MiB/s takes at least 2 s,
    // and three leechers share the one budget.
    let slow_seed = swarm_node("slow-seed", ExchangeConfig { upload_bytes_per_sec: Some(2 * MIB), ..cfg() });
    let p2 = slow_seed.ex.seed_package_dir(&fx.pkg, &fx.anchors).unwrap();
    let net = Arc::new(Net::new("leech").with(&slow_seed, Spec::default()));
    let leech = swarm_node("leech", cfg());
    let t1 = Instant::now();
    leech
        .ex
        .swarm_fetch(net, cands(std::slice::from_ref(&slow_seed)), ArtifactKey::Content(binary_hash(&p2)), &opts())
        .await
        .unwrap();
    assert!(t1.elapsed() >= Duration::from_secs(2), "upload cap: {:?}", t1.elapsed());
    assert!(t1.elapsed() < Duration::from_secs(3), "and not much slower: {:?}", t1.elapsed());
}

#[tokio::test(start_paused = true)]
async fn locality_and_measured_speed_choose_the_sources() {
    let fx = fixture(BIN);
    let (seeds, pkg) = seeders(4, &fx);
    let net = Arc::new(
        Net::new("leech")
            .with(&seeds[0], Spec { rate: Some(8 * MIB), ..Default::default() })
            .with(&seeds[1], Spec { rate: Some(MIB), ..Default::default() })
            .with(&seeds[2], Spec::default())
            .with(&seeds[3], Spec::default()),
    );
    let leech = swarm_node("leech", ExchangeConfig { max_sources: 2, ..cfg() });
    // seed-2 and seed-3 share the leech's LAN: they are dialed, the others are not.
    let c = vec![
        PeerCandidate::new("seed-0").on_lan("cloud"),
        PeerCandidate::new("seed-1").on_lan("cloud"),
        PeerCandidate::new("seed-2").on_lan("home"),
        PeerCandidate::new("seed-3").on_lan("home"),
    ];
    let o = SwarmFetchOptions { local_lan: Some("home".into()) };
    leech
        .ex
        .swarm_fetch(net.clone(), c.clone(), ArtifactKey::Content(binary_hash(&pkg)), &o)
        .await
        .unwrap();
    assert_eq!((net.dialed("seed-2"), net.dialed("seed-3")), (1, 1));
    assert_eq!((net.dialed("seed-0"), net.dialed("seed-1")), (0, 0));

    // Fetch from the two WAN peers so this node measures their links; the
    // next fetch dials the faster one first even when it is listed last.
    let big = ArtifactKey::Content(binary_hash(&pkg));
    let manifest = ArtifactKey::Content(
        crate::workload_pkg::codec::hex_decode_exact::<32>(&pkg.manifest_hash).unwrap(),
    );
    let wan = vec![PeerCandidate::new("seed-1"), PeerCandidate::new("seed-0")];
    let leech2 = swarm_node("leech2", ExchangeConfig { max_sources: 2, ..cfg() });
    leech2.ex.swarm_fetch(net.clone(), wan.clone(), big, &opts()).await.unwrap();
    let (fast, slow) = (
        leech2.ex.link_stats().bytes_per_sec("seed-0").unwrap(),
        leech2.ex.link_stats().bytes_per_sec("seed-1").unwrap(),
    );
    assert!(fast > 4.0 * slow, "measured {fast} vs {slow}");
    let before = net.dial_log().len();
    leech2.ex.swarm_fetch(net.clone(), wan, manifest, &opts()).await.unwrap();
    assert_eq!(net.dial_log()[before], "seed-0", "faster measured link first");
}

// ── who has / advertisement ──────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn who_has_finds_holders_without_a_tracker() {
    let fx = fixture(2 * 1024 * 1024);
    let (seeds, pkg) = seeders(2, &fx);
    let empty = swarm_node("empty", cfg());
    let net = Arc::new(
        Net::new("asker")
            .with(&seeds[0], Spec::default())
            .with(&seeds[1], Spec::default())
            .with(&empty, Spec::default()),
    );
    let asker = swarm_node("asker", cfg());
    let key = ArtifactKey::Content(binary_hash(&pkg));
    let neighbours = vec![PeerCandidate::new("seed-1"), PeerCandidate::new("empty"), PeerCandidate::new("seed-0")];
    let holders = asker.ex.who_has(net.clone(), &neighbours, key).await;
    let ids: Vec<_> = holders.iter().map(|h| h.peer.as_str()).collect();
    assert_eq!(ids, vec!["seed-0", "seed-1"]);
    assert!(holders.iter().all(|h| h.have.is_complete()));
    let found = asker
        .ex
        .find_holders(net, &crate::node_facts::NodeFactsCache::new(), &neighbours, key, 0)
        .await;
    assert_eq!(found.len(), 2);
}

#[test]
fn held_artifacts_are_advertised_as_capabilities_and_found_in_facts() {
    use crate::mesh_swarm_cache::{ArtifactCache, ArtifactKind, CacheConfig};
    use crate::mesh_swarm_lookup::{LAN_CAPABILITY, holders_from_facts};
    use crate::node_facts_advert::sign_node_facts;
    use clawft_types::placement::{Capability, CapabilityId, NodeFacts, Provenance, TrustTier};

    let fx = fixture(2 * 1024 * 1024);
    let (seeds, pkg) = seeders(1, &fx);
    let holder = &seeds[0];
    let cache = ArtifactCache::new(holder.ex.clone(), CacheConfig { max_bytes: 1 << 30 });
    let bin_hash = binary_hash(&pkg);
    let bin_id = holder.ex.resolve(&ArtifactKey::Content(bin_hash)).unwrap().id();
    assert!(cache.set_kind(&bin_id, ArtifactKind::Model));

    let sk = key(7);
    let node_id = crate::node_registry::node_id_from_pubkey(&sk.verifying_key().to_bytes());
    let mut facts = NodeFacts::new(node_id.clone(), 1_000, 600, 1);
    facts.capabilities.push(
        Capability::new(CapabilityId::new(LAN_CAPABILITY).unwrap(), Provenance::Probed)
            .with_attr("lan_id", "home"),
    );
    cache.advertise(&mut facts);
    let hex = hex_encode(&bin_hash);
    let held = facts
        .capabilities
        .iter()
        .find(|c| c.id.as_str() == format!("store.artifact.{}", &hex[..16]))
        .expect("store.artifact.* capability");
    assert_eq!(held.provenance, Provenance::Probed);
    let shards = facts.find("model.present").next().expect("model.present");
    assert!(format!("{:?}", shards.attrs["shards"]).contains(&hex));
    // Advertising twice replaces, never duplicates.
    cache.advertise(&mut facts);
    assert_eq!(
        facts.capabilities.iter().filter(|c| c.id.is_under("store.artifact")).count(),
        3
    );

    let facts_cache = crate::node_facts::NodeFactsCache::new();
    facts_cache
        .insert(sign_node_facts(&facts, &sk).unwrap(), TrustTier::Paired, 1_000)
        .unwrap();
    let found = holders_from_facts(&facts_cache, &bin_hash, 1_000, "me");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].peer_id, node_id);
    assert_eq!(found[0].lan_id.as_deref(), Some("home"));
    assert!(holders_from_facts(&facts_cache, &[3; 32], 1_000, "me").is_empty());
    assert!(holders_from_facts(&facts_cache, &bin_hash, 1_000, &node_id).is_empty());
}

// ── revocation ───────────────────────────────────────────────────

fn revocations(tmp: &tempfile::TempDir) -> Arc<RevocationList> {
    Arc::new(RevocationList::new(tmp.path().join("revoked.json")))
}

/// B holds and seeds the package it fetched from `origin`.
async fn seeded_b(fx: &Fixture) -> (Node, ExchangedPackage, Arc<RevocationList>, tempfile::TempDir) {
    let origin = swarm_node("origin", cfg());
    let pkg = origin.ex.seed_package_dir(&fx.pkg, &fx.anchors).unwrap();
    let b = swarm_node("node-b", cfg());
    let tmp = tempfile::tempdir().unwrap();
    let list = revocations(&tmp);
    assert!(b.ex.set_revocations(list.clone()));
    let net = Arc::new(Net::new("node-b").with(&origin, Spec::default()));
    b.ex
        .swarm_fetch_package(net, &cands(&[origin]), &pkg.manifest_hash, &fx.anchors, &opts())
        .await
        .unwrap();
    (b, pkg, list, tmp)
}

async fn c_can_fetch_from(b: &Node, pkg: &ExchangedPackage, fx: &Fixture) -> bool {
    let c = swarm_node("node-c", cfg());
    let net = Arc::new(Net::new("node-c").with(b, Spec::default()));
    c.ex
        .swarm_fetch_package(net, &cands(std::slice::from_ref(b)), &pkg.manifest_hash, &fx.anchors, &opts())
        .await
        .is_ok()
}

#[tokio::test(start_paused = true)]
async fn revoking_the_package_evicts_it_and_stops_seeding_with_chained_events() {
    let fx = fixture(3 * 1024 * 1024);
    let (b, pkg, list, _tmp) = seeded_b(&fx).await;
    assert!(c_can_fetch_from(&b, &pkg, &fx).await, "B seeds before the revocation");
    let held = b.ex.store().count();
    assert!(held > 0);

    list.revoke_subject(RevocationKind::Package, &pkg.package_id, "compromised build").unwrap();
    // Seeding stops at once, before any sweep.
    assert!(!c_can_fetch_from(&b, &pkg, &fx).await);

    let revoked = b.ex.apply_revocations();
    assert_eq!(revoked.len(), 3, "manifest, cog.toml and binary: {revoked:?}");
    assert!(revoked.iter().all(|r| r.package_id == pkg.package_id));
    assert!(b.ex.servable_artifacts().is_empty());
    assert_eq!(b.ex.store().count(), 0, "bytes evicted");
    assert!(b.ex.resolve(&ArtifactKey::Content(binary_hash(&pkg))).is_none());

    let rev = events(&b.chain, EVENT_KIND_ARTIFACT_REVOKE);
    assert_eq!(rev.len(), 3);
    assert!(rev.iter().all(|e| e["subject_kind"] == "package" && e["reason"] == "compromised build"));
    let evicted = events(&b.chain, EVENT_KIND_ARTIFACT_EVICT);
    assert_eq!(evicted.len(), 3);
    assert!(evicted.iter().all(|e| e["reason"] == "revoked"));
    assert!(evicted.iter().map(|e| e["bytes_freed"].as_u64().unwrap()).sum::<u64>() >= 3 * 1024 * 1024);
    // Applying again changes nothing.
    assert!(b.ex.apply_revocations().is_empty());

    // The revoked package is refused if anyone offers it again.
    let origin = swarm_node("origin2", cfg());
    origin.ex.seed_package_dir(&fx.pkg, &fx.anchors).unwrap();
    let net = Arc::new(Net::new("node-b").with(&origin, Spec::default()));
    let err = b
        .ex
        .swarm_fetch_package(net, &cands(&[origin]), &pkg.manifest_hash, &fx.anchors, &opts())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("revoked"), "{err}");
}

#[tokio::test(start_paused = true)]
async fn revoking_a_signer_key_or_an_artifact_hash_evicts_what_it_allowed() {
    let fx = fixture(2 * 1024 * 1024);

    // Signer key.
    let (b, pkg, list, _tmp) = seeded_b(&fx).await;
    let signer = hex_encode(&key(1).verifying_key().to_bytes());
    list.revoke_subject(RevocationKind::SignerKey, &signer, "key leaked").unwrap();
    assert!(!c_can_fetch_from(&b, &pkg, &fx).await);
    let r = b.ex.apply_revocations();
    assert_eq!(r.len(), 3);
    assert!(r.iter().all(|x| x.subject.kind == RevocationKind::SignerKey));
    assert_eq!(b.ex.store().count(), 0);

    // One artifact hash: only that file goes; the rest of the package stays.
    let (b, pkg, list, _tmp) = seeded_b(&fx).await;
    let bin = hex_encode(&binary_hash(&pkg));
    list.revoke_subject(RevocationKind::ArtifactHash, &bin, "bad binary").unwrap();
    let r = b.ex.apply_revocations();
    assert_eq!(r.len(), 1);
    assert_eq!(hex_encode(&r[0].content_hash), bin);
    assert_eq!(b.ex.servable_artifacts().len(), 2, "manifest and cog.toml remain");
    assert_eq!(events(&b.chain, EVENT_KIND_ARTIFACT_REVOKE).len(), 1);
}
