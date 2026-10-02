//! The user daemon as a client of the machine mesh service (ADR-103 P3-U).
//!
//! [`resolve`] decides the mesh mode at boot (`kernel.mesh.service`): it probes
//! the service by really connecting (hello, server verification against the
//! pinned machine key, register), so a service that answers but fails
//! verification fails the boot instead of silently falling back. In service
//! mode [`spawn`] then keeps the link alive: certificate renewal, inbound
//! delivery, verdicts answered by the governance gate, journal anchors into the
//! user chain, outbound remote-node messages as `send`. If the service drops
//! the daemon keeps running and the link reconnects with jittered backoff.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clawft_kernel::gate::GateBackend;
use clawft_kernel::mesh_delivery::LocalDelivery;
use clawft_kernel::mesh_mode::{self, MeshMode, ServiceProbe};
use clawft_mesh_local::client::RegisterParams;
use clawft_mesh_local::UserCert;
use clawft_mesh_local::{Backoff, ClientConfig, ClientError, MeshLocalClient};
use clawft_rpc::handshake::MeshHandshake;
use clawft_types::config::MeshConfig;
use ed25519_dalek::SigningKey;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

use crate::mesh_local_chain::ChainQueue;
use crate::mesh_local_sink::{MeshSink, OutCmd, ServiceForwarder};
use crate::mesh_state::{MeshStateCell, plain};
use crate::node_identity::{DaemonIdentity, IdentityError};

mod endpoint;
mod session;
pub use endpoint::{build_endpoint, build_endpoint_in, state_dir};
use session::session;

/// Environment override for the service state dir (holds `service.json`).
pub const STATE_DIR_ENV: &str = "WEFTOS_MESH_STATE_DIR";
/// Default service state dir.
pub const DEFAULT_STATE_DIR: &str = "/var/lib/weftos/mesh";

/// Link tuning. Defaults are production values; tests shrink them.
#[derive(Debug, Clone)]
pub struct Timings {
    /// How often to anchor the service journal head.
    pub anchor_every: Duration,
    /// Reconnect backoff `(base, max)`.
    pub backoff: (Duration, Duration),
    /// Per-step handshake / request deadline.
    pub deadline: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            anchor_every: Duration::from_secs(60),
            backoff: (Duration::from_millis(500), Duration::from_secs(30)),
            deadline: Duration::from_secs(5),
        }
    }
}

/// Everything needed to connect and register.
pub struct ServiceEndpoint {
    /// Socket, pin, deadlines.
    pub client: ClientConfig,
    /// The user key that signs registrations.
    pub user_key: SigningKey,
    /// What to register.
    pub register: RegisterParams,
}

/// The decision [`resolve`] reached.
pub enum Resolved {
    /// No mesh.
    Off,
    /// Mesh runs in this daemon.
    Collapsed,
    /// Mesh is the service's; this daemon is registered with it.
    Service(Box<ServiceLink>),
}

/// A verified, registered connection to the service.
pub struct ServiceLink {
    endpoint: ServiceEndpoint,
    client: MeshLocalClient,
}

impl ServiceLink {
    /// The service's node id (`hello_ack.node_id`), which becomes this
    /// daemon's.
    pub fn node_id(&self) -> &str {
        &self.client.hello_ack().node_id
    }

    /// This user's id.
    pub fn user_id(&self) -> &str {
        &self.client.register_ack().user_id
    }

    /// The daemon identity in service mode: the service's node id and public
    /// key, and no signing key.
    pub fn identity(&self) -> Result<DaemonIdentity, IdentityError> {
        let ack = self.client.hello_ack();
        DaemonIdentity::for_service(ack.node_id.clone(), ack.machine_pubkey)
    }
}

fn is_absent(e: &ClientError) -> bool {
    matches!(e, ClientError::Io(io) if matches!(
        io.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    ))
}

async fn connect(ep: &ServiceEndpoint) -> Result<MeshLocalClient, ClientError> {
    MeshLocalClient::connect_and_register(&ep.client, &ep.user_key, &ep.register).await
}

/// Delays between probe retries when a socket exists but refuses connections
/// (a service that is still starting, or a stale socket).
pub const PROBE_RETRIES: [Duration; 3] =
    [Duration::from_millis(200), Duration::from_millis(500), Duration::from_secs(1)];

/// Decide the mesh mode (`kernel.mesh.service`). Errors carry the reason the
/// boot must fail with.
pub async fn resolve(
    cfg: &MeshConfig,
    endpoint: Result<Option<ServiceEndpoint>, String>,
) -> Result<Resolved, String> {
    resolve_with(cfg, endpoint, &PROBE_RETRIES).await
}

