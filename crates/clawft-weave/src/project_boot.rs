//! Child bootstrap: what a `project`-profile kernel does before it opens its
//! chain (ADR-103 A6, Phase 2 package H).
//!
//! [`pre_boot`] runs first in `daemon::run`. The order matters and is the
//! point of this module: the chain, the node key and the governance overlay
//! are all opened by the kernel afterwards, and every one of them needs the
//! project certificate, so the certificate has to exist (and verify) before
//! the kernel boots.
//!
//! 1. refuse unless `spawn.json` exists, is private, unexpired and valid; it
//!    is deleted as it is read ([`SpawnFile::read_and_consume`]);
//! 2. make [`RuntimePaths::resolve`] return this child's root for the rest
//!    of the process (never by walk-up; a `project` profile without a child
//!    root is a hard failure);
//! 3. load or create the one project key (`project.key`, 0600);
//! 4. `mesh.challenge`, then `mesh.register` with a proof of possession and
//!    the spawn nonce, to the user daemon. The acknowledgement is checked
//!    (user-key signature over a value this child chose), the certificate in
//!    it is verified under the user key from `spawn.json` (and the
//!    `user.pub` pin when the supervisor wrote one) and persisted;
//! 5. the signed parent policy and overlay are verified now
//!    ([`clawft_kernel::overlay_runtime::prepare`]), so a bad policy refuses
//!    the boot before any chain is touched;
//! 6. node id and project key id are asserted equal and the forward-header
//!    trust is installed from the certificate's user key.
//!
//! **Parent down at boot.** A child that has a cached, verifying certificate
//! (it registered on an earlier boot) continues degraded: it keeps running,
//! queues anchors on disk, and keeps trying to register. A child with no
//! certificate refuses to boot: nobody has ever vouched for its key.
//! A parent that answers and refuses always refuses the boot.
//!
//! The parent never takes the child's word for anything it can check: the
//! certificate is the user key's signature, the policy is the user key's
//! signature, the acknowledgement is the user key's signature. The socket
//! path in `spawn.json` is only where to ask; a socket squatter can make the
//! child wait, not make it run.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clawft_kernel::project_identity as ident;
use clawft_rpc::mesh_local::{
    ChallengeReply, ChallengeRequest, METHOD_CHALLENGE, METHOD_REGISTER, MeshRole, NonceReply,
    PROTOCOL_TAG, ParentHead, RegisterAck, RegisterRequest, ack_signed_bytes, bind_signed_bytes,
};
use clawft_types::config::{Config, KernelConfig, KernelProfile};
use clawft_types::project::canon::{hex_decode, hex_encode};
use clawft_types::project::cert::{PopOp, ProjectCert, key_id};
use clawft_types::project::{SpawnError, SpawnFile};
use clawft_types::runtime_paths::{RuntimePaths, set_child_profile};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use tracing::{info, warn};

use crate::node_identity::DaemonIdentity;
use crate::project_boot_link::{LinkError, call_async, verify_parent_socket};

/// Environment variable naming the project a child serves (set by the
/// supervisor next to `WEFTOS_RUNTIME_DIR`).
pub const PROJECT_ID_ENV: &str = "WEFTOS_PROJECT_ID";
/// The user daemon's per-call deadline for the registration exchange.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Message every spawn refusal starts with.
pub const NOT_SPAWNED_MSG: &str = "project kernels are started by the user daemon";

