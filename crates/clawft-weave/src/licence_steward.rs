//! The steward relay in the daemon (ADR-106 phase 3).
//!
//! A node becomes the steward when the operator-signed binding names it
//! (`steward_node_id` and `steward_pubkey`). To relay, it also needs the link
//! to `weft-licence`, read from `licence-link.json` in the runtime dir (owner
//! and mode checked like every placement policy file):
//!
//! ```json
//! {"url": "http://100.64.0.10:8700", "allow_unpinned_lab_link": true}
//! ```
//!
//! - `url`: `http(s)://<ip>[:port]`. Address the Seed by IP (it checks
//!   `Host` against its listen addresses).
//! - `tls_spki_sha256` (`spki-sha256:<64 hex>`) or `tls_sha256`: pin an
//!   `https://` link.
//! - `allow_unpinned_lab_link`: the explicit opt-in for a plain-http link.
//!   `weft-licence` serves plain HTTP on the USB and tailnet interfaces until
//!   TLS lands (W4), so today a real link needs it. Without a pin or the
//!   opt-in no relay is built.
//! - `max_artifact_bytes`, `timeout_secs`: optional bounds.
//!
//! The relay is built at placement start once the cog mesh exists, with the
//! node's governance gate (it asks `cog.checkout`) and the licence exchange
//! as its grant flood. Its client reads the binding on every call, so a bind
//! or a steward change at runtime needs no restart; while the binding does
//! not name this node the relay refuses with `not_steward` and sends nothing.
//! A node without the file answers `no_steward`.
//!
//! In service mode the relay and the renewer act only while this daemon holds
//! the service's reserved licence topics: the client checks the holder state
//! on every call (`not_holder`, nothing sent), so a role that comes or goes
//! after placement started is followed without a rebuild. They sign as the
//! mesh node id (the machine's, which the binding names as steward) with the
//! placement signer (the control key in service mode). Their grant flood and
//! the run gate's approval store are looked up on each use, so an exchange
//! that starts after placement is used as soon as it exists.
//!
//! The same client drives the renewal pull ([`clawft_kernel::licence::Renewer`]):
//! `POST /licence/v1/renew` every 12 h plus jitter, then the
//! `GET /licence/v1/grants` catch-up; renewed grants and withdrawals are
//! installed and flooded, withdrawals first. An unreachable Seed backs off.

use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use clawft_kernel::chain::ChainManager;
use clawft_kernel::gate::GateBackend;
use clawft_kernel::licence::{
    CheckoutGrantStore, CheckoutRelay, HttpLicenceTransport, LicenceLinkConfig, LicenceTransport,
    StewardLicenceClient, TransportLimits, system_clock_ms,
};
use clawft_kernel::mesh_artifact::ArtifactExchange;
use clawft_kernel::mesh_cog::CogMesh;
use clawft_kernel::workload_runtime::seed_tls::SeedTls;
use ed25519_dalek::SigningKey;
use serde::Deserialize;
use serde_json::{Value, json};

