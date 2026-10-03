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
    /// The dialing peer is verified by admission (default: only claimed).
    verified: bool,
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
type StreamFactory = Arc<dyn Fn() -> Box<dyn MeshStream> + Send + Sync>;

struct Net {
    /// Id the dialing node presents to the servers.
    client: String,
    peers: HashMap<String, (Arc<ArtifactExchange>, Spec)>,
    /// Scripted (malicious) peers: a fresh stream per dial.
    custom: HashMap<String, StreamFactory>,
    dials: Mutex<Vec<String>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Net {
    fn new(client: &str) -> Self {
        Self {
            client: client.into(),
            peers: HashMap::new(),
            custom: HashMap::new(),
            dials: Mutex::new(Vec::new()),
            tasks: Mutex::new(Vec::new()),
        }
    }
    fn with(mut self, n: &Node, spec: Spec) -> Self {
        self.peers
            .insert(n.ex.node_id().to_string(), (n.ex.clone(), spec));
        self
    }
    fn with_stream(mut self, peer: &str, f: StreamFactory) -> Self {
        self.custom.insert(peer.to_string(), f);
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
        if let Some(f) = self.custom.get(peer) {
            return Ok(f());
        }
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
            let who = if s.spec.verified {
                crate::mesh_swarm_state::ServePeer::verified(me)
            } else {
                crate::mesh_swarm_state::ServePeer::unverified(me)
            };
            let _ = ex.serve_as(&mut s, &who).await;
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
    let o = SwarmFetchOptions { local_lan: Some("home".into()), ..Default::default() };
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

// ── hardening: licence, size limits, liars, eviction ────────────────

use crate::mesh_artifact_types::{ArtifactDescriptor, MAX_PIECE_SIZE};
use crate::mesh_artifact_wire::ArtifactMsg;
use crate::workload_pkg::{CogPackInput, PackageSource, key_id_for, pack_cog, sign_envelope, write_manifest};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A peer that answers `meta_request` with `descriptor` and, if `data` is
/// given, serves its pieces from it. Counts `request` frames.
struct Script {
    descriptor: ArtifactDescriptor,
    data: Option<Vec<u8>>,
    requests: Arc<AtomicUsize>,
    queue: std::collections::VecDeque<Vec<u8>>,
}

#[async_trait]
impl MeshStream for Script {
    async fn send(&mut self, raw: &[u8]) -> Result<(), MeshError> {
        match ArtifactMsg::from_wire(raw).map_err(|e| MeshError::Transport(e.to_string()))? {
            ArtifactMsg::MetaRequest { .. } => {
                let d = self.descriptor.clone();
                let have = {
                    let mut b = crate::mesh_artifact_types::Bitfield::new(d.piece_count());
                    for i in 0..d.piece_count() {
                        b.set(i, true);
                    }
                    b
                };
                let id = d.id();
                self.queue.push_back(ArtifactMsg::Meta { descriptor: d }.to_wire().unwrap());
                self.queue.push_back(ArtifactMsg::Announce { id, have }.to_wire().unwrap());
            }
            ArtifactMsg::Request { id, pieces } => {
                self.requests.fetch_add(1, Ordering::SeqCst);
                if let Some(data) = &self.data {
                    for i in pieces {
                        let start = i as usize * self.descriptor.piece_size as usize;
                        let end = (start + self.descriptor.piece_size as usize).min(data.len());
                        let msg = ArtifactMsg::Piece { id, index: i, offset: 0, data: data[start..end].to_vec() };
                        self.queue.push_back(msg.to_wire().unwrap());
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        self.queue.pop_front().ok_or(MeshError::ConnectionClosed)
    }
    async fn close(&mut self) -> Result<(), MeshError> {
        Ok(())
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        None
    }
}

fn scripted(d: ArtifactDescriptor, data: Option<Vec<u8>>, requests: Arc<AtomicUsize>) -> StreamFactory {
    Arc::new(move || {
        Box::new(Script {
            descriptor: d.clone(),
            data: data.clone(),
            requests: requests.clone(),
            queue: Default::default(),
        })
    })
}

/// A signed package; `commit` varies the package id, `record` adds a
/// Cognitum release-record attestation. The binary is the same bytes every time.
fn pack(
    root: &std::path::Path,
    len: usize,
    k: &ed25519_dalek::SigningKey,
    commit: &str,
    record: bool,
    redistributable: bool,
) -> PathBuf {
    let cog_dir = root.join("cog");
    std::fs::create_dir_all(&cog_dir).unwrap();
    std::fs::write(cog_dir.join("cog.toml"), "[cog]\nid = \"swarm-probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\n").unwrap();
    let bin: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
    std::fs::write(root.join("bin"), &bin).unwrap();
    let rec = root.join("record.json");
    std::fs::write(&rec, b"{\"kind\":\"cognitum.cog.release-record.v1\"}").unwrap();
    let input = CogPackInput {
        cog_dir,
        binaries: vec![("aarch64".into(), root.join("bin"))],
        source: PackageSource { repo: None, commit: Some(commit.into()), release_url: None },
        cognitum_record: record.then_some(rec),
        redistributable,
        provenance: None,
        allow_no_provenance: true,
    };
    let pkg = root.join("pkg");
    let mut env = pack_cog(&input, &pkg).unwrap();
    sign_envelope(&mut env, k, &key_id_for(&k.verifying_key().to_bytes())).unwrap();
    write_manifest(&pkg, &env).unwrap();
    pkg
}

#[tokio::test(start_paused = true)]
async fn a_cognitum_origin_artifact_is_neither_advertised_nor_served() {
    use crate::mesh_swarm_cache::{ArtifactCache, CacheConfig};
    let tmp = tempfile::tempdir().unwrap();
    let k = key(1);
    let dir = pack(tmp.path(), 2 * 1024 * 1024, &k, "aaaaaaa", true, true);
    let anchors = anchors_for(&k);
    let holder = swarm_node("holder", cfg());
    let pkg = holder.ex.seed_package_dir(&dir, &anchors).unwrap();

    // The holder has it, verified and signed, and can read it ...
    let bin = binary_hash(&pkg);
    let id = holder.ex.resolve(&ArtifactKey::Content(bin)).unwrap().id();
    assert!(holder.ex.read_all(&id).is_ok());
    // ... but does not advertise it ...
    assert!(holder.ex.servable_artifacts().is_empty());
    let cache = ArtifactCache::new(holder.ex.clone(), CacheConfig { max_bytes: 1 << 30 });
    let mut facts = clawft_types::placement::NodeFacts::new("holder", 1_000, 600, 1);
    cache.advertise(&mut facts);
    assert!(facts.capabilities.iter().all(|c| !c.id.is_under("store.artifact")), "{:?}", facts.capabilities);
    // ... never chains a seed event ...
    assert!(events(&holder.chain, EVENT_KIND_ARTIFACT_SEED).is_empty());
    // ... and refuses every peer.
    let leech = swarm_node("leech", cfg());
    let net = Arc::new(Net::new("leech").with(&holder, Spec::default()));
    let err = leech
        .ex
        .swarm_fetch_package(net.clone(), &cands(std::slice::from_ref(&holder)), &pkg.manifest_hash, &anchors, &opts())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not servable") || err.to_string().contains("fetch incomplete"), "{err}");
    assert!(events(&holder.chain, crate::chain::EVENT_KIND_ARTIFACT_SERVE).is_empty());
    assert!(holder.ex.who_has(net, &cands(std::slice::from_ref(&holder)), ArtifactKey::Content(bin)).await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn content_listed_by_two_packages_survives_the_revocation_of_one() {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(1);
    let anchors = anchors_for(&k);
    let (a_dir, b_dir) = (tmp.path().join("a"), tmp.path().join("b"));
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::create_dir_all(&b_dir).unwrap();
    let a = pack(&a_dir, 2 * 1024 * 1024, &k, "aaaaaaa", false, true);
    let b = pack(&b_dir, 2 * 1024 * 1024, &k, "bbbbbbb", false, true);
    let holder = swarm_node("holder", cfg());
    let pa = holder.ex.seed_package_dir(&a, &anchors).unwrap();
    let pb = holder.ex.seed_package_dir(&b, &anchors).unwrap();
    assert_ne!(pa.package_id, pb.package_id);
    assert_eq!(binary_hash(&pa), binary_hash(&pb), "same binary in both");

    let rtmp = tempfile::tempdir().unwrap();
    let list = revocations(&rtmp);
    holder.ex.set_revocations(list.clone());
    list.revoke_subject(RevocationKind::Package, &pa.package_id, "bad").unwrap();
    holder.ex.apply_revocations();
    let bin = holder.ex.resolve(&ArtifactKey::Content(binary_hash(&pa))).expect("package B still lists it");
    assert!(holder.ex.is_servable(&bin));
    list.revoke_subject(RevocationKind::Package, &pb.package_id, "bad too").unwrap();
    holder.ex.apply_revocations();
    assert!(holder.ex.resolve(&ArtifactKey::Content(binary_hash(&pa))).is_none());
}

#[tokio::test(start_paused = true)]
async fn an_oversize_descriptor_is_refused_before_any_piece_is_requested() {
    let leech = swarm_node("leech", ExchangeConfig { max_artifact_bytes: 1 << 30, ..cfg() });
    let requests = Arc::new(AtomicUsize::new(0));
    let key = ArtifactKey::Content([5; 32]);

    // A piece size far above the cap (valid on the wire: two 1 GiB pieces).
    let huge_pieces = ArtifactDescriptor {
        piece_size: MAX_PIECE_SIZE,
        total_size: 2 * MAX_PIECE_SIZE,
        content_hash: [5; 32],
        pieces: vec![[1; 32]; 2],
    };
    // A total far above the cap (10 GiB in 1 MiB pieces).
    let huge_total = ArtifactDescriptor {
        piece_size: MIB,
        total_size: 10 * 1024 * MIB,
        content_hash: [5; 32],
        pieces: vec![[2; 32]; 10 * 1024],
    };
    for (name, d) in [("piece-size", huge_pieces), ("total", huge_total)] {
        let net = Arc::new(Net::new("leech").with_stream("evil", scripted(d, None, requests.clone())));
        let err = leech
            .ex
            .swarm_fetch(net, vec![PeerCandidate::new("evil")], key, &opts())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("descriptor refused"), "{name}: {err}");
    }
    assert_eq!(requests.load(Ordering::SeqCst), 0, "no piece was requested");
    assert_eq!(leech.ex.pending_bytes(), 0, "nothing was left pending");
    assert_eq!(leech.ex.store().count(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_descriptor_that_does_not_fit_the_manifest_size_is_refused_before_any_piece() {
    let fx = fixture(2 * 1024 * 1024);
    let (seeds, pkg) = seeders(1, &fx);
    let net = Arc::new(Net::new("leech").with(&seeds[0], Spec::default()));
    let leech = swarm_node("leech", cfg());
    let wrong = SwarmFetchOptions {
        expect: crate::mesh_swarm_fetch::Expect { size: Some(3 * MIB), ..Default::default() },
        ..Default::default()
    };
    let err = leech
        .ex
        .swarm_fetch(net.clone(), cands(&seeds), ArtifactKey::Content(binary_hash(&pkg)), &wrong)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not the expected"), "{err}");
    assert!(events(&seeds[0].chain, crate::chain::EVENT_KIND_ARTIFACT_SERVE).is_empty(), "no piece served");
    // The right size passes.
    let right = SwarmFetchOptions {
        expect: crate::mesh_swarm_fetch::Expect { size: Some(2 * MIB), ..Default::default() },
        ..Default::default()
    };
    leech.ex.swarm_fetch(net, cands(&seeds), ArtifactKey::Content(binary_hash(&pkg)), &right).await.unwrap();
}

#[test]
fn partial_downloads_are_capped() {
    let ex = swarm_node("n", cfg()).ex;
    for i in 0..crate::mesh_artifact::MAX_PENDING as u8 {
        let d = ArtifactDescriptor { piece_size: 1024, total_size: 1, content_hash: [i; 32], pieces: vec![[i; 32]] };
        ex.note_pending(&d).unwrap();
    }
    let extra = ArtifactDescriptor { piece_size: 1024, total_size: 1, content_hash: [200; 32], pieces: vec![[200; 32]] };
    assert!(ex.note_pending(&extra).is_err());
}

#[tokio::test(start_paused = true)]
async fn a_peer_that_lies_about_a_content_hash_is_banned_and_the_fetch_finishes_elsewhere() {
    let fx = fixture(BIN);
    let (seeds, pkg) = seeders(2, &fx);
    let hash = binary_hash(&pkg);
    // The liar's pieces are self-consistent garbage under the real content hash.
    let garbage = vec![0xABu8; BIN];
    let liar = ArtifactDescriptor {
        piece_size: MIB,
        total_size: BIN as u64,
        content_hash: hash,
        pieces: garbage.chunks(MIB as usize).map(|c| *blake3::hash(c).as_bytes()).collect(),
    };
    let net = Arc::new(
        Net::new("leech")
            .with_stream("a-liar", scripted(liar, Some(garbage), Arc::new(AtomicUsize::new(0))))
            .with(&seeds[0], Spec::default())
            .with(&seeds[1], Spec::default()),
    );
    let leech = swarm_node("leech", cfg());
    let mut c = cands(&seeds);
    c.insert(0, PeerCandidate::new("a-liar")); // ranked first
    let out = leech
        .ex
        .swarm_fetch(net, c, ArtifactKey::Content(hash), &opts())
        .await
        .expect("honest peers finish what the liar blocked");
    assert!(leech.ex.is_banned("a-liar"));
    assert!(!out.sources.contains(&"a-liar".to_string()));
    assert!(!seeds.iter().any(|s| leech.ex.is_banned(s.ex.node_id())), "honest peers are not blamed");
    assert_eq!(leech.ex.read_all(&out.id).unwrap().len(), BIN);
    assert_eq!(leech.ex.pending_bytes(), 0);
    assert_eq!(events(&leech.chain, EVENT_KIND_ARTIFACT_PEER_BAN).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_package_fetch_pins_its_files_so_a_small_cache_cannot_evict_them() {
    use crate::mesh_swarm_cache::{ArtifactCache, CacheConfig};
    let fx = fixture(3 * 1024 * 1024);
    let (seeds, pkg) = seeders(1, &fx);
    let net = Arc::new(Net::new("leech").with(&seeds[0], Spec::default()));
    let leech = swarm_node("leech", cfg());
    // The binary alone is bigger than the whole budget.
    let cache = ArtifactCache::new(leech.ex.clone(), CacheConfig { max_bytes: 2 * MIB });
    let got = leech
        .ex
        .swarm_fetch_package(net.clone(), &cands(&seeds), &pkg.manifest_hash, &fx.anchors, &opts())
        .await
        .expect("pinned files are kept even over budget");
    for (_, id) in &got.files {
        assert!(leech.ex.is_verified(id), "file evicted mid-fetch");
    }
    assert_eq!(leech.ex.servable_artifacts().len(), 3);
    assert!(cache.over_budget() > 0, "the pins hold the cache over budget");
    // Removing the package releases the pins and the budget applies again.
    cache.unpin_package(&got);
    assert!(leech.ex.resolve(&ArtifactKey::Content(binary_hash(&pkg))).is_none());

    // Control: the same fetch without a package (nothing pinned) does not survive.
    let leech2 = swarm_node("leech2", cfg());
    let _cache2 = ArtifactCache::new(leech2.ex.clone(), CacheConfig { max_bytes: 2 * MIB });
    let net2 = Arc::new(Net::new("leech2").with(&seeds[0], Spec::default()));
    leech2.ex.swarm_fetch(net2, cands(&seeds), ArtifactKey::Content(binary_hash(&pkg)), &opts()).await.unwrap();
    assert!(leech2.ex.resolve(&ArtifactKey::Content(binary_hash(&pkg))).is_none(), "unpinned, over budget: evicted");
}

#[tokio::test(start_paused = true)]
async fn bytes_being_downloaded_count_against_the_budget() {
    use crate::mesh_swarm_cache::{ArtifactCache, CacheConfig};
    let fx = fixture(3 * 1024 * 1024);
    let (seeds, pkg) = seeders(1, &fx);
    let net = Arc::new(Net::new("leech").with(&seeds[0], Spec::default()));
    let leech = swarm_node("leech", cfg());
    let cache = ArtifactCache::new(leech.ex.clone(), CacheConfig { max_bytes: 5 * MIB });
    let old = leech.ex.seed_bytes(&vec![9u8; 4 * MIB as usize]).unwrap().id();
    assert!(leech.ex.is_verified(&old));
    // 4 MiB held + 3 MiB arriving > 5 MiB: the old, unpinned entry makes room.
    leech
        .ex
        .swarm_fetch(net, cands(&seeds), ArtifactKey::Content(binary_hash(&pkg)), &opts())
        .await
        .unwrap();
    assert!(!leech.ex.is_verified(&old), "LRU entry evicted to make room for the download");
    assert!(leech.ex.resolve(&ArtifactKey::Content(binary_hash(&pkg))).is_some());
    assert!(cache.used_bytes() <= 5 * MIB);
}

#[test]
fn eviction_never_removes_a_blob_another_subsystem_stored() {
    let ex = swarm_node("n", cfg()).ex;

    // 1. A workload stored these bytes first; the exchange only found them there.
    let data = vec![3u8; 1000];
    ex.store().store(&data, crate::artifact_store::ArtifactType::Generic).unwrap();
    let d = ex.seed_bytes(&data).unwrap();
    ex.forget(&d.id(), "lru").unwrap();
    assert!(ex.store().contains(&hex_encode(&d.content_hash)), "not ours: left alone");

    // 2. The exchange created the blob, then a workload stored the same bytes.
    let data2 = vec![4u8; 1000];
    let d2 = ex.seed_bytes(&data2).unwrap();
    ex.store().store(&data2, crate::artifact_store::ArtifactType::Generic).unwrap();
    ex.forget(&d2.id(), "lru").unwrap();
    assert!(ex.store().contains(&hex_encode(&d2.content_hash)), "shared since: left alone");

    // 3. Only ours, nobody else: removed.
    let d3 = ex.seed_bytes(&vec![5u8; 1000]).unwrap();
    ex.forget(&d3.id(), "lru").unwrap();
    assert!(!ex.store().contains(&hex_encode(&d3.content_hash)));
}

#[test]
fn a_pinned_entry_is_not_forgotten_except_by_revocation() {
    use crate::mesh_swarm_cache::{ArtifactCache, CacheConfig};
    let ex = swarm_node("n", cfg()).ex;
    let cache = ArtifactCache::new(ex.clone(), CacheConfig { max_bytes: 1 << 30 });
    let d = ex.seed_bytes(&vec![6u8; 1000]).unwrap();
    assert!(cache.pin(&d.id()));
    assert!(ex.forget(&d.id(), "lru").is_none());
    assert!(ex.is_verified(&d.id()));
    assert!(ex.forget(&d.id(), "revoked").is_some(), "revocation overrides a pin");
}

#[tokio::test(start_paused = true)]
async fn a_repack_without_the_attestation_or_the_flag_is_not_served() {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(1);
    let anchors = anchors_for(&k);
    // An operator re-pack of a licence-gated binary: no attestation, and the
    // signer never opted in to redistribution.
    let dir = pack(tmp.path(), 2 * 1024 * 1024, &k, "ccccccc", false, false);
    let holder = swarm_node("holder", cfg());
    let pkg = holder.ex.seed_package_dir(&dir, &anchors).unwrap();
    assert!(holder.ex.servable_artifacts().is_empty());
    assert!(events(&holder.chain, EVENT_KIND_ARTIFACT_SEED).is_empty());
    let leech = swarm_node("leech", cfg());
    let net = Arc::new(Net::new("leech").with(&holder, Spec::default()));
    assert!(leech
        .ex
        .swarm_fetch_package(net, &cands(std::slice::from_ref(&holder)), &pkg.manifest_hash, &anchors, &opts())
        .await
        .is_err());
    assert!(events(&holder.chain, crate::chain::EVENT_KIND_ARTIFACT_SERVE).is_empty());
}

#[tokio::test(start_paused = true)]
async fn one_non_redistributable_grant_vetoes_a_hash_for_every_package() {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(1);
    let anchors = anchors_for(&k);
    let (open_dir, gated_dir) = (tmp.path().join("open"), tmp.path().join("gated"));
    std::fs::create_dir_all(&open_dir).unwrap();
    std::fs::create_dir_all(&gated_dir).unwrap();
    let open = pack(&open_dir, 2 * 1024 * 1024, &k, "aaaaaaa", false, true);
    let gated = pack(&gated_dir, 2 * 1024 * 1024, &k, "bbbbbbb", false, false);
    for order in [[&open, &gated], [&gated, &open]] {
        let holder = swarm_node("holder", cfg());
        let mut last = None;
        for d in order {
            last = Some(holder.ex.seed_package_dir(d, &anchors).unwrap());
        }
        let bin = holder.ex.resolve(&ArtifactKey::Content(binary_hash(&last.unwrap()))).unwrap();
        assert!(!holder.ex.is_servable(&bin), "shared hash, one non-redistributable grant");
        assert!(holder.ex.servable_artifacts().iter().all(|d| d.id() != bin.id()));
    }
}

#[test]
fn a_blob_left_on_disk_by_an_earlier_run_is_never_evicted_after_a_restart() {
    // An installed workload's file, written by a previous process.
    let dir = tempfile::tempdir().unwrap();
    let data = vec![8u8; 2000];
    ArtifactStore::new_file(dir.path().to_path_buf())
        .store(&data, crate::artifact_store::ArtifactType::Generic)
        .unwrap();
    // The daemon reopens the store with its index (`open_file`); the exchange
    // finds the blob present, so it does not own it.
    let ex = node_with("n", ArtifactStore::open_file(dir.path().to_path_buf()).unwrap(), cfg()).ex;
    let d = ex.seed_bytes(&data).unwrap();
    ex.forget(&d.id(), "lru").unwrap();
    assert!(ex.store().contains(&hex_encode(&d.content_hash)), "the workload's file survives");
    // Control: with `new_file` (no index) the exchange would have re-stored the
    // bytes as its own and evicted them, which is why the daemon must not use it.
    let ex2 = node_with("n2", ArtifactStore::new_file(dir.path().to_path_buf()), cfg()).ex;
    let d2 = ex2.seed_bytes(&data).unwrap();
    ex2.forget(&d2.id(), "lru").unwrap();
    assert!(!ex2.store().contains(&hex_encode(&d2.content_hash)));
}

/// Allows serving verified peers only; never advertising; seeding yes.
#[derive(Debug)]
struct VerifiedServeOnly;
impl crate::mesh_swarm_state::RedistributionPolicy for VerifiedServeOnly {
    fn allows(
        &self,
        _: &[u8; 32],
        g: &[crate::mesh_swarm_state::GrantInfo],
        a: &crate::mesh_swarm_state::Audience<'_>,
    ) -> bool {
        use crate::mesh_swarm_state::Audience::*;
        !g.is_empty()
            && match a {
                Serve(p) => p.verified,
                Advertise => false,
                Seed => true,
            }
    }
}

#[derive(Debug)]
struct AllowEverything;
impl crate::mesh_swarm_state::RedistributionPolicy for AllowEverything {
    fn allows(&self, _: &[u8; 32], g: &[crate::mesh_swarm_state::GrantInfo], _: &crate::mesh_swarm_state::Audience<'_>) -> bool {
        !g.is_empty()
    }
}

fn gated_package(tmp: &std::path::Path) -> (PathBuf, TrustAnchors) {
    let k = key(1);
    (pack(tmp, 2 * 1024 * 1024, &k, "ddddddd", false, false), anchors_for(&k))
}

#[test]
fn the_default_policy_decides_each_audience_the_same_way() {
    use crate::mesh_swarm_state::{Audience, GrantInfo, GrantOrigin, ManifestPolicy, RedistributionPolicy, ServePeer};
    let g = |o: GrantOrigin| GrantInfo { package_id: "p".into(), signers: vec![], origin: o };
    let cog = GrantOrigin::Cognitum { cog_id: "c".into(), version: "1".into() };
    let peer = ServePeer::verified("n");
    let unverified = ServePeer::unverified("n");
    let auds = [Audience::Serve(&peer), Audience::Serve(&unverified), Audience::Advertise, Audience::Seed];
    for a in &auds {
        assert!(ManifestPolicy.allows(&[0; 32], &[g(GrantOrigin::OptIn)], a), "{a:?}");
        assert!(ManifestPolicy.allows(&[0; 32], &[g(GrantOrigin::OptIn), g(GrantOrigin::OptIn)], a));
        // Any non-OptIn grant vetoes the hash, whatever else lists it.
        assert!(!ManifestPolicy.allows(&[0; 32], &[g(GrantOrigin::OptIn), g(GrantOrigin::NotFlagged)], a));
        assert!(!ManifestPolicy.allows(&[0; 32], &[g(GrantOrigin::OptIn), g(cog.clone())], a));
        assert!(!ManifestPolicy.allows(&[0; 32], &[g(cog.clone())], a));
        assert!(!ManifestPolicy.allows(&[0; 32], &[g(GrantOrigin::NotFlagged)], a));
        assert!(!ManifestPolicy.allows(&[0; 32], &[], a));
    }
}

#[tokio::test(start_paused = true)]
async fn the_cognitum_origin_is_recorded_on_the_grant() {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(1);
    let dir = pack(tmp.path(), 1024 * 1024, &k, "aaaaaaa", true, true);
    let holder = swarm_node("holder", cfg());
    let pkg = holder.ex.seed_package_dir(&dir, &anchors_for(&k)).unwrap();
    let g = holder.ex.grants.get(&binary_hash(&pkg)).unwrap().clone();
    assert_eq!(
        g[0].origin,
        crate::mesh_swarm_state::GrantOrigin::Cognitum { cog_id: "swarm-probe".into(), version: "0.1.0".into() },
        "the flag does not override Cognitum provenance"
    );
}

#[tokio::test(start_paused = true)]
async fn a_provenance_json_stamp_makes_the_grant_cognitum_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(1);
    std::fs::create_dir_all(tmp.path().join("cog")).unwrap();
    std::fs::write(tmp.path().join("cog/provenance.json"), br#"{"trust":"cognitum-sha256","sha256":"ab"}"#).unwrap();
    let dir = pack(tmp.path(), 1024 * 1024, &k, "aaaaaaa", false, false);
    let holder = swarm_node("holder", cfg());
    let pkg = holder.ex.seed_package_dir(&dir, &anchors_for(&k)).unwrap();
    let g = holder.ex.grants.get(&binary_hash(&pkg)).unwrap().clone();
    assert_eq!(
        g[0].origin,
        crate::mesh_swarm_state::GrantOrigin::Cognitum { cog_id: "swarm-probe".into(), version: "0.1.0".into() }
    );
}

#[tokio::test(start_paused = true)]
async fn a_cognitum_release_url_alone_still_marks_cognitum_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(1);
    let dir = pack(tmp.path(), 1024 * 1024, &k, "aaaaaaa", false, true);
    let mut env = crate::workload_pkg::ManifestEnvelope::from_bytes(&std::fs::read(dir.join("cogpkg.json")).unwrap()).unwrap();
    env.body["source"]["release_url"] = "https://example.invalid/cognitum/cogs".into();
    env.signatures.clear();
    sign_envelope(&mut env, &k, &key_id_for(&k.verifying_key().to_bytes())).unwrap();
    write_manifest(&dir, &env).unwrap();
    let holder = swarm_node("holder", cfg());
    let pkg = holder.ex.seed_package_dir(&dir, &anchors_for(&k)).unwrap();
    let g = holder.ex.grants.get(&binary_hash(&pkg)).unwrap().clone();
    assert_eq!(
        g[0].origin,
        crate::mesh_swarm_state::GrantOrigin::Cognitum { cog_id: "swarm-probe".into(), version: "0.1.0".into() },
        "a cognitum release URL is a fail-closed second signal, even with redistributable set"
    );
}

#[tokio::test(start_paused = true)]
async fn a_policy_can_serve_verified_peers_only_and_it_sees_who_asks() {
    let tmp = tempfile::tempdir().unwrap();
    let (dir, anchors) = gated_package(tmp.path());
    let holder = swarm_node("holder", ExchangeConfig { redistribution: Arc::new(VerifiedServeOnly), ..cfg() });
    let pkg = holder.ex.seed_package_dir(&dir, &anchors).unwrap();
    let bin = ArtifactKey::Content(binary_hash(&pkg));

    // Seed: allowed, so the seed event is chained.
    assert_eq!(events(&holder.chain, EVENT_KIND_ARTIFACT_SEED).len(), 3);
    // Advertise: never, so nothing is in the broadcast facts or the servable list.
    assert!(holder.ex.servable_artifacts().is_empty());
    assert!(holder.ex.held_capabilities(&Default::default()).is_empty());

    // Serve: only to a verified peer. who_has (the discovery path) sees the same.
    let verified = Arc::new(Net::new("v").with(&holder, Spec { verified: true, ..Default::default() }));
    let claimed = Arc::new(Net::new("c").with(&holder, Spec::default()));
    let peers = [PeerCandidate::new("holder")];
    let asker = swarm_node("asker", cfg());
    assert!(asker.ex.who_has(claimed.clone(), &peers, bin).await.is_empty(), "unverified peer sees nothing");
    assert_eq!(asker.ex.who_has(verified.clone(), &peers, bin).await.len(), 1);
    let leech = swarm_node("leech", cfg());
    assert!(leech.ex.swarm_fetch(claimed, peers.to_vec(), bin, &opts()).await.is_err());
    leech.ex.swarm_fetch(verified, peers.to_vec(), bin, &opts()).await.expect("a verified peer is served");
}

#[tokio::test(start_paused = true)]
async fn content_that_is_not_opt_in_never_reaches_broadcast_facts_whatever_the_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let (dir, anchors) = gated_package(tmp.path());
    let holder = swarm_node("holder", ExchangeConfig { redistribution: Arc::new(AllowEverything), ..cfg() });
    holder.ex.seed_package_dir(&dir, &anchors).unwrap();
    // The policy lets this node seed and serve it ...
    assert_eq!(holder.ex.servable_artifacts().len(), 3);
    // ... but it is never listed for every peer to see.
    assert!(holder.ex.held_capabilities(&Default::default()).is_empty());
    // An opt-in package, same policy, is listed.
    let tmp2 = tempfile::tempdir().unwrap();
    let k = key(1);
    let open = pack(tmp2.path(), 1024 * 1024, &k, "eeeeeee", false, true);
    let other = swarm_node("other", ExchangeConfig { redistribution: Arc::new(AllowEverything), ..cfg() });
    other.ex.seed_package_dir(&open, &anchors).unwrap();
    assert!(!other.ex.held_capabilities(&Default::default()).is_empty());
}
