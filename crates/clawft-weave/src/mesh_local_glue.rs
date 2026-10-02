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
use clawft_mesh_local::UserCert;
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
    match std::fs::symlink_metadata(&sock) {
        Ok(_) => {}
        // Only a missing path means "no service". Anything else (permission,
        // loop, I/O) means something may be there that this user cannot reach
        // or verify, which must not turn into a quiet fallback.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "cannot inspect {} ({e}); if the mesh service is installed, this user needs \
                 access to its socket directory (it is group-owned by the service group, \
                 mode 0750: add your account to that group and log in again)",
                sock.display()
            ));
        }
    }
    let record_path = state_dir.join("service.json");
    let record = ServiceRecord::load(&record_path).map_err(|e| {
        format!(
            "{} is unreadable ({e}); without it the service's machine key cannot be checked",
            record_path.display()
        )
    })?;
    check_record_owner(&record_path, record.service_uid)?;
    // A fresh user.key would split the identity while a legacy chain waits.
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

/// `service.json` is what clients pin: it must belong to root or the service
/// account, or anyone who can write the state dir could repoint the pin.
fn check_record_owner(path: &Path, service_uid: u32) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let owner = std::fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .uid();
    if owner == 0 || owner == service_uid {
        Ok(())
    } else {
        Err(format!(
            "{} is owned by uid {owner}, not root or the service account (uid {service_uid}); \
             refusing to trust it",
            path.display()
        ))
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
    let sink = Arc::new(MeshSink::new(deps.delivery.clone(), client.register_ack().user_id.clone()));
    let mut backoff = Backoff::new(deps.timings.backoff.0, deps.timings.backoff.1);
    let mut next = Some(client);
    let mut last_security_log: Option<std::time::Instant> = None;
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
            () = tokio::time::sleep(backoff.next_delay()) => {}
            _ = shutdown.changed() => return,
        }
        deps.chain.flush();
        sync_chain_counts(&deps);
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
            Err(e) if is_security_failure(&e) => {
                if last_security_log.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                    last_security_log = Some(std::time::Instant::now());
                    error!(
                        error = %e,
                        "the mesh service failed verification on reconnect and is NOT being used. \
                         service.json is read once at boot; after a legitimate machine key rotation run \
                         `weaver mesh trust` (from the mesh service package) and restart this daemon"
                    );
                }
            }
            Err(e) => debug!(error = %e, "mesh service reconnect failed"),
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

fn dead(e: &ClientError) -> bool {
    !matches!(e, ClientError::Server(_) | ClientError::Unexpected(_) | ClientError::Addr(_))
}

/// Bound on deliveries waiting for the router; past it new ones are dropped
/// and counted, so a slow inbox cannot stall anything else.
const DELIVER_QUEUE: usize = 256;
/// Bound on concurrent outbound sends.
const MAX_SENDS: usize = 64;