/// [`resolve`] with explicit retry delays.
pub async fn resolve_with(
    cfg: &MeshConfig,
    endpoint: Result<Option<ServiceEndpoint>, String>,
    retries: &[Duration],
) -> Result<Resolved, String> {
    if !mesh_mode::wants_probe(cfg) {
        return Ok(from_mode(mesh_mode::decide(cfg, None)?));
    }
    let (probe, link) = match endpoint {
        Err(why) => (ServiceProbe::Refused(why), None),
        Ok(None) => (ServiceProbe::Absent, None),
        Ok(Some(ep)) => {
            let mut attempt = 0;
            loop {
                match connect(&ep).await {
                    Ok(client) => {
                        break (ServiceProbe::Present, Some(ServiceLink { endpoint: ep, client }));
                    }
                    Err(e) if is_refused(&e) && attempt < retries.len() => {
                        tokio::time::sleep(retries[attempt]).await;
                        attempt += 1;
                    }
                    Err(e) if is_absent(&e) => {
                        // The socket path existed but nothing serves it: say so,
                        // because the daemon is about to run its own listener
                        // and its own node key instead.
                        warn!(
                            socket = %ep.client.socket_path.display(),
                            error = %e,
                            policy = ?cfg.service,
                            "the mesh service socket exists but did not answer; this daemon falls back to \
                             collapsed mode (its own mesh listener and node.key) unless kernel.mesh.service \
                             is \"required\""
                        );
                        break (ServiceProbe::Absent, None);
                    }
                    Err(e) => break (ServiceProbe::Refused(e.to_string()), None),
                }
            }
        }
    };
    let mode = mesh_mode::decide(cfg, Some(probe))?;
    Ok(match (mode, link) {
        (MeshMode::Service { .. }, Some(link)) => Resolved::Service(Box::new(link)),
        (m, _) => from_mode(m),
    })
}

fn is_refused(e: &ClientError) -> bool {
    matches!(e, ClientError::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused)
}

/// After a collapsed fallback under `auto`: keep checking whether a service
/// appears, and say so loudly once. The mode is never switched at runtime;
/// the operator restarts the daemon.
pub fn watch_for_service(sock: PathBuf, every: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(every).await;
            if tokio::net::UnixStream::connect(&sock).await.is_ok() {
                error!(
                    socket = %sock.display(),
                    "a machine mesh service is now running, but this daemon booted in collapsed mode \
                     with its own mesh listener and node key; restart the daemon to use the service \
                     (the two cannot share port 9489)"
                );
                return;
            }
        }
    })
}

fn from_mode(mode: MeshMode) -> Resolved {
    match mode {
        MeshMode::Collapsed => Resolved::Collapsed,
        _ => Resolved::Off,
    }
}

/// What the link task needs from the daemon.
pub struct LinkDeps {
    /// Where inbound messages go (the daemon's `A2ARouter`).
    pub delivery: Arc<dyn LocalDelivery>,
    /// Governance gate answering verdicts (`None`: every verdict is denied).
    pub gate: Option<Arc<dyn GateBackend>>,
    /// Chain events (anchors, binding).
    pub chain: Arc<ChainQueue>,
    /// Handshake state to keep current.
    pub state: Arc<MeshStateCell>,
    /// Tuning.
    pub timings: Timings,
}

