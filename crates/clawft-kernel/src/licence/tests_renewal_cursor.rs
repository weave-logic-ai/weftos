//! The renewal pull's catch-up cursor and skip polling, against a scripted
//! client: a page may not move the cursor past the Seed's counter plus what it
//! carries, a rebind restarts the cursor, and a binding that arrives while
//! passes are skipped is renewed within the skip poll.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use super::tests_common::*;
use super::tests_exchange::wait_for;
use super::*;
use crate::artifact_store::ArtifactStore;
use crate::mesh_artifact::{ArtifactExchange, ExchangeConfig};

#[derive(Default)]
struct Scripted {
    renew_next: u64,
    page_next: Mutex<u64>,
    sinces: Mutex<Vec<u64>>,
    bound: AtomicBool,
    renews: AtomicU32,
}

#[async_trait]
impl LicenceClient for Scripted {
    async fn checkout(&self, _: &CheckoutWire) -> Result<SignedGrant, LicenceClientError> {
        Err(LicenceClientError::Transport("unused".into()))
    }
    async fn artifact(&self, _: &str, _: u64) -> Result<Vec<u8>, LicenceClientError> {
        Err(LicenceClientError::Transport("unused".into()))
    }
    async fn grants_since(&self, _: u64) -> Result<Vec<SignedGrant>, LicenceClientError> {
        Ok(Vec::new())
    }
    async fn grants_page(&self, since: u64) -> Result<GrantsPage, LicenceClientError> {
        self.sinces.lock().unwrap().push(since);
        Ok(GrantsPage { grants: Vec::new(), next: *self.page_next.lock().unwrap(), more: false })
    }
    async fn renew(&self) -> Result<GrantsPage, LicenceClientError> {
        if !self.bound.load(Ordering::SeqCst) {
            return Err(LicenceClientError::Refused { status: 0, code: "not_steward".into() });
        }
        self.renews.fetch_add(1, Ordering::SeqCst);
        Ok(GrantsPage { grants: Vec::new(), next: self.renew_next, more: false })
    }
}

fn renewer(fx: &Fx, client: Arc<Scripted>, cfg: RenewalConfig) -> Arc<Renewer> {
    let ex = Arc::new(ArtifactExchange::new("s", Arc::new(ArtifactStore::new_memory()), ExchangeConfig::default()).unwrap());
    Renewer::new(fx.store.clone(), ex, client, Arc::new(NoFlood), None, cfg)
}

#[tokio::test]
async fn a_page_cannot_jump_the_cursor_past_the_seeds_counter() {
    let fx = Fx::new();
    fx.bind();
    let c = Arc::new(Scripted { renew_next: 5, bound: AtomicBool::new(true), ..Default::default() });
    let r = renewer(&fx, c.clone(), RenewalConfig::default());
    *c.page_next.lock().unwrap() = 1_000;
    assert!(matches!(r.run_once().await, Err(LicenceClientError::BadResponse(_))));
    assert_eq!(r.cursor(), 0, "the cursor did not move");
    *c.page_next.lock().unwrap() = 5;
    r.run_once().await.unwrap();
    assert_eq!(r.cursor(), 5);
    // An empty page may not move it on beyond the counter either.
    *c.page_next.lock().unwrap() = 7;
    assert!(r.run_once().await.is_err());
    assert_eq!(r.cursor(), 5);
}

#[tokio::test]
async fn a_new_binding_seq_or_seed_restarts_the_cursor() {
    let fx = Fx::new();
    fx.bind();
    let c = Arc::new(Scripted { renew_next: 3, bound: AtomicBool::new(true), ..Default::default() });
    *c.page_next.lock().unwrap() = 3;
    let r = renewer(&fx, c.clone(), RenewalConfig::default());
    r.run_once().await.unwrap();
    r.run_once().await.unwrap();
    assert_eq!(*c.sinces.lock().unwrap(), vec![0, 3]);
    // A rebind (higher seq): back to 0.
    fx.store.accept_binding(&binding(2, BindState::Bound), posture(), &NoExtraChecks).unwrap();
    r.run_once().await.unwrap();
    assert_eq!(c.sinces.lock().unwrap().last(), Some(&0));
    // Another Seed device: back to 0 again.
    r.run_once().await.unwrap();
    assert_eq!(c.sinces.lock().unwrap().last(), Some(&3));
    let mut rec = binding_rec(3, BindState::Bound, &grant_key(), &mesh());
    rec.device_id = "seed-other".into();
    fx.store.accept_binding(&sign_binding(&rec, &op()).unwrap(), posture(), &NoExtraChecks).unwrap();
    r.run_once().await.unwrap();
    assert_eq!(c.sinces.lock().unwrap().last(), Some(&0));
}

#[tokio::test]
async fn a_binding_that_arrives_while_passes_are_skipped_is_renewed_within_the_skip_poll() {
    let fx = Fx::new();
    let c = Arc::new(Scripted::default());
    let cfg = RenewalConfig {
        first_delay: Duration::ZERO,
        skip_poll: Duration::from_millis(50),
        period: Duration::from_secs(3600),
        jitter: Duration::ZERO,
        ..RenewalConfig::default()
    };
    let task = renewer(&fx, c.clone(), cfg).spawn();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(c.renews.load(Ordering::SeqCst), 0, "skipped while not the steward");
    c.bound.store(true, Ordering::SeqCst);
    let start = std::time::Instant::now();
    wait_for("a renewal after the binding arrived", || c.renews.load(Ordering::SeqCst) >= 1).await;
    assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    task.abort();
}