/// Why a child refused to boot.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    /// `profile = project` but no child root, or the root is inconsistent.
    #[error("{NOT_SPAWNED_MSG}: {0}")]
    NotSpawned(String),
    /// `spawn.json` was refused.
    #[error("{NOT_SPAWNED_MSG}: {0}")]
    Spawn(#[from] SpawnError),
    /// The user daemon revoked this project (`revoked` marker).
    #[error("this project was revoked by the user daemon ({0} exists)")]
    Revoked(PathBuf),
    /// The project key could not be loaded or created.
    #[error("project key: {0}")]
    Key(String),
    /// Parent unreachable and no certificate was ever issued.
    #[error(
        "the user daemon is unreachable ({0}) and this project has no certificate yet; \
         a project key nobody vouched for does not boot"
    )]
    NeverRegistered(String),
    /// The user daemon answered and refused.
    #[error("the user daemon refused this project's registration ({kind}): {message}")]
    Refused {
        /// Daemon `error_kind`.
        kind: String,
        /// Daemon message.
        message: String,
    },
    /// The certificate, acknowledgement or policy did not verify.
    #[error("{0}")]
    Untrusted(String),
    /// Anything else (filesystem).
    #[error("{0}")]
    Io(String),
}

/// What the child learned from the registration, kept for the running half.
#[derive(Clone)]
pub struct ChildBoot {
    /// This child's paths (also installed as the process's runtime paths).
    pub paths: RuntimePaths,
    /// Host root used in the certificate binding (guest path may differ).
    pub host_root: PathBuf,
    pub container: Option<clawft_rpc::mesh_local::ContainerRegistration>,
    /// The project key; the node key and the chain signing key.
    pub key: SigningKey,
    /// The certificate in force (verified).
    pub cert: ProjectCert,
    /// The user daemon's socket.
    pub parent_socket: PathBuf,
    /// The user public key every verification uses.
    pub user_pubkey: [u8; 32],
    /// `key_id` of the user key (named in proofs of possession).
    pub user_key_id: String,
    /// The session the user daemon opened; `None` when degraded.
    pub session: Option<String>,
    /// The user-chain head at registration (for `project.genesis`).
    pub parent_head: Option<ParentHead>,
    /// Heartbeat interval the user daemon asked for.
    pub heartbeat_secs: u64,
    /// Why the child is degraded, when it is.
    pub degraded: Option<String>,
    /// The spawn nonce, kept in memory only for a first registration after a
    /// degraded boot (it is void after 60 s; the parent decides).
    pub spawn_nonce: Option<String>,
    /// The project-scoped token for `shared.*` calls, from `spawn.json`
    /// (which is deleted once read); handed to package F's parent link.
    /// Never logged.
    pub project_token: Option<String>,
}

impl std::fmt::Debug for ChildBoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChildBoot")
            .field("project_id", &self.cert.project_id)
            .field("serial", &self.cert.serial)
            .field("session", &self.session)
            .field("degraded", &self.degraded)
            .finish_non_exhaustive()
    }
}

/// What [`pre_boot`] hands back to `run()`.
#[derive(Debug, Default)]
pub struct PreBoot {
    /// The node identity to boot with (the project key); `None` outside the
    /// `project` profile, where `run()` loads `node.key` as before.
    pub identity: Option<DaemonIdentity>,
    /// The child's registration, for [`crate::project_boot_run`].
    pub child: Option<ChildBoot>,
}

impl PreBoot {
    /// Take the identity `run()` must boot with.
    pub fn take_identity(&mut self) -> Option<DaemonIdentity> {
        self.identity.take()
    }
}

/// What package F's parent link needs from the (deleted) `spawn.json`:
/// socket, project id, token.
static SPAWN_LINK: std::sync::OnceLock<(PathBuf, String, Option<String>)> =
    std::sync::OnceLock::new();

/// The parent link's inputs from this child's consumed `spawn.json`, once
/// [`pre_boot`] has run for a project kernel.
pub fn spawn_link() -> Option<(PathBuf, String, Option<String>)> {
    SPAWN_LINK.get().cloned()
}

fn is_project(config: &Config, kernel_config: &KernelConfig) -> bool {
    kernel_config.profile == Some(KernelProfile::Project)
        || config.kernel.profile == Some(KernelProfile::Project)
}

