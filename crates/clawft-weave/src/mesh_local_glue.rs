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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clawft_kernel::gate::GateBackend;
use clawft_kernel::mesh_delivery::LocalDelivery;
use clawft_kernel::mesh_mode::{self, MeshMode, ServiceProbe};
use clawft_mesh_local::client::RegisterParams;
use clawft_mesh_local::proto::{Frame, Message, ServiceRecord};
use clawft_mesh_local::{Backoff, ClientConfig, ClientError, MeshLocalClient};
use clawft_rpc::handshake::MeshHandshake;
use clawft_types::config::MeshConfig;
use ed25519_dalek::SigningKey;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

use crate::mesh_local_chain::ChainQueue;
use crate::mesh_local_sink::{MeshSink, OutCmd, ServiceForwarder};
use crate::mesh_local_verdict;
use crate::mesh_state::{MeshStateCell, plain};
use crate::node_identity::{DaemonIdentity, IdentityError};

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

/// `$WEFTOS_MESH_STATE_DIR` or the default state dir.
pub fn state_dir() -> PathBuf {
    std::env::var_os(STATE_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from(DEFAULT_STATE_DIR), PathBuf::from)
}

/// Build the endpoint for the real service. `Ok(None)`: there is no socket,
/// so no service. `Err`: something is there that cannot be verified
/// (unreadable `service.json`, unusable user key); the caller treats that as
/// a refusal.
pub fn build_endpoint(
    cfg: &MeshConfig,
    home: &Path,
    build_sha: &str,
) -> Result<Option<ServiceEndpoint>, String> {
    build_endpoint_in(cfg, home, build_sha, &state_dir())
}