/// One connected session. The loop only waits on events, commands and timers;
/// deliveries, sends, verdicts and anchors run on their own tasks (verdicts
/// and sends concurrently, deliveries in order), so renewal, verdict replies
/// and forwarding never queue behind a slow inbox.
async fn session(
    mut c: MeshLocalClient,
    sink: &Arc<MeshSink>,
    deps: &LinkDeps,
    out_rx: &mut mpsc::Receiver<OutCmd>,
    shutdown: &mut watch::Receiver<bool>,
    node_id: &str,
) -> SessionEnd {
    let mut events = c.take_events();
    let c = Arc::new(c);
    let mut tasks = tokio::task::JoinSet::new();
    let (lost_tx, mut lost_rx) = mpsc::unbounded_channel::<String>();
    let (deliver_tx, mut deliver_rx) = mpsc::channel::<clawft_mesh_local::proto::Deliver>(DELIVER_QUEUE);
    let worker_sink = sink.clone();
    tasks.spawn(async move {
        while let Some(d) = deliver_rx.recv().await {
            if let Err(why) = worker_sink.deliver(d).await {
                warn!(%why, "inbound mesh delivery failed");
            }
        }
    });
    let sends = Arc::new(tokio::sync::Semaphore::new(MAX_SENDS));
    let anchoring = Arc::new(AtomicBool::new(false));
    let last_anchor: Arc<std::sync::Mutex<Option<(u64, String)>>> = Arc::default();
    let mut dropped_deliveries = 0u64;
    let first_cert = c.cert().clone();
    let mut anchor_tick = tokio::time::interval(deps.timings.anchor_every);
    let mut renew_at = tokio::time::Instant::now() + renew_after(&first_cert);
    let end = loop {
        tokio::select! {
            _ = shutdown.changed() => break SessionEnd::Shutdown,
            Some(why) = lost_rx.recv() => break SessionEnd::Lost(why),
            ev = events.recv() => match ev {
                None => break SessionEnd::Lost("service closed the connection".into()),
                Some(Frame { msg: Message::Deliver(d), .. }) => {
                    if deliver_tx.try_send(d).is_err() {
                        dropped_deliveries += 1;
                        if dropped_deliveries.is_power_of_two() {
                            warn!(dropped_deliveries, "inbound mesh deliveries are backing up; dropping");
                        }
                    }
                }
                Some(Frame { id: Some(id), msg: Message::VerdictRequest(req) }) => {
                    let (c, gate, user) = (c.clone(), deps.gate.clone(), c.register_ack().user_id.clone());
                    tasks.spawn(async move {
                        let a = mesh_local_verdict::answer(gate.as_deref(), &user, &req);
                        let reply = Message::VerdictReply {
                            allow: a.allow,
                            ttl_s: a.ttl_s,
                            reason: a.reason,
                            rule_hash: a.rule_hash,
                        };
                        if let Err(e) = c.reply(id, reply).await {
                            debug!(error = %e, "verdict reply failed");
                        }
                    });
                }
                Some(other) => debug!(?other, "ignored mesh service event"),
            },
            Some(cmd) = out_rx.recv() => {
                let Ok(permit) = sends.clone().try_acquire_owned() else {
                    let _ = cmd.reply.send(Err("too many sends in flight (busy)".into()));
                    continue;
                };
                let (c, lost_tx) = (c.clone(), lost_tx.clone());
                tasks.spawn(async move {
                    let r = c.send(&cmd.dest, cmd.message).await;
                    if let Some(why) = r.as_ref().err().filter(|e| dead(e)).map(ToString::to_string) {
                        let _ = lost_tx.send(why);
                    }
                    let _ = cmd.reply.send(r.map(|_| ()).map_err(|e| e.to_string()));
                    drop(permit);
                });
            }
            () = tokio::time::sleep_until(renew_at) => match renew(&c).await {
                Ok(fresh) => {
                    publish_cert(&deps.state, &c, &fresh, "connected");
                    renew_at = tokio::time::Instant::now() + renew_after(&fresh);
                }
                Err(e) => break SessionEnd::Lost(format!("certificate renewal failed: {e}")),
            },
            _ = anchor_tick.tick() => {
                if !anchoring.swap(true, Ordering::AcqRel) {
                    let (c, chain, last, flag, lost_tx, node) = (
                        c.clone(), deps.chain.clone(), last_anchor.clone(), anchoring.clone(),
                        lost_tx.clone(), node_id.to_owned(),
                    );
                    tasks.spawn(async move {
                        match c.request(Message::JournalHead {}).await {
                            Ok(Message::JournalHeadReply { seq, hash, ts, .. }) => {
                                let mut l = last.lock().unwrap_or_else(|e| e.into_inner());
                                if l.as_ref() != Some(&(seq, hash.clone())) {
                                    chain.anchor(seq, &hash, &node, ts);
                                    *l = Some((seq, hash));
                                }
                            }
                            Ok(other) => debug!(?other, "unexpected journal.head reply"),
                            Err(e) if dead(&e) => { let _ = lost_tx.send(format!("journal.head: {e}")); }
                            Err(e) => debug!(error = %e, "journal.head refused"),
                        }
                        chain.flush();
                        flag.store(false, Ordering::Release);
                    });
                }
                deps.chain.flush();
                sync_chain_counts(deps);
            }
        }
    };
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    if let Ok(c) = Arc::try_unwrap(c) {
        if matches!(end, SessionEnd::Shutdown) {
            c.close().await;
        }
    }
    end
}

/// Ask for a fresh certificate and verify it against the machine key and our
/// user key (what `MeshLocalClient::renew` does, without needing `&mut`).
async fn renew(c: &MeshLocalClient) -> Result<UserCert, ClientError> {
    match c.request(Message::Renew {}).await? {
        Message::Cert { cert } => {
            cert.verify(&c.hello_ack().machine_pubkey, clawft_mesh_local::client::now_unix())?;
            if cert.user_pubkey != c.cert().user_pubkey {
                return Err(ClientError::Unexpected("renewed cert has a different user key".into()));
            }
            Ok(cert)
        }
        other => Err(ClientError::Unexpected(format!("{other:?}"))),
    }
}

/// Renew at half the certificate's lifetime (at least a second).
fn renew_after(cert: &UserCert) -> Duration {
    let life = cert.not_after.saturating_sub(cert.issued_at);
    Duration::from_secs((life / 2).max(1))
}

/// Collapsed or off: record the mode for the handshake.
pub fn record_plain_mode(state: &MeshStateCell, resolved: &Resolved) {
    match resolved {
        Resolved::Collapsed => state.set(plain("collapsed")),
        Resolved::Off => state.set(plain("off")),
        Resolved::Service(_) => {}
    }
}