/// Body of `project_hooks::pre_boot`. A no-op outside the `project` profile.
pub async fn pre_boot(config: &Config, kernel_config: &KernelConfig) -> Result<PreBoot, BootError> {
    if !is_project(config, kernel_config) {
        return Ok(PreBoot::default());
    }
    let run_dir = std::env::var_os(clawft_types::runtime_paths::RUNTIME_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            BootError::NotSpawned(
                "profile `project` needs a child run dir ($WEFTOS_RUNTIME_DIR)".into(),
            )
        })?;
    let id = std::env::var(PROJECT_ID_ENV)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            BootError::NotSpawned(format!("profile `project` needs ${PROJECT_ID_ENV}"))
        })?;
    let child = bootstrap(&run_dir, &id, now_unix(), CALL_TIMEOUT).await?;
    if !set_child_profile(Some(child.paths.clone())) {
        return Err(BootError::NotSpawned(
            "the child root is not a child root".into(),
        ));
    }
    let node_id = clawft_kernel::node_id_from_pubkey(&child.key.verifying_key().to_bytes());
    if node_id != child.cert.project_key_id {
        return Err(BootError::Untrusted(format!(
            "node id {node_id} is not the certified project key id {}",
            child.cert.project_key_id
        )));
    }
    match crate::project_forward::install_trust(&child.user_pubkey) {
        Ok(true) => {}
        Ok(false) => warn!("forward trust was already installed; keeping the first"),
        Err(e) => return Err(BootError::Untrusted(format!("forward trust: {e}"))),
    }
    let _ = SPAWN_LINK.set((
        child.parent_socket.clone(),
        child.cert.project_id.clone(),
        child.project_token.clone(),
    ));
    // The project key is this kernel's own (local) node key; node_id is
    // derived from it, and equals the certified key id checked above.
    let identity = DaemonIdentity::local(child.key.clone());
    debug_assert_eq!(identity.node_id, node_id);
    info!(
        project = %child.cert.project_id,
        serial = child.cert.serial,
        node_id = %identity.node_id,
        degraded = child.degraded.as_deref().unwrap_or("no"),
        "project kernel bootstrapped"
    );
    Ok(PreBoot {
        identity: Some(identity),
        child: Some(child),
    })
}

pub(crate) fn now_unix() -> u64 {
    crate::mesh_local_registry::now_unix()
}

/// Everything before the kernel boots, over explicit inputs (no process
/// state, so tests can run it repeatedly): see the module docs.
pub async fn bootstrap(
    run_dir: &Path,
    project_id: &str,
    now: u64,
    timeout: Duration,
) -> Result<ChildBoot, BootError> {
    bootstrap_with(run_dir, project_id, now, timeout, Retry::default()).await
}

/// Refusal kinds that mean "the user daemon cannot do it right now" rather
/// than "no": retried inside the spawn window instead of failing the boot.
const TRANSIENT_KINDS: &[&str] = &[
    "cert_unavailable",
    "project_store_error",
    "spawn_ledger_full",
    "challenge_unknown",
];

/// Retry policy for transient refusals during the first registration.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    /// Total attempts (at least 1).
    pub attempts: u32,
    /// First delay.
    pub initial: Duration,
    /// Delay cap.
    pub max: Duration,
    /// Total time to keep retrying; under the 60 s spawn window.
    pub budget: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            attempts: 6,
            initial: Duration::from_millis(500),
            max: Duration::from_secs(5),
            budget: Duration::from_secs(45),
        }
    }
}

