//! The steward's renewal pull against the real `weft-licence` on loopback
//! (ADR-106 phase 3): a renewal before expiry keeps a member's grant valid
//! past its first TTL, a withdrawal reaches a member at once (from the renew
//! answer and from the catch-up listing), and a pass that is not the
//! steward's or cannot reach the Seed is skipped or backs off.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use super::tests_common::*;
use super::tests_exchange::{TNode, link, wait_for};
use super::tests_http_e2e::{Lic, PermitAll, client_transport, node_clocked, seed_with, wire};
use super::*;
use crate::artifact_store::ArtifactStore;
use crate::chain::ChainManager;
use crate::mesh_artifact::{ArtifactExchange, ExchangeConfig};

struct Rig {
    now: Arc<AtomicU64>,
    lic: Lic,
    a: TNode,
    b: TNode,
    ax: Arc<ArtifactExchange>,
    client: Arc<dyn LicenceClient>,
    addr: std::net::SocketAddr,
    _seed: (tempfile::TempDir, weft_licence::http::Server),
}

fn wall() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

/// A steward A and a member B on one clock the test moves, the Seed on the same clock.
async fn rig(ttl: u64) -> Rig {
    let now = Arc::new(AtomicU64::new(wall()));
    let lic = Lic::default();
    let c = now.clone();
    let (dir, binding, server) = seed_with(Arc::new(AtomicU64::new(0)), Arc::new(move || c.load(Ordering::SeqCst)), ttl, lic.clone());
    let addr = server.addrs()[0];
    let (a, b) = (node_clocked("node-steward", clock_of(&now)), node_clocked("node-b", clock_of(&now)));
    link(&a, &b, true);
    a.ex.issue_binding(binding).await.unwrap();
    wait_for("B bound", || b.fx.store.active_binding().is_some()).await;
    let c = now.clone();
    let client: Arc<dyn LicenceClient> = StewardLicenceClient::new(
        a.fx.store.clone(),
        sk(21),
        "node-steward",
        client_transport(addr),
        Arc::new(move || c.load(Ordering::SeqCst) * 1000),
    );
    let ax = Arc::new(ArtifactExchange::new("node-steward", Arc::new(ArtifactStore::new_memory()), ExchangeConfig::default()).unwrap());
    let flood: Arc<dyn GrantFlood> = a.ex.clone();
    let relay = CheckoutRelay::new(a.fx.store.clone(), ax.clone(), client.clone(), Arc::new(PermitAll), flood, None);
    relay.handle(CheckoutCaller::Kernel, &wire()).await.expect("checkout");
    wait_for("B holds the grant", || b.fx.store.held_grant("fall-detect", "1.2.0").is_some()).await;
    Rig { now, lic, a, b, ax, client, addr, _seed: (dir, server) }
}

fn renewer(r: &Rig, client: Arc<dyn LicenceClient>, chain: Option<Arc<ChainManager>>) -> Arc<Renewer> {
    let flood: Arc<dyn GrantFlood> = r.a.ex.clone();
    Renewer::new(r.a.fx.store.clone(), r.ax.clone(), client, flood, chain, RenewalConfig::default())
}

fn seq_on(n: &TNode) -> u64 {
    n.fx.store.held_grant("fall-detect", "1.2.0").map_or(0, |(s, _)| s)
}