/// Handle to the running link.
pub struct LinkHandle {
    /// Install on the router with `set_remote_forwarder`.
    pub forwarder: Arc<ServiceForwarder>,
    shutdown: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl LinkHandle {
    /// Stop the link and wait for the task.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }
}

/// Start the link task over an established [`ServiceLink`].
pub fn spawn(link: Box<ServiceLink>, deps: LinkDeps) -> LinkHandle {
    // The handshake shows the link before the task gets its first turn.
    publish(&deps.state, &link.client, "connected");
    let connected = Arc::new(AtomicBool::new(true));
    let (forwarder, out_rx) = ServiceForwarder::new(connected.clone());
    let (shutdown, shutdown_rx) = watch::channel(false);
    let task = tokio::spawn(run(link, deps, out_rx, connected, shutdown_rx));
    LinkHandle { forwarder, shutdown, task }
}

fn publish(state: &MeshStateCell, c: &MeshLocalClient, link_state: &str) {
    publish_cert(state, c, c.cert(), link_state);
}

fn publish_cert(
    state: &MeshStateCell,
    c: &MeshLocalClient,
    cert: &UserCert,
    link_state: &str,
) {
    let (pending, dropped) = state
        .get()
        .map_or((None, None), |s| (s.events_pending, s.events_dropped));
    state.set(MeshHandshake {
        mode: "service".into(),
        state: Some(link_state.into()),
        service_node_id: Some(c.hello_ack().node_id.clone()),
        cert_serial: Some(cert.serial),
        cert_not_after: Some(cert.not_after),
        proto: Some(c.proto()),
        events_pending: pending,
        events_dropped: dropped,
    });
}

impl ServiceLink {
    /// Record service mode in the handshake state now, so the roles are right
    /// before the link task starts.
    pub fn publish_state(&self, state: &MeshStateCell) {
        publish(state, &self.client, "connected");
    }
}

enum SessionEnd {
    Shutdown,
    Lost(String),
}

/// Failures that retrying cannot fix and that may mean an impostor or a
/// rotated key, as opposed to plain transport trouble.
fn is_security_failure(e: &ClientError) -> bool {
    matches!(
        e,
        ClientError::MachineKeyChanged { .. }
            | ClientError::ServerUid { .. }
            | ClientError::BadServerProof
            | ClientError::UidMismatch { .. }
            | ClientError::PinCorrupt(_)
    )
}

async fn run(
    link: Box<ServiceLink>,
    deps: LinkDeps,
    mut out_rx: mpsc::Receiver<OutCmd>,
    connected: Arc<AtomicBool>,
    mut shutdown: watch::Receiver<bool>,
) {
    let ServiceLink { endpoint, client } = *link;
    let node_id = client.hello_ack().node_id.clone();
    let sink = Arc::new(
        MeshSink::new(deps.delivery.clone(), client.register_ack().user_id.clone())
            .with_machine_key(client.hello_ack().machine_pubkey),
    );
    let mut backoff = Backoff::new(deps.timings.backoff.0, deps.timings.backoff.1);
    let mut next = Some(client);
    let mut last_security_log: Option<std::time::Instant> = None;
    // The service refused the registration itself (key rebound or revoked,
    // bind pending, rate limited): retry at the slowest pace so a daemon left
    // with a stale key does not burn its uid's registration budget.
    let mut refused: Option<std::time::Instant> = None;
    loop {
        if let Some(c) = next.take() {
            publish(&deps.state, &c, "connected");
            connected.store(true, Ordering::Release);
            deps.chain.bound(&node_id, c.cert().serial);
            backoff.reset();
            info!(node_id = %node_id, serial = c.cert().serial, "mesh service link up");
            let end = session(c, &sink, &deps, &mut out_rx, &mut shutdown, &node_id).await;
            connected.store(false, Ordering::Release);
            fail_pending(&mut out_rx);
            match end {
                SessionEnd::Shutdown => return,
                SessionEnd::Lost(why) => {
                    warn!(%why, "mesh service link lost; reconnecting (the daemon keeps running)");
                    deps.state.update(|s| s.state = Some("reconnecting".into()));
                }
            }
        }
        tokio::select! {
            () = tokio::time::sleep(if refused.is_some() { deps.timings.backoff.1 } else { backoff.next_delay() }) => {}
            _ = shutdown.changed() => return,
        }
        deps.chain.flush();
        sync_chain_counts(&deps);
        let mut ep_client = endpoint.client.clone();
        ep_client.deadline = deps.timings.deadline;
        match MeshLocalClient::connect_and_register(&ep_client, &endpoint.user_key, &endpoint.register).await {
            Ok(c) if c.hello_ack().node_id == node_id => {
                refused = None;
                next = Some(c);
            }
            Ok(c) => {
                error!(
                    old = %node_id,
                    new = %c.hello_ack().node_id,
                    "the mesh service now has a different node id; restart this daemon to adopt it"
                );
                c.close().await;
            }
            Err(e) if is_security_failure(&e) => {
                if last_security_log.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                    last_security_log = Some(std::time::Instant::now());
                    error!(
                        error = %e,
                        "the mesh service failed verification on reconnect and is NOT being used. \
                         service.json is read once at boot; after a legitimate machine key rotation run \
                         `weaver mesh trust --replace` and restart this daemon"
                    );
                }
            }
            Err(ClientError::Server(e)) => {
                if refused.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                    refused = Some(std::time::Instant::now());
                    warn!(
                        kind = ?e.kind,
                        message = %e.message,
                        remedy = %e.remedy,
                        "the mesh service refused this daemon's registration; retrying slowly. After \
                         `weaver mesh bind rebind` or `revoke`, restart this daemon with the current user key"
                    );
                }
            }
            Err(e) => {
                // Transport trouble (the service restarting) is not a refusal:
                // back to the normal backoff.
                refused = None;
                debug!(error = %e, "mesh service reconnect failed");
            }
        }
    }
}

fn sync_chain_counts(deps: &LinkDeps) {
    let (p, d) = (deps.chain.pending() as u64, deps.chain.dropped());
    deps.state.update(|s| {
        s.events_pending = Some(p);
        s.events_dropped = Some(d);
    });
}

/// Fail every outbound message still queued when the link drops.
fn fail_pending(out_rx: &mut mpsc::Receiver<OutCmd>) {
    while let Ok(cmd) = out_rx.try_recv() {
        let _ = cmd.reply.send(Err("the mesh service link dropped".into()));
    }
}

/// Collapsed or off: record the mode for the handshake.
pub fn record_plain_mode(state: &MeshStateCell, resolved: &Resolved) {
    match resolved {
        Resolved::Collapsed => state.set(plain("collapsed")),
        Resolved::Off => state.set(plain("off")),
        Resolved::Service(_) => {}
    }
}