/// [`bootstrap`] with an explicit [`Retry`].
pub async fn bootstrap_with(
    run_dir: &Path,
    project_id: &str,
    now: u64,
    timeout: Duration,
    retry: Retry,
) -> Result<ChildBoot, BootError> {
    let trust_dir = std::env::var_os("WEFTOS_TRUST_DIR").map(PathBuf::from);
    let spawn = SpawnFile::read_and_consume(
        &trust_dir.as_deref().unwrap_or(run_dir).join(clawft_types::runtime_paths::SPAWN_JSON_FILE),
        now,
    )?;
    if spawn.project_id != project_id {
        return Err(BootError::NotSpawned(format!(
            "spawn.json is for project {}, this kernel was told {project_id}",
            spawn.project_id
        )));
    }
    if run_dir.file_name().and_then(|n| n.to_str()) != Some(project_id) {
        return Err(BootError::NotSpawned(format!(
            "run dir {} is not named for project {project_id}",
            run_dir.display()
        )));
    }
    if let Some(c) = &spawn.container
        && (run_dir != c.guest_runtime_root || trust_dir.as_deref() != Some(c.guest_trust_root.as_path()))
    {
        return Err(BootError::NotSpawned("container runtime and trust mounts differ from the protected spawn contract".into()));
    }
    let paths = match &spawn.container {
        Some(c) => RuntimePaths::child_container_at(run_dir, &c.guest_trust_root, project_id, &c.guest_project_root),
        None => RuntimePaths::child_at(run_dir, project_id, &spawn.root),
    }
        .ok_or_else(|| BootError::NotSpawned("project id is not a safe path component".into()))?;
    let revoked = paths.revoked_marker();
    if revoked.exists() {
        return Err(BootError::Revoked(revoked));
    }
    let user_pubkey: [u8; 32] = hex_decode(&spawn.user_pubkey).ok_or_else(|| {
        BootError::Untrusted("spawn.json user_pubkey is not 32 bytes of hex".into())
    })?;
    check_pin(&paths, &user_pubkey)?;
    if spawn.container.is_some() && !paths.trust_root().join(clawft_kernel::overlay_runtime::USER_PIN_FILE).is_file() {
        return Err(BootError::Untrusted("container boot requires a parent-owned user.pub pin".into()));
    }
    let key_path = paths
        .project_key()
        .ok_or_else(|| BootError::Io("no project key path".into()))?;
    let key = load_key(&key_path)?;
    let cert_path = paths
        .project_cert()
        .ok_or_else(|| BootError::Io("no certificate path".into()))?;
    let cached = read_cached_cert(&cert_path, &key, project_id, &user_pubkey, now);

    let params = LinkParams {
        socket: spawn.container.as_ref().map_or_else(|| spawn.parent_socket.clone(), |c| c.guest_parent_socket.clone()),
        project_id: project_id.to_owned(),
        user_pubkey,
        user_key_id: spawn.user_key_id.clone(),
        own_socket: paths.socket(),
        host_socket: spawn.container.as_ref().map(|c| c.host_child_socket.clone()),
        container_id: spawn.container.as_ref().map(|c| c.container_id.clone()),
        container_engine: spawn.container.as_ref().map(|c| c.engine.clone()),
        root: spawn.root.clone(),
        timeout,
    };
    let (cert, session, parent_head, heartbeat_secs, degraded, spawn_nonce) =
        match register_retrying(&params, &key, cached.as_ref(), &spawn.nonce, now, retry).await {
            Ok(reg) => {
                if cached.as_ref() != Some(&reg.cert) {
                    write_cert(&cert_path, &reg.cert)?;
                }
                (
                    reg.cert,
                    Some(reg.session),
                    reg.parent_head,
                    reg.heartbeat_secs.max(1),
                    None,
                    None,
                )
            }
            Err(RegError::Refused { kind, message }) => {
                return Err(BootError::Refused { kind, message });
            }
            Err(RegError::Untrusted(m)) => return Err(BootError::Untrusted(m)),
            Err(RegError::Unavailable(why)) => match cached {
                Some(cert) => {
                    warn!(reason = %why, "user daemon unreachable at boot; continuing degraded on the cached certificate");
                    (
                        cert,
                        None,
                        None,
                        crate::mesh_local_registry::HEARTBEAT_SECS,
                        Some(why),
                        Some(spawn.nonce.clone()),
                    )
                }
                None => return Err(BootError::NeverRegistered(why)),
            },
        };
    let boot = ChildBoot {
        paths,
        host_root: spawn.root.clone(),
        container: spawn.container.as_ref().map(|c| clawft_rpc::mesh_local::ContainerRegistration {
            engine: c.engine.clone(), container_id: c.container_id.clone(),
            host_socket: c.host_child_socket.to_string_lossy().into_owned(),
        }),
        key,
        cert,
        parent_socket: spawn.container.as_ref().map_or_else(|| spawn.parent_socket.clone(), |c| c.guest_parent_socket.clone()),
        user_pubkey,
        user_key_id: spawn.user_key_id.clone(),
        session,
        parent_head,
        heartbeat_secs,
        degraded,
        spawn_nonce,
        project_token: spawn.project_token.clone(),
    };
    // Policy and overlay now, before any chain is opened: a failure refuses
    // the boot with the offending key named.
    clawft_kernel::overlay_runtime::prepare(&boot.paths)
        .map_err(|e| BootError::Untrusted(e.boot_message()))?;
    Ok(boot)
}