fn valid_on(n: &TNode) -> bool {
    n.fx.store.grant_rows().iter().any(|g| g.cog_id == "fall-detect" && g.valid)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_renewal_before_expiry_keeps_the_members_grant_valid_past_its_first_ttl() {
    let r = rig(3600).await;
    let first = r.b.fx.store.grant_rows()[0].expires_at;
    r.now.fetch_add(3000, Ordering::SeqCst);
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rep = renewer(&r, r.client.clone(), Some(chain.clone())).run_once().await.expect("pass");
    assert_eq!((rep.renewed, rep.withdrawn, rep.refused, rep.skipped), (1, 0, 0, false), "{rep:?}");
    wait_for("B holds the renewed grant", || seq_on(&r.b) == 2).await;
    // Past the first TTL: the first grant alone would have lapsed.
    r.now.fetch_add(3000, Ordering::SeqCst);
    assert!(r.now.load(Ordering::SeqCst) > first);
    assert!(valid_on(&r.b), "the renewed grant is still valid on the member");
    assert!(valid_on(&r.a));
    assert!(chain.tail(chain.len()).iter().any(|e| e.kind == EVENT_KIND_CHECKOUT_RENEWED));
    // Without a further renewal it lapses at its own TTL.
    r.now.fetch_add(3000, Ordering::SeqCst);
    assert!(!valid_on(&r.b));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_withdrawal_in_the_renew_answer_reaches_the_member_at_once() {
    let r = rig(72 * 3600).await;
    assert!(valid_on(&r.b));
    r.lic.0.store(true, Ordering::SeqCst); // the licence stops covering the cog
    let chain = Arc::new(ChainManager::new(0, 1000));
    let rep = renewer(&r, r.client.clone(), Some(chain.clone())).run_once().await.expect("pass");
    assert_eq!(rep.withdrawn, 1, "{rep:?}");
    wait_for("B has the withdrawal", || seq_on(&r.b) == 2).await;
    assert!(!valid_on(&r.b) && !valid_on(&r.a));
    assert!(r.b.fx.store.grant_rows()[0].withdrawn);
    assert!(chain.tail(chain.len()).iter().any(|e| e.kind == EVENT_KIND_CHECKOUT_LAPSED));
}

/// A client whose renew answer was lost: only the catch-up listing remains.
struct LostRenew(Arc<dyn LicenceClient>);

#[async_trait]
impl LicenceClient for LostRenew {
    async fn checkout(&self, req: &CheckoutWire) -> Result<SignedGrant, LicenceClientError> {
        self.0.checkout(req).await
    }
    async fn artifact(&self, b3: &str, max: u64) -> Result<Vec<u8>, LicenceClientError> {
        self.0.artifact(b3, max).await
    }
    async fn grants_since(&self, since: u64) -> Result<Vec<SignedGrant>, LicenceClientError> {
        self.0.grants_since(since).await
    }
    async fn grants_page(&self, since: u64) -> Result<GrantsPage, LicenceClientError> {
        self.0.grants_page(since).await
    }
    async fn renew(&self) -> Result<GrantsPage, LicenceClientError> {
        // The answer arrives, but without the withdrawal an earlier call carried.
        let mut p = self.0.renew().await?;
        p.grants.clear();
        Ok(p)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_withdrawal_missed_by_renew_is_caught_up_from_the_grants_listing() {
    let r = rig(72 * 3600).await;
    r.lic.0.store(true, Ordering::SeqCst);
    // The Seed signs (and persists) the withdrawal; this answer never arrives.
    let signed = r.client.renew().await.unwrap();
    assert_eq!(signed.grants.len(), 1);
    let rep = renewer(&r, Arc::new(LostRenew(r.client.clone())), None).run_once().await.expect("pass");
    assert_eq!((rep.withdrawn, rep.renewed), (1, 0), "{rep:?}");
    wait_for("B has the withdrawal", || seq_on(&r.b) == 2).await;
    assert!(!valid_on(&r.b));
    // A second pass finds nothing new past the cursor.
    let again = renewer(&r, Arc::new(LostRenew(r.client.clone())), None);
    let rep = again.run_once().await.unwrap();
    assert_eq!((rep.unchanged, rep.withdrawn + rep.renewed), (1, 0), "a fresh renewer re-reads the held grant from cursor 0, floods nothing");
    let rep = again.run_once().await.unwrap();
    assert_eq!(rep, RenewalReport::default(), "the cursor moved past it");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_steward_pass_is_skipped_and_an_unreachable_seed_backs_off() {
    let r = rig(72 * 3600).await;
    let c = r.now.clone();
    let other: Arc<dyn LicenceClient> = StewardLicenceClient::new(
        r.a.fx.store.clone(), sk(21), "node-x", client_transport(r.addr), Arc::new(move || c.load(Ordering::SeqCst) * 1000));
    assert!(renewer(&r, other, None).run_once().await.unwrap().skipped);

    // A port nobody listens on: the pass fails and the delay backs off.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let c = r.now.clone();
    let gone: Arc<dyn LicenceClient> = StewardLicenceClient::new(
        r.a.fx.store.clone(), sk(21), "node-steward", client_transport(dead), Arc::new(move || c.load(Ordering::SeqCst) * 1000));
    let rw = renewer(&r, gone, None);
    assert!(matches!(rw.run_once().await, Err(LicenceClientError::Transport(_))));
    assert_eq!(rw.next_delay(1), Duration::from_secs(60));
    assert_eq!(rw.next_delay(3), Duration::from_secs(240));
    assert_eq!(rw.next_delay(30), Duration::from_secs(3600));
    let d = rw.next_delay(0);
    assert!(d >= Duration::from_secs(12 * 3600) && d < Duration::from_secs(12 * 3600 + 30 * 60), "{d:?}");
}
