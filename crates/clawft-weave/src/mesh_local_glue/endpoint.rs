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

/// [`build_endpoint`] for async callers: the blocking record wait (up to
/// [`RECORD_WAIT`]) and the key file I/O run on the blocking pool, never on
/// a runtime worker.
pub async fn build_endpoint_async(
    cfg: &MeshConfig,
    home: &Path,
    build_sha: &str,
) -> Result<Option<ServiceEndpoint>, String> {
    let (cfg, home, sha) = (cfg.clone(), home.to_path_buf(), build_sha.to_owned());
    tokio::task::spawn_blocking(move || build_endpoint(&cfg, &home, &sha))
        .await
        .map_err(|e| format!("endpoint probe task failed: {e}"))?
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
    // A record that is missing, empty or not yet complete JSON inside the
    // wait window is a writer mid-publish: retry until the window expires.
    let (owner, record) = loop {
        let retry = started.elapsed() < RECORD_WAIT;
        let mut file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && retry => {
                std::thread::sleep(RECORD_POLL);
                continue;
            }
            Err(e) => return Err(unreadable(e)),
        };
        let owner = file.metadata().map_err(unreadable)?.uid();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(unreadable)?;
        match serde_json::from_slice::<ServiceRecord>(&bytes) {
            Ok(r) => break (owner, r),
            Err(_) if retry => std::thread::sleep(RECORD_POLL),
            Err(e) => return Err(unreadable(std::io::Error::new(std::io::ErrorKind::InvalidData, e))),
        }
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_mesh_local::proto::VersionRange;

    fn record(service_uid: u32) -> ServiceRecord {
        ServiceRecord {
            node_id: "n".repeat(32),
            machine_pubkey: [3u8; 32],
            service_uid,
            proto: VersionRange { min: 1, max: 1, sha: None },
            build_sha: "t".into(),
            started_at: 1,
        }
    }

    fn me() -> u32 {
        nix::unistd::geteuid().as_raw()
    }

    fn write(dir: &Path, rec: &ServiceRecord) -> PathBuf {
        let p = dir.join("service.json");
        std::fs::write(&p, serde_json::to_vec(rec).unwrap()).unwrap();
        p
    }

    #[test]
    fn a_record_naming_the_socket_owner_is_trusted() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), &record(me()));
        assert_eq!(read_record(&p, me()).unwrap().service_uid, me());
    }

    #[test]
    fn a_record_whose_service_uid_is_not_the_socket_owner_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), &record(me() + 1));
        let e = read_record(&p, me()).unwrap_err();
        assert!(e.contains("names service uid") && e.contains("refusing"), "{e}");
    }

    #[test]
    fn a_record_file_owned_by_neither_root_nor_the_socket_owner_is_refused() {
        if me() == 0 {
            return; // root-owned files are always acceptable
        }
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), &record(me() + 1));
        // The file is ours; the socket belongs to someone else.
        let e = read_record(&p, me() + 1).unwrap_err();
        assert!(e.contains("owned by uid"), "{e}");
    }

    #[test]
    fn an_empty_or_truncated_record_is_retried_until_complete() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("service.json");
        std::fs::write(&p, b"").unwrap();
        let full = serde_json::to_vec(&record(me())).unwrap();
        let (pp, half) = (p.clone(), full[..full.len() / 2].to_vec());
        let w = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            std::fs::write(&pp, half).unwrap();
            std::thread::sleep(Duration::from_millis(250));
            std::fs::write(&pp, full).unwrap();
        });
        assert_eq!(read_record(&p, me()).unwrap().service_uid, me());
        w.join().unwrap();
    }

    #[test]
    fn a_record_that_stays_unparseable_fails_after_the_window() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("service.json");
        std::fs::write(&p, b"{").unwrap();
        let t = Instant::now();
        let e = read_record(&p, me()).unwrap_err();
        assert!(e.contains("unreadable") && t.elapsed() >= RECORD_WAIT, "{e}");
    }

    #[test]
    fn a_missing_record_is_waited_for_without_blocking_the_runtime() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("service.json");
        let rec = record(me());
        let late = p.clone();
        let w = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            write(late.parent().unwrap(), &rec);
        });
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        // On a current-thread runtime a blocking wait on the runtime thread
        // would starve the ticker; the blocking pool must not.
        let ticks = rt.block_on(async {
            let ticker = tokio::spawn(async {
                let mut n = 0u32;
                for _ in 0..5 {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    n += 1;
                }
                n
            });
            let r = tokio::task::spawn_blocking(move || read_record(&p, me())).await.unwrap();
            assert!(r.is_ok(), "{r:?}");
            ticker.await.unwrap()
        });
        w.join().unwrap();
        assert_eq!(ticks, 5);
    }
}