async fn register_retrying(
    p: &LinkParams,
    key: &SigningKey,
    cached: Option<&ProjectCert>,
    spawn_nonce: &str,
    now: u64,
    retry: Retry,
) -> Result<Registered, RegError> {
    let started = std::time::Instant::now();
    let mut delay = retry.initial;
    let mut attempt = 1;
    loop {
        let at = now + started.elapsed().as_secs();
        match register_once(p, key, cached, Some(spawn_nonce), at).await {
            Err(RegError::Refused { kind, message })
                if attempt < retry.attempts.max(1)
                    && started.elapsed() + delay < retry.budget
                    && TRANSIENT_KINDS.contains(&kind.as_str()) =>
            {
                warn!(%kind, %message, attempt, "user daemon cannot register this project yet; retrying");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(retry.max);
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// The `user.pub` pin, when the supervisor wrote one, must be the key
/// `spawn.json` names.
fn check_pin(paths: &RuntimePaths, user_pubkey: &[u8; 32]) -> Result<(), BootError> {
    let pin = paths
        .trust_root()
        .join(clawft_kernel::overlay_trust::USER_PIN_FILE);
    match std::fs::read_to_string(&pin) {
        Ok(t) if hex_decode::<32>(t.trim()).as_ref() == Some(user_pubkey) => Ok(()),
        Ok(_) => Err(BootError::Untrusted(format!(
            "{} does not match the user key in spawn.json",
            pin.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(BootError::Io(format!("{}: {e}", pin.display()))),
    }
}

fn load_key(path: &Path) -> Result<SigningKey, BootError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| BootError::Io(format!("{}: {e}", dir.display())))?;
    }
    ident::load_or_create_project_key(path).map_err(|e| BootError::Key(e.to_string()))
}

fn write_cert(path: &Path, cert: &ProjectCert) -> Result<(), BootError> {
    let text = serde_json::to_vec_pretty(cert).map_err(|e| BootError::Io(e.to_string()))?;
    ident::write_private_atomic(path, &text, false).map_err(|e| BootError::Io(e.to_string()))
}

/// The cached certificate, only if it verifies for this key and project.
fn read_cached_cert(
    path: &Path,
    key: &SigningKey,
    project_id: &str,
    user_pubkey: &[u8; 32],
    now: u64,
) -> Option<ProjectCert> {
    let text = std::fs::read_to_string(path).ok()?;
    let cert: ProjectCert = match serde_json::from_str(&text) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "cached certificate does not parse; ignoring it");
            return None;
        }
    };
    match verify_cert(&cert, key, project_id, user_pubkey, now) {
        Ok(()) => Some(cert),
        Err(e) => {
            warn!(path = %path.display(), error = %e, "cached certificate does not verify; ignoring it");
            None
        }
    }
}

/// A certificate is accepted only if the user key signed it, it is for this
/// project and this key, and it has not expired.
pub fn verify_cert(
    cert: &ProjectCert,
    key: &SigningKey,
    project_id: &str,
    user_pubkey: &[u8; 32],
    now: u64,
) -> Result<(), String> {
    let at = chrono::DateTime::from_timestamp(i64::try_from(now).unwrap_or(i64::MAX), 0)
        .ok_or("clock out of range")?;
    cert.verify(user_pubkey, at).map_err(|e| e.to_string())?;
    if cert.project_id != project_id {
        return Err(format!("certificate is for project {}", cert.project_id));
    }
    if cert.project_pubkey != hex_encode(&key.verifying_key().to_bytes()) {
        return Err("certificate names a different project key".into());
    }
    Ok(())
}

/// How to reach and recognise the user daemon.
#[derive(Debug, Clone)]
pub struct LinkParams {
    /// The user daemon's socket.
    pub socket: PathBuf,
    /// Project ULID.
    pub project_id: String,
    /// The trusted user public key.
    pub user_pubkey: [u8; 32],
    /// `key_id` of the user key (named in the proof of possession).
    pub user_key_id: String,
    /// This child's own socket (registered as its address).
    pub own_socket: PathBuf,
    /// Parent-selected host socket and inspected engine identity, when isolated.
    pub host_socket: Option<PathBuf>,
    pub container_id: Option<String>,
    pub container_engine: Option<String>,
    /// Canonical project root.
    pub root: PathBuf,
    /// Per-call deadline.
    pub timeout: Duration,
}

/// A verified registration.
#[derive(Debug, Clone)]
pub struct Registered {
    /// Session id.
    pub session: String,
    /// The certificate in force.
    pub cert: ProjectCert,
    /// User-chain head at registration.
    pub parent_head: Option<ParentHead>,
    /// Heartbeat interval.
    pub heartbeat_secs: u64,
}

/// Why a registration attempt did not produce a [`Registered`].
#[derive(Debug, Clone)]
pub enum RegError {
    /// Could not talk to the parent (or it is not a socket we may trust).
    Unavailable(String),
    /// The parent answered and refused.
    Refused {
        /// Daemon `error_kind`.
        kind: String,
        /// Daemon message.
        message: String,
    },
    /// The parent answered but its answer does not verify.
    Untrusted(String),
}

fn fresh_hex() -> String {
    let mut raw = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut raw);
    ident::hex(&raw)
}