/// The link file under the runtime dir.
pub const LINK_FILE: &str = "licence-link.json";
/// Longest call timeout the file may ask for.
const MAX_TIMEOUT_SECS: u64 = 600;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkFile {
    url: String,
    #[serde(default)]
    tls_spki_sha256: Option<String>,
    #[serde(default)]
    tls_sha256: Option<String>,
    #[serde(default)]
    allow_unpinned_lab_link: bool,
    #[serde(default)]
    max_artifact_bytes: Option<u64>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

/// The link configured in `dir`, if any. Fails on a malformed file.
pub fn load_link(dir: &Path) -> Result<Option<LicenceLinkConfig>, String> {
    let Some(f) = crate::workload_place_policy::parse::<LinkFile>(dir, LINK_FILE)? else {
        return Ok(None);
    };
    let tls = match (&f.tls_spki_sha256, &f.tls_sha256) {
        (Some(_), Some(_)) => return Err(format!("{LINK_FILE}: give tls_spki_sha256 or tls_sha256, not both")),
        (Some(p), None) => SeedTls::pinned_spki(p).map_err(|e| format!("{LINK_FILE}: {e}"))?,
        (None, Some(p)) => SeedTls::pinned(p).map_err(|e| format!("{LINK_FILE}: {e}"))?,
        (None, None) => SeedTls::WebPki,
    };
    let mut limits = TransportLimits::default();
    if let Some(n) = f.max_artifact_bytes {
        if n == 0 || n > clawft_kernel::licence::MAX_ARTIFACT_BYTES {
            return Err(format!("{LINK_FILE}: max_artifact_bytes out of range"));
        }
        limits.max_artifact = n;
    }
    if let Some(s) = f.timeout_secs {
        if s == 0 || s > MAX_TIMEOUT_SECS {
            return Err(format!("{LINK_FILE}: timeout_secs must be 1 to {MAX_TIMEOUT_SECS}"));
        }
        limits.call_timeout = Duration::from_secs(s);
        limits.artifact_timeout = limits.artifact_timeout.max(Duration::from_secs(s));
    }
    Ok(Some(LicenceLinkConfig { url: f.url, tls, allow_unpinned_lab_link: f.allow_unpinned_lab_link, limits }))
}

/// What placement start decided about the relay, for status and doctor.
#[derive(Debug, Clone)]
pub struct RelayState {
    /// A link file is present.
    pub link_configured: bool,
    /// The relay is installed in the cog mesh.
    pub relay_installed: bool,
    /// `pinned` or `lab_opt_in`, when installed.
    pub link_security: Option<&'static str>,
    /// Why no relay was built although a link file is present.
    pub error: Option<String>,
    /// The node id the relay signs as (the mesh node id: the machine's in
    /// service mode, which the binding names as `steward_node_id`).
    pub node_id: String,
}

static STATE: OnceLock<RelayState> = OnceLock::new();

/// The relay state (`None` before placement started).
pub fn state() -> Option<RelayState> {
    STATE.get().cloned()
}

/// Everything [`wire`] reads.
pub struct WireArgs<'a> {
    /// The runtime dir.
    pub dir: &'a Path,
    /// The placement signer (the binding's `steward_pubkey` when this node is
    /// the steward; the control key in service mode).
    pub key: &'a SigningKey,
    /// This node's mesh id (the binding's `steward_node_id` when this node is
    /// the steward; the machine's node id in service mode, never the control
    /// key's id).
    pub node_id: String,
    /// The boot-owned grant store.
    pub store: &'a Arc<CheckoutGrantStore>,
    /// The node's artifact exchange.
    pub exchange: &'a Arc<ArtifactExchange>,
    /// The kernel chain.
    pub chain: &'a Arc<ChainManager>,
    /// The node's governance gate (`cog.checkout` is asked of it).
    pub gate: Option<Arc<dyn GateBackend>>,
    /// The installed cog mesh.
    pub mesh: &'a Arc<CogMesh>,
}

type Built = (Arc<CheckoutRelay>, Arc<dyn clawft_kernel::licence::LicenceClient>, &'static str);

fn build(a: &WireArgs<'_>, link: LicenceLinkConfig) -> Result<Built, String> {
    let gate = a.gate.clone().ok_or("no governance gate on this kernel: the relay asks it for cog.checkout")?;
    let transport = HttpLicenceTransport::new(link).map_err(|e| e.to_string())?;
    let security = match transport.link_security() {
        clawft_kernel::workload_runtime::LinkSecurity::Pinned => "pinned",
        _ => "lab_opt_in",
    };
    let transport: Arc<dyn LicenceTransport> = Arc::new(transport);
    let client = steward_client(a.store, a.key, &a.node_id, transport, Arc::new(holder_allows));
    let relay = CheckoutRelay::new(
        a.store.clone(),
        a.exchange.clone(),
        client.clone(),
        gate,
        crate::cog_swarm::late_grant_flood(),
        Some(a.chain.clone()),
    );
    Ok((Arc::new(relay), client, security))
}

/// Start the renewal pull (every 12 h plus jitter, first pass 5 min after
/// start): renewed grants and withdrawals are installed and flooded. It
/// skips while the binding does not name this node.
fn spawn_renewal(a: &WireArgs<'_>, client: Arc<dyn clawft_kernel::licence::LicenceClient>) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let r = clawft_kernel::licence::Renewer::new(
        a.store.clone(),
        a.exchange.clone(),
        client,
        crate::cog_swarm::late_grant_flood(),
        Some(a.chain.clone()),
        clawft_kernel::licence::RenewalConfig::default(),
    );
    let _ = RENEWER.set(r.clone());
    r.spawn();
}

static RENEWER: OnceLock<Arc<clawft_kernel::licence::Renewer>> = OnceLock::new();

/// The steward's renewer (`weaver cog checkout renew | release` use its
/// path), when this node has a licence link and placement started it.
pub fn renewer() -> Option<Arc<clawft_kernel::licence::Renewer>> {
    RENEWER.get().cloned()
}

