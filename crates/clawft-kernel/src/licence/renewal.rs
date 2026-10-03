//! The steward's renewal pull (ADR-106 section 4, "Renewal"; phase 3).
//!
//! Every 12 h plus jitter the steward calls `POST /licence/v1/renew`, which
//! makes the Seed sign a new grant (`seq + 1`) for every active checkout, or a
//! withdrawal for one the licence no longer covers. It then pages
//! `GET /licence/v1/grants?since=<ctr>` to catch up on anything issued
//! meanwhile (a release, a withdrawal, a checkout through another path).
//!
//! Each grant is installed like a relayed one ([`super::install_grant`]: the
//! store verifies it under the bound key, then the bytes of the valid grants
//! become shareable) and flooded to the mesh. Withdrawals are applied and
//! flooded first, as soon as a response holds one. A grant the store already
//! holds is not flooded again.
//!
//! The Seed never connects into the mesh: only the steward pulls. On a node
//! the binding does not name the client refuses before sending
//! (`seed_not_bound`, `not_steward`, or `not_holder` from a daemon that does not
//! hold the licence role) and the pass is skipped. When the Seed
//! cannot be reached the next pass backs off (1 min doubling to 1 h); a
//! skipped pass polls again in 5 min, so a binding that arrives later is
//! renewed soon. The catch-up cursor may not move past the Seed's counter
//! from the renew answer plus the grants a page carries, and it restarts at
//! 0 when the binding's `seq` or the Seed's device id changes. Every
//! response is bounded by the transport caps; a response over them fails the
//! pass, which is retried with the backoff. W3 (releasing unused checkouts
//! automatically) is still open, so every active checkout is renewed.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::client::{LicenceClient, LicenceClientError};
use super::relay::{GrantFlood, install_grant};
use super::{CheckoutGrant, CheckoutGrantStore, Outcome, SignedGrant};
use crate::chain::ChainManager;
use crate::mesh_artifact::ArtifactExchange;

/// Chain event: a renewed grant was installed and flooded.
pub const EVENT_KIND_CHECKOUT_RENEWED: &str = "cog.checkout.renewed";
/// Chain event: the Seed withdrew a checkout (released, or no longer licensed).
pub const EVENT_KIND_CHECKOUT_LAPSED: &str = "cog.checkout.lapsed";

/// Timing of the pull.
#[derive(Debug, Clone)]
pub struct RenewalConfig {
    /// Between successful passes (12 h).
    pub period: Duration,
    /// Up to this much is added to each period, at random.
    pub jitter: Duration,
    /// Delay of the first pass after start.
    pub first_delay: Duration,
    /// First backoff after a failed pass; doubles each time.
    pub backoff_min: Duration,
    /// Longest backoff.
    pub backoff_max: Duration,
    /// Most catch-up pages followed in one pass.
    pub max_pages: u32,
    /// Delay of the next pass while passes are skipped (no binding naming
    /// this node yet): a binding that arrives later is renewed this soon.
    pub skip_poll: Duration,
}

impl Default for RenewalConfig {
    fn default() -> Self {
        Self {
            period: Duration::from_secs(12 * 3600),
            jitter: Duration::from_secs(30 * 60),
            first_delay: Duration::from_secs(5 * 60),
            backoff_min: Duration::from_secs(60),
            backoff_max: Duration::from_secs(3600),
            max_pages: 16,
            skip_poll: Duration::from_secs(5 * 60),
        }
    }
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenewalReport {
    /// The binding does not name this node (or there is none): nothing sent.
    pub skipped: bool,
    /// Grants installed and flooded (renewals and catch-up).
    pub renewed: u32,
    /// Withdrawals installed and flooded.
    pub withdrawn: u32,
    /// Grants already held.
    pub unchanged: u32,
    /// Grants the store refused (logged).
    pub refused: u32,
}

/// The steward's renewal pull.
pub struct Renewer {
    store: Arc<CheckoutGrantStore>,
    exchange: Arc<ArtifactExchange>,
    client: Arc<dyn LicenceClient>,
    flood: Arc<dyn GrantFlood>,
    chain: Option<Arc<ChainManager>>,
    cfg: RenewalConfig,
    cursor: Mutex<u64>,
    /// The (binding seq, seed device id) the cursor belongs to.
    cursor_for: Mutex<Option<(u64, String)>>,
}

fn skip(e: &LicenceClientError) -> bool {
    matches!(e, LicenceClientError::Refused { status: 0, code } if code == "not_steward" || code == "seed_not_bound" || code == "not_holder")
}

impl Renewer {
    /// A renewer over the steward's client (it signs for the binding in effect).
    pub fn new(
        store: Arc<CheckoutGrantStore>,
        exchange: Arc<ArtifactExchange>,
        client: Arc<dyn LicenceClient>,
        flood: Arc<dyn GrantFlood>,
        chain: Option<Arc<ChainManager>>,
        cfg: RenewalConfig,
    ) -> Arc<Self> {
        Arc::new(Self { store, exchange, client, flood, chain, cfg, cursor: Mutex::new(0), cursor_for: Mutex::new(None) })
    }