fn link_err(e: LinkError) -> RegError {
    match e {
        LinkError::Unavailable(m) => RegError::Unavailable(m),
        LinkError::Refused { kind, message, .. } => RegError::Refused { kind, message },
    }
}

/// `mesh.challenge` then `mesh.register`, and check what comes back. Used at
/// boot and again by the running link on every re-register.
pub async fn register_once(
    p: &LinkParams,
    key: &SigningKey,
    cached: Option<&ProjectCert>,
    spawn_nonce: Option<&str>,
    now: u64,
) -> Result<Registered, RegError> {
    verify_parent_socket(&p.socket).map_err(RegError::Unavailable)?;
    let ch = call_async(
        &p.socket,
        METHOD_CHALLENGE,
        serde_json::to_value(ChallengeRequest {
            project_id: p.project_id.clone(),
        })
        .unwrap_or_default(),
        Some(&p.project_id),
        p.timeout,
    )
    .await
    .map_err(link_err)?;
    let ch: ChallengeReply = serde_json::from_value(ch)
        .map_err(|e| RegError::Untrusted(format!("malformed challenge: {e}")))?;
    let client_nonce = fresh_hex();
    let sig = ident::pop_sign(
        key,
        PopOp::Register,
        &p.user_key_id,
        &ch.nonce,
        &p.project_id,
    )
    .map_err(|e| RegError::Untrusted(format!("cannot sign the proof of possession: {e}")))?;
    let socket_str = p.own_socket.to_string_lossy().into_owned();
    let pid = std::process::id();
    let container = match (&p.host_socket, &p.container_id, &p.container_engine) {
        (Some(host), Some(id), Some(engine)) => Some(clawft_rpc::mesh_local::ContainerRegistration {
            engine: engine.clone(), container_id: id.clone(),
            host_socket: host.to_string_lossy().into_owned(),
        }),
        _ => None,
    };
    let bind_bytes = match &container {
        Some(c) => clawft_rpc::mesh_local::bind_container_bytes(&p.project_id, &ch.nonce, &client_nonce, &socket_str, pid, c),
        None => bind_signed_bytes(&p.project_id, &ch.nonce, &client_nonce, &socket_str, pid),
    };
    let bind = key.sign(&bind_bytes);
    let req = RegisterRequest {
        protocol: PROTOCOL_TAG.to_owned(),
        role: MeshRole::Project,
        project_id: p.project_id.clone(),
        project_pubkey: hex_encode(&key.verifying_key().to_bytes()),
        cert: cached.cloned(),
        addresses: vec![p.project_id.clone()],
        topic_prefixes: vec![format!("chain/{}/", p.project_id)],
        version: env!("CARGO_PKG_VERSION").to_owned(),
        build_sha: option_env!("WEFTOS_BUILD_SHA")
            .unwrap_or_default()
            .to_owned(),
        pid,
        socket: socket_str,
        container,
        bind_sig: hex_encode(&bind.to_bytes()),
        features: vec!["anchor".to_owned(), "subscribe".to_owned()],
        client_nonce: client_nonce.clone(),
        root_sha256: crate::project_cert_rpc::root_sha256(&p.root),
        spawn_nonce: spawn_nonce.map(str::to_owned),
        nonce_reply: NonceReply {
            nonce: ch.nonce.clone(),
            sig: hex_encode(&sig),
        },
    };
    let ack = call_async(
        &p.socket,
        METHOD_REGISTER,
        serde_json::to_value(req).unwrap_or_default(),
        Some(&p.project_id),
        p.timeout,
    )
    .await
    .map_err(link_err)?;
    let ack: RegisterAck = serde_json::from_value(ack)
        .map_err(|e| RegError::Untrusted(format!("malformed acknowledgement: {e}")))?;
    check_ack(p, key, &ack, &ch.nonce, &client_nonce, now)
}