/// Build the relay from `licence-link.json` and install it in the cog mesh.
/// No file: no relay (the node answers `no_steward`). A bad file or link is
/// logged and reported by `weaver doctor`; the node runs without a relay.
pub fn wire(a: WireArgs<'_>) -> RelayState {
    let node_id = a.node_id.clone();
    let st = match load_link(a.dir) {
        Ok(None) => RelayState { link_configured: false, relay_installed: false, link_security: None, error: None, node_id },
        Ok(Some(link)) => match build(&a, link) {
            Ok((relay, client, security)) => {
                a.mesh.set_relay(Some(relay));
                spawn_renewal(&a, client);
                tracing::info!(security, "steward checkout relay installed (relays while the binding names this node)");
                RelayState { link_configured: true, relay_installed: true, link_security: Some(security), error: None, node_id }
            }
            Err(e) => {
                tracing::warn!(error = %e, "steward checkout relay NOT installed");
                RelayState { link_configured: true, relay_installed: false, link_security: None, error: Some(e), node_id }
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "licence link file unreadable; no steward relay");
            RelayState { link_configured: true, relay_installed: false, link_security: None, error: Some(e), node_id }
        }
    };
    let _ = STATE.set(st.clone());
    st
}

/// The run gate a `workload-host` asks for Cognitum-origin cogs: the
/// boot-owned grant store and the licence exchange's approval store, looked
/// up on every check (none while no exchange runs: every Cognitum run in a
/// Seed-bound mesh is then refused).
pub fn run_gate(store: &Arc<CheckoutGrantStore>) -> Arc<dyn clawft_kernel::licence::CognitumRunGate> {
    Arc::new(LateRunGate {
        grants: store.clone(),
        approvals: Arc::new(|| crate::workload_place_rpc::licence_exchange().map(|x| x.approvals().clone())),
        holder: Arc::new(crate::licence_boot::holder_state),
    })
}

type ApprovalSource = Arc<dyn Fn() -> Option<Arc<clawft_kernel::licence::ApprovalStore>> + Send + Sync>;

/// [`clawft_kernel::licence::StoreRunGate`] whose approval store is looked up
/// on each call, so an exchange that starts after placement counts at once.
pub struct LateRunGate {
    /// The boot-owned grant store.
    pub grants: Arc<CheckoutGrantStore>,
    /// Where the approval store is, when there is one.
    pub approvals: ApprovalSource,
    /// This daemon's licence role, read on every check.
    pub holder: Arc<dyn Fn() -> crate::licence_boot::HolderState + Send + Sync>,
}

impl LateRunGate {
    fn now(&self) -> clawft_kernel::licence::StoreRunGate {
        clawft_kernel::licence::StoreRunGate { grants: self.grants.clone(), approvals: (self.approvals)() }
    }

    /// One licence covers the whole mesh, local tenants included: on a
    /// machine of a Seed-licensed mesh (this daemon has a mesh id) a daemon
    /// that is not the licence holder runs no Cognitum cog, and does not fall
    /// back to the ADR-105 path. While the role is unknown the same holds for
    /// a daemon whose own store was never bound (a tenant), so a holder's
    /// link blip does not stop its restarts.
    fn role_refusal(&self) -> Option<clawft_kernel::licence::RunRefusal> {
        use crate::licence_boot::HolderState;
        self.grants.local_mesh_id().get()?;
        match (self.holder)() {
            HolderState::NotHolder => Some(clawft_kernel::licence::RunRefusal::NotHolder(
                "this is not the cluster owner's daemon".into(),
            )),
            HolderState::Unknown if !self.grants.was_ever_bound() => Some(clawft_kernel::licence::RunRefusal::NotHolder(
                "the licence holder could not be determined (mesh service query failed)".into(),
            )),
            _ => None,
        }
    }
}

impl clawft_kernel::licence::CognitumRunGate for LateRunGate {
    fn check(
        &self,
        req: &clawft_kernel::licence::RunRequest<'_>,
    ) -> Result<clawft_kernel::licence::RunVerdict, clawft_kernel::licence::RunRefusal> {
        if let Some(r) = self.role_refusal() {
            return Err(r);
        }
        self.now().check(req)
    }

    fn claims(&self, sha256: &str, blake3: &str) -> bool {
        self.now().claims(sha256, blake3)
    }

    fn revoked(&self, blake3: &str) -> bool {
        self.now().revoked(blake3)
    }
}

/// The licence path may act here: collapsed mode, or the service's
/// reserved-topic holder.
fn holder_allows() -> bool {
    crate::licence_boot::holder_refusal().is_none()
}