/// [`build_endpoint`] with an explicit service state dir.
pub fn build_endpoint_in(
    cfg: &MeshConfig,
    home: &Path,
    build_sha: &str,
    state_dir: &Path,
) -> Result<Option<ServiceEndpoint>, String> {
    let sock = mesh_mode::service_socket(cfg);
    if std::fs::symlink_metadata(&sock).is_err() {
        return Ok(None);
    }
    let record_path = state_dir.join("service.json");
    let record = ServiceRecord::load(&record_path).map_err(|e| {
        format!(
            "{} is unreadable ({e}); without it the service's machine key cannot be checked",
            record_path.display()
        )
    })?;
    let (user_key, _) =
        crate::user_key::resolve_user_key(home, true).map_err(|e| format!("user key: {e}"))?;
    let mut client = ClientConfig::new(&sock, record);
    client.machine_pin = Some(clawft_types::runtime_paths::user_weftos_dir(home).join("mesh/machine.pub"));
    client.build_sha = build_sha.to_owned();
    client.exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    let user_id = clawft_mesh_local::node_id_from_pubkey(&user_key.verifying_key().to_bytes());
    let register = RegisterParams {
        projects: Vec::new(),
        topic_prefixes: vec![format!("user/{user_id}/")],
        capabilities: vec!["a2a".to_owned()],
        version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    Ok(Some(ServiceEndpoint { client, user_key, register }))
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

/// Decide the mesh mode (`kernel.mesh.service`). Errors carry the reason the
/// boot must fail with.
pub async fn resolve(
    cfg: &MeshConfig,
    endpoint: Result<Option<ServiceEndpoint>, String>,
) -> Result<Resolved, String> {
    if !mesh_mode::wants_probe(cfg) {
        return Ok(from_mode(mesh_mode::decide(cfg, None)?));
    }
    let (probe, link) = match endpoint {
        Err(why) => (ServiceProbe::Refused(why), None),
        Ok(None) => (ServiceProbe::Absent, None),
        Ok(Some(ep)) => match connect(&ep).await {
            Ok(client) => (ServiceProbe::Present, Some(ServiceLink { endpoint: ep, client })),
            Err(e) if is_absent(&e) => (ServiceProbe::Absent, None),
            Err(e) => (ServiceProbe::Refused(e.to_string()), None),
        },
    };
    let mode = mesh_mode::decide(cfg, Some(probe))?;
    Ok(match (mode, link) {
        (MeshMode::Service { .. }, Some(link)) => Resolved::Service(Box::new(link)),
        (m, _) => from_mode(m),
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
    let cert = c.cert();
    state.set(MeshHandshake {
        mode: "service".into(),
        state: Some(link_state.into()),
        service_node_id: Some(c.hello_ack().node_id.clone()),
        cert_serial: Some(cert.serial),
        cert_not_after: Some(cert.not_after),
        proto: Some(c.proto()),
    });
}

enum SessionEnd {
    Shutdown,
    Lost(String),
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
    let sink = MeshSink::new(deps.delivery.clone(), client.register_ack().user_id.clone());
    let mut backoff = Backoff::new(deps.timings.backoff.0, deps.timings.backoff.1);
    let mut next = Some(client);
    loop {
        if let Some(mut c) = next.take() {
            publish(&deps.state, &c, "connected");
            connected.store(true, Ordering::Release);
            deps.chain.bound(&node_id, c.cert().serial);
            backoff.reset();
            info!(node_id = %node_id, serial = c.cert().serial, "mesh service link up");
            let end = session(&mut c, &sink, &deps, &mut out_rx, &mut shutdown, &node_id).await;
            connected.store(false, Ordering::Release);
            fail_pending(&mut out_rx);
            match end {
                SessionEnd::Shutdown => {
                    c.close().await;
                    return;
                }
                SessionEnd::Lost(why) => {
                    warn!(%why, "mesh service link lost; reconnecting (the daemon keeps running)");
                    deps.state.update(|s| s.state = Some("reconnecting".into()));
                }
            }
        }
        tokio::select! {
            () = tokio::time::sleep(backoff.next_delay()) => {}
            _ = shutdown.changed() => return,
        }
        deps.chain.flush();
        let mut ep_client = endpoint.client.clone();
        ep_client.deadline = deps.timings.deadline;
        match MeshLocalClient::connect_and_register(&ep_client, &endpoint.user_key, &endpoint.register).await {
            Ok(c) if c.hello_ack().node_id == node_id => next = Some(c),
            Ok(c) => {
                error!(
                    old = %node_id,
                    new = %c.hello_ack().node_id,
                    "the mesh service now has a different node id; restart this daemon to adopt it"
                );
                c.close().await;
            }
            Err(e) => debug!(error = %e, "mesh service reconnect failed"),
        }
    }
}

/// Fail every outbound message still queued when the link drops.
fn fail_pending(out_rx: &mut mpsc::Receiver<OutCmd>) {
    while let Ok(cmd) = out_rx.try_recv() {
        let _ = cmd.reply.send(Err("the mesh service link dropped".into()));
    }
}

fn dead(e: &ClientError) -> bool {
    !matches!(e, ClientError::Server(_) | ClientError::Unexpected(_) | ClientError::Addr(_))
}

async fn session(
    c: &mut MeshLocalClient,
    sink: &MeshSink,
    deps: &LinkDeps,
    out_rx: &mut mpsc::Receiver<OutCmd>,
    shutdown: &mut watch::Receiver<bool>,
    node_id: &str,
) -> SessionEnd {
    let mut anchor_tick = tokio::time::interval(deps.timings.anchor_every);
    let mut renew_at = tokio::time::Instant::now() + renew_after(c);
    let mut last_anchor: Option<(u64, String)> = None;
    loop {
        tokio::select! {
            _ = shutdown.changed() => return SessionEnd::Shutdown,
            ev = c.next_event() => match ev {
                None => return SessionEnd::Lost("service closed the connection".into()),
                Some(frame) => handle_event(c, sink, deps, frame).await,
            },
            Some(cmd) = out_rx.recv() => {
                let r = c.send(&cmd.dest, cmd.message).await;
                let lost = r.as_ref().err().filter(|e| dead(e)).map(ToString::to_string);
                let _ = cmd.reply.send(r.map(|_| ()).map_err(|e| e.to_string()));
                if let Some(why) = lost { return SessionEnd::Lost(why); }
            }
            () = tokio::time::sleep_until(renew_at) => match c.renew().await {
                Ok(_) => {
                    publish(&deps.state, c, "connected");
                    renew_at = tokio::time::Instant::now() + renew_after(c);
                }
                Err(e) => return SessionEnd::Lost(format!("certificate renewal failed: {e}")),
            },
            _ = anchor_tick.tick() => {
                match c.request(Message::JournalHead {}).await {
                    Ok(Message::JournalHeadReply { seq, hash, ts, .. }) => {
                        if last_anchor.as_ref() != Some(&(seq, hash.clone())) {
                            deps.chain.anchor(seq, &hash, node_id, ts);
                            last_anchor = Some((seq, hash));
                        }
                    }
                    Ok(other) => debug!(?other, "unexpected journal.head reply"),
                    Err(e) if dead(&e) => return SessionEnd::Lost(format!("journal.head: {e}")),
                    Err(e) => debug!(error = %e, "journal.head refused"),
                }
                deps.chain.flush();
            }
        }
    }
}

/// Renew at half the certificate's lifetime (at least a second).
fn renew_after(c: &MeshLocalClient) -> Duration {
    let cert = c.cert();
    let life = cert.not_after.saturating_sub(cert.issued_at);
    Duration::from_secs((life / 2).max(1))
}

async fn handle_event(c: &MeshLocalClient, sink: &MeshSink, deps: &LinkDeps, frame: Frame) {
    match frame.msg {
        Message::Deliver(d) => {
            if let Err(why) = sink.deliver(d).await {
                warn!(%why, "inbound mesh delivery failed");
            }
        }
        Message::VerdictRequest(req) => {
            let Some(id) = frame.id else { return };
            let a = mesh_local_verdict::answer(deps.gate.as_deref(), &c.register_ack().user_id, &req);
            let reply = Message::VerdictReply {
                allow: a.allow,
                ttl_s: a.ttl_s,
                reason: a.reason,
                rule_hash: a.rule_hash,
            };
            if let Err(e) = c.reply(id, reply).await {
                debug!(error = %e, "verdict reply failed");
            }
        }
        other => debug!(?other, "ignored mesh service event"),
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