fn check_ack(
    p: &LinkParams,
    key: &SigningKey,
    ack: &RegisterAck,
    nonce: &str,
    client_nonce: &str,
    now: u64,
) -> Result<Registered, RegError> {
    let bad = |m: &str| {
        Err(RegError::Untrusted(format!(
            "registration acknowledgement: {m}"
        )))
    };
    if !ack.ok {
        return bad("not ok");
    }
    let Some(sig) = ack.parent_sig.as_deref().and_then(hex_decode::<64>) else {
        return bad("no signature from the user key");
    };
    let vk = VerifyingKey::from_bytes(&p.user_pubkey)
        .map_err(|_| RegError::Untrusted("user key is not a valid key".into()))?;
    if vk
        .verify_strict(
            &ack_signed_bytes(&p.project_id, &ack.session, nonce, client_nonce),
            &Signature::from_bytes(&sig),
        )
        .is_err()
    {
        return bad(
            "signature does not verify under the user key (is something else listening on the parent socket?)",
        );
    }
    let Some(cert) = ack.cert.clone() else {
        return bad("carries no certificate");
    };
    if let Err(e) = verify_cert(&cert, key, &p.project_id, &p.user_pubkey, now) {
        return bad(&format!("certificate rejected: {e}"));
    }
    if cert.user_key_id != p.user_key_id || cert.user_key_id != key_id(&p.user_pubkey) {
        return bad("certificate names another user key");
    }
    Ok(Registered {
        session: ack.session.clone(),
        cert,
        parent_head: ack.parent_head.clone(),
        heartbeat_secs: ack.heartbeat_secs,
    })
}