/// The steward client the relay and the renewer use: bound to the binding in
/// effect, signing as `node_id` with `key`, and refusing `not_holder` (with
/// nothing sent) while `allowed` says this daemon may not act.
pub fn steward_client(
    store: &Arc<CheckoutGrantStore>,
    key: &SigningKey,
    node_id: &str,
    transport: Arc<dyn LicenceTransport>,
    allowed: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Arc<dyn clawft_kernel::licence::LicenceClient> {
    let inner = StewardLicenceClient::new(store.clone(), key.clone(), node_id.to_owned(), transport, system_clock_ms());
    Arc::new(HolderGated { inner, allowed })
}

struct HolderGated {
    inner: Arc<StewardLicenceClient>,
    allowed: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl HolderGated {
    fn check(&self) -> Result<(), clawft_kernel::licence::LicenceClientError> {
        if (self.allowed)() {
            Ok(())
        } else {
            Err(clawft_kernel::licence::LicenceClientError::Refused { status: 0, code: "not_holder".into() })
        }
    }
}

#[async_trait::async_trait]
impl clawft_kernel::licence::LicenceClient for HolderGated {
    async fn checkout(
        &self,
        req: &clawft_kernel::licence::CheckoutWire,
    ) -> Result<clawft_kernel::licence::SignedGrant, clawft_kernel::licence::LicenceClientError> {
        self.check()?;
        self.inner.checkout(req).await
    }

    async fn artifact(&self, blake3_hex: &str, max_len: u64) -> Result<Vec<u8>, clawft_kernel::licence::LicenceClientError> {
        self.check()?;
        self.inner.artifact(blake3_hex, max_len).await
    }

    async fn grants_since(
        &self,
        since: u64,
    ) -> Result<Vec<clawft_kernel::licence::SignedGrant>, clawft_kernel::licence::LicenceClientError> {
        self.check()?;
        self.inner.grants_since(since).await
    }

    async fn grants_page(
        &self,
        since: u64,
    ) -> Result<clawft_kernel::licence::GrantsPage, clawft_kernel::licence::LicenceClientError> {
        self.check()?;
        self.inner.grants_page(since).await
    }

    async fn renew(&self) -> Result<clawft_kernel::licence::GrantsPage, clawft_kernel::licence::LicenceClientError> {
        self.check()?;
        self.inner.renew().await
    }

    async fn release(
        &self,
        cog_id: &str,
        version: &str,
    ) -> Result<clawft_kernel::licence::GrantsPage, clawft_kernel::licence::LicenceClientError> {
        self.check()?;
        self.inner.release(cog_id, version).await
    }
}

/// `{link_configured, relay_installed, link_security, error}` for status.
pub fn status_json(st: Option<&RelayState>) -> Value {
    match st {
        None => json!({ "started": false }),
        Some(s) => json!({
            "started": true, "link_configured": s.link_configured, "relay_installed": s.relay_installed,
            "link_security": s.link_security, "error": s.error,
        }),
    }
}

#[cfg(test)]
#[path = "licence_steward_tests.rs"]
mod late_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(LINK_FILE);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn the_link_file_is_optional_and_validated() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_link(tmp.path()).unwrap().is_none());
        write(tmp.path(), r#"{"url":"http://100.64.0.10:8700","allow_unpinned_lab_link":true,"timeout_secs":20}"#);
        let l = load_link(tmp.path()).unwrap().unwrap();
        assert!(l.allow_unpinned_lab_link);
        assert_eq!(l.limits.call_timeout, Duration::from_secs(20));
        write(tmp.path(), r#"{"url":"http://100.64.0.10:8700","bogus":1}"#);
        assert!(load_link(tmp.path()).is_err(), "unknown fields are refused");
        write(tmp.path(), r#"{"url":"https://x:1","tls_spki_sha256":"spki-sha256:00","tls_sha256":"sha256:00"}"#);
        assert!(load_link(tmp.path()).is_err());
        write(tmp.path(), r#"{"url":"http://100.64.0.10:8700","timeout_secs":0}"#);
        assert!(load_link(tmp.path()).is_err());
        let spki = format!("spki-sha256:{}", "ab".repeat(32));
        write(tmp.path(), &format!(r#"{{"url":"https://100.64.0.10:8700","tls_spki_sha256":"{spki}"}}"#));
        assert!(matches!(load_link(tmp.path()).unwrap().unwrap().tls, SeedTls::PinnedSpki(_)));
    }

    #[test]
    fn an_unpinned_link_without_the_opt_in_builds_no_transport() {
        let link = LicenceLinkConfig {
            url: "http://100.64.0.10:8700".into(),
            tls: SeedTls::WebPki,
            allow_unpinned_lab_link: false,
            limits: TransportLimits::default(),
        };
        assert!(HttpLicenceTransport::new(link).is_err());
    }
}
