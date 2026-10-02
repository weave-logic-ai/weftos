//! Building the [`ServiceEndpoint`] for the real service (split out of
//! [`super`]): the socket probe, the `service.json` record clients pin, and
//! the user key and registration.

use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clawft_kernel::mesh_mode;
use clawft_mesh_local::ClientConfig;
use clawft_mesh_local::client::RegisterParams;
use clawft_mesh_local::proto::ServiceRecord;
use clawft_types::config::MeshConfig;

use super::{DEFAULT_STATE_DIR, STATE_DIR_ENV, ServiceEndpoint};

/// How long a socket without its record is given for the record to appear
/// (a service from before the record-first start order, or one mid-start).
const RECORD_WAIT: Duration = Duration::from_secs(2);
const RECORD_POLL: Duration = Duration::from_millis(100);

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
///
/// `service.json` is read from beside the socket: the state dir is 0700 and
/// owned by the service account, so the service writes a copy there for its
/// clients (the same copy `weaver mesh` verbs and the doctor read).
pub fn build_endpoint(
    cfg: &MeshConfig,
    home: &Path,
    build_sha: &str,
) -> Result<Option<ServiceEndpoint>, String> {
    let sock = mesh_mode::service_socket(cfg);
    let record_dir = sock.parent().map_or_else(state_dir, Path::to_path_buf);
    build_endpoint_in(cfg, home, build_sha, &record_dir)
}

/// [`build_endpoint`] with an explicit directory holding `service.json`.
pub fn build_endpoint_in(
    cfg: &MeshConfig,
    home: &Path,
    build_sha: &str,
    state_dir: &Path,
) -> Result<Option<ServiceEndpoint>, String> {
    let sock = mesh_mode::service_socket(cfg);
    let sock_owner = match std::fs::symlink_metadata(&sock) {
        Ok(md) => md.uid(),
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
    };
    let record = read_record(&state_dir.join("service.json"), sock_owner)?;
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
        // No other local tenant may send to this user unless configured;
        // the service refuses cross-tenant sends by default (P3 S).
        accept_from: Vec::new(),
    };
    Ok(Some(ServiceEndpoint { client, user_key, register }))
}

/// Read `service.json` and decide whether to trust it. The owner check uses
/// the opened file (no stat-then-read race) and is anchored on the socket's
/// owner, not on the uid the file itself names: the record must belong to
/// root or the account that owns the socket, and must name that account.
///
/// A record still missing while the socket exists is waited for briefly
/// (boot-time only; blocking is bounded by [`RECORD_WAIT`]).
fn read_record(path: &Path, sock_owner: u32) -> Result<ServiceRecord, String> {
    let unreadable = |e: std::io::Error| {
        format!("{} is unreadable ({e}); without it the service's machine key cannot be checked", path.display())
    };
    let started = Instant::now();
    let mut file = loop {
        match std::fs::File::open(path) {
            Ok(f) => break f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && started.elapsed() < RECORD_WAIT => {
                std::thread::sleep(RECORD_POLL);
            }
            Err(e) => return Err(unreadable(e)),
        }
    };
    let owner = file.metadata().map_err(unreadable)?.uid();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(unreadable)?;
    let record: ServiceRecord = serde_json::from_slice(&bytes)
        .map_err(|e| unreadable(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
    if owner != 0 && owner != sock_owner {
        return Err(format!(
            "{} is owned by uid {owner}, not root or the socket's owner (uid {sock_owner}); refusing to trust it",
            path.display()
        ));
    }
    if sock_owner != 0 && record.service_uid != sock_owner {
        return Err(format!(
            "{} names service uid {} but the socket is owned by uid {sock_owner}; refusing to trust it",
            path.display(),
            record.service_uid
        ));
    }
    Ok(record)
}