    /// The catch-up cursor (the Seed's issue counter).
    pub fn cursor(&self) -> u64 {
        *self.cursor.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn set_cursor(&self, v: u64) {
        *self.cursor.lock().unwrap_or_else(|p| p.into_inner()) = v;
    }

    /// The cursor counts one Seed's issues under one binding: a new binding
    /// `seq` or another device starts it again from 0.
    fn reset_cursor_on_rebind(&self) {
        let now = self.store.held_binding().map(|b| (b.seq, b.device_id));
        let mut f = self.cursor_for.lock().unwrap_or_else(|p| p.into_inner());
        if *f != now {
            *f = now;
            self.set_cursor(0);
        }
    }

    /// One pass: renew, then catch up. Errors only for a failed call.
    pub async fn run_once(&self) -> Result<RenewalReport, LicenceClientError> {
        let mut report = RenewalReport::default();
        self.reset_cursor_on_rebind();
        let renewed = match self.client.renew().await {
            Ok(g) => g,
            Err(e) if skip(&e) => return Ok(RenewalReport { skipped: true, ..report }),
            Err(e) => return Err(e),
        };
        // The Seed's counter after the renewal bounds the cursor: grants issued
        // since add one each, and they are in the pages that follow.
        let seed_ctr = renewed.next;
        self.apply_all(renewed.grants, &mut report).await;
        for _ in 0..self.cfg.max_pages {
            let since = self.cursor();
            let page = self.client.grants_page(since).await?;
            let bound = seed_ctr.max(since).saturating_add(page.grants.len() as u64);
            if page.next > bound {
                // A page may not skip the cursor past grants it does not carry.
                return Err(LicenceClientError::BadResponse(format!(
                    "grants page moves the cursor to {} past the bound {bound}",
                    page.next
                )));
            }
            self.apply_all(page.grants, &mut report).await;
            self.set_cursor(page.next.max(since));
            if !page.more || page.next <= since {
                break;
            }
        }
        Ok(report)
    }

    /// Operator release of one checkout: the Seed withdraws it (and renews
    /// the others, as every renewal does); everything it returns is
    /// installed and flooded, the withdrawal first. A pass this node may not
    /// make is `skipped`, nothing sent.
    pub async fn release(&self, cog_id: &str, version: &str) -> Result<RenewalReport, LicenceClientError> {
        let mut report = RenewalReport::default();
        let page = match self.client.release(cog_id, version).await {
            Ok(p) => p,
            Err(e) if skip(&e) => return Ok(RenewalReport { skipped: true, ..report }),
            Err(e) => return Err(e),
        };
        self.apply_all(page.grants, &mut report).await;
        Ok(report)
    }

    /// Withdrawals first, then the rest.
    async fn apply_all(&self, grants: Vec<SignedGrant>, report: &mut RenewalReport) {
        let parsed: Vec<(SignedGrant, Option<CheckoutGrant>)> =
            grants.into_iter().map(|s| { let g = serde_json::from_str(&s.payload).ok(); (s, g) }).collect();
        let (mut first, rest): (Vec<_>, Vec<_>) =
            parsed.into_iter().partition(|(_, g)| g.as_ref().is_some_and(CheckoutGrant::is_withdrawal));
        first.extend(rest);
        for (s, g) in first {
            self.apply(&s, g.as_ref(), report).await;
        }
    }

    async fn apply(&self, signed: &SignedGrant, g: Option<&CheckoutGrant>, report: &mut RenewalReport) {
        match install_grant(&self.store, &self.exchange, signed) {
            Ok(Outcome::Applied | Outcome::AppliedUnsaved) => {
                let withdrawn = g.is_some_and(CheckoutGrant::is_withdrawal);
                self.flood.flood(signed).await;
                if withdrawn {
                    report.withdrawn += 1;
                } else {
                    report.renewed += 1;
                }
                if let (Some(cm), Some(g)) = (&self.chain, g) {
                    let kind = if withdrawn { EVENT_KIND_CHECKOUT_LAPSED } else { EVENT_KIND_CHECKOUT_RENEWED };
                    cm.append("licence", kind, Some(serde_json::json!({
                        "cog_id": g.cog_id, "version": g.version, "seq": g.seq, "grant_id": g.grant_id,
                        "expires_at": g.expires_at, "reason": if withdrawn { "withdrawn by the Seed" } else { "renewed" },
                    })));
                }
            }
            Ok(_) => report.unchanged += 1,
            Err(e) => {
                report.refused += 1;
                tracing::warn!(error = %e, "renewed grant refused by the store");
            }
        }
    }

    /// The delay before the next pass after `failures` failed passes in a row.
    pub fn next_delay(&self, failures: u32) -> Duration {
        if failures == 0 {
            let j = self.cfg.jitter.as_millis() as u64;
            let extra = if j == 0 { 0 } else { rand::random::<u64>() % j };
            return self.cfg.period + Duration::from_millis(extra);
        }
        let exp = self.cfg.backoff_min.saturating_mul(1u32 << (failures - 1).min(16));
        exp.min(self.cfg.backoff_max)
    }

    /// Run passes forever on the current runtime. A skipped pass (no binding
    /// naming this node) polls again after `skip_poll`, not a full period.
    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            tokio::time::sleep(self.cfg.first_delay).await;
            let mut failures = 0u32;
            loop {
                let delay = match self.run_once().await {
                    Ok(r) if r.skipped => {
                        failures = 0;
                        self.cfg.skip_poll
                    }
                    Ok(r) => {
                        failures = 0;
                        tracing::info!(renewed = r.renewed, withdrawn = r.withdrawn, unchanged = r.unchanged,
                            refused = r.refused, "checkout renewal pass");
                        self.next_delay(0)
                    }
                    Err(e) => {
                        failures = failures.saturating_add(1);
                        tracing::warn!(error = %e, failures, "checkout renewal failed; backing off");
                        self.next_delay(failures)
                    }
                };
                tokio::time::sleep(delay).await;
            }
        })
    }
}
