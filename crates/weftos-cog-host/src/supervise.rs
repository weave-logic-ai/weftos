//! The supervisor: keeps every `enabled` cog running, restarts it on exit with capped backoff, and
//! reports status. No cap on how many run at once — that is governed by the hardware, not policy.
//!
//! With a licence gate set ([`Supervisor::set_licence_gate`], ADR-106 phase 3) every spawn first
//! hashes the binary file and asks [`crate::licence::check_start`]; a refusal is a failed spawn
//! (backoff, retried) carrying the gate's code. Every tick, a running cog whose binary hash is
//! revoked is stopped.
//!
//! The bytes that were hashed are the bytes that run: a start the gate permitted as a licensed one
//! executes exactly the hashed bytes, not whatever the path holds a moment later. On Linux that is
//! an in-memory sealed `memfd` (no file, nothing to swap or to wear the SD card); elsewhere, and for
//! `#!` scripts, a content-addressed `0700` copy `<root>/.run/<blake3>` (the directory is cleared at
//! host start). This closes the check-then-exec swap by another user or a later write. It is not a
//! defence against a process with the host's own uid, which can edit the licence state, the cog
//! records and the host itself; unclaimed (non-Cognitum) starts exec by path. See ADR-106, "Limits".

use crate::licence::{check_start_hashed, hashes};
use crate::{load_records, save_record, CogRecord};
use clawft_kernel::licence::CognitumRunGate;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Private executables of licence-checked starts (a cog id never starts with a dot).
const RUN_DIR: &str = ".run";

struct Running {
    child: Child,
    started: Instant,
    /// BLAKE3 of the started binary (only hashed when a licence gate is set).
    blake3: Option<String>,
    /// The grant that covered a licensed start.
    grant_id: Option<String>,
}

/// Largest binary the host will read to hash and run (the install body cap).
const MAX_BIN_BYTES: u64 = 32 * 1024 * 1024;

/// Read a cog binary, refusing one over [`MAX_BIN_BYTES`].
fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    f.take(MAX_BIN_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("read {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_BIN_BYTES {
        return Err(format!("{} is larger than {MAX_BIN_BYTES} bytes", path.display()));
    }
    Ok(bytes)
}

/// A sealed in-memory file holding `bytes`, and the `/proc/self/fd` path that executes it. Not for
/// `#!` scripts (the interpreter would open a closed descriptor). `None` if the kernel lacks it or
/// sealing fails. A cog run this way sees `current_exe()` as `/memfd:weft-cog-<id> (deleted)`, and
/// each licensed instance holds its binary in RAM (it is released when the cog exits).
#[cfg(target_os = "linux")]
fn memfd_exec(id: &str, bytes: &[u8]) -> Option<(PathBuf, std::fs::File)> {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    if bytes.starts_with(b"#!") {
        return None;
    }
    let name = std::ffi::CString::new(format!("weft-cog-{}", id.chars().filter(|c| c.is_ascii_graphic()).take(200).collect::<String>())).ok()?;
    // MFD_EXEC (kernel 6.3+) asks for an executable memfd explicitly, which `vm.memfd_noexec=1`
    // would otherwise make non-executable; older kernels answer EINVAL, so retry without it.
    const MFD_EXEC: libc::c_uint = 0x0010;
    let base = libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING;
    // SAFETY: plain syscalls; the fd is owned by the File built from it.
    let mut fd = unsafe { libc::memfd_create(name.as_ptr(), base | MFD_EXEC) };
    if fd < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL) {
        fd = unsafe { libc::memfd_create(name.as_ptr(), base) };
    }
    if fd < 0 {
        return None;
    }
    // SAFETY: `fd` is a fresh descriptor nothing else owns.
    let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
    f.write_all(bytes).ok()?;
    let seals = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    // Never exec an unsealed memfd: if sealing fails, the caller falls back to the file copy.
    // SAFETY: plain fcntl on our own fd.
    if unsafe { libc::fcntl(f.as_raw_fd(), libc::F_ADD_SEALS, seals) } != 0 {
        return None;
    }
    Some((PathBuf::from(format!("/proc/self/fd/{}", f.as_raw_fd())), f))
}

/// The `0700` file `<root>/.run/<blake3>` holding exactly `bytes`, written once (temp file then
/// rename, no fsync: it is rebuilt at host start anyway) and reused while it still hashes right.
fn content_copy(root: &Path, blake3: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = root.join(RUN_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("chmod {}: {e}", dir.display()))?;
    }
    let path = dir.join(blake3);
    if std::fs::read(&path).is_ok_and(|have| hashes(&have).1 == blake3) {
        return Ok(path);
    }
    let tmp = dir.join(format!(".tmp-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o700);
    }
    let res = opts.open(&tmp).and_then(|mut f| f.write_all(bytes)).and_then(|_| std::fs::rename(&tmp, &path));
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("write {}: {e}", path.display()));
    }
    Ok(path)
}

/// Spawn `exe`, retrying a few times on ETXTBSY (another thread forked while a just-written file
/// was open).
fn spawn_retry(cmd: &mut Command) -> std::io::Result<Child> {
    let mut spawned = cmd.spawn();
    for _ in 0..5 {
        match &spawned {
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(Duration::from_millis(20));
                spawned = cmd.spawn();
            }
            _ => break,
        }
    }
    spawned
}

/// One cog's live status, as the API serializes it.
#[derive(Clone, Debug, Default, Serialize)]
pub struct CogStatus {
    pub id: String,
    pub version: String,
    pub source: String,
    pub enabled: bool,
    pub running: bool,
    pub pid: Option<u32>,
    pub restarts: u32,
    pub rss_kb: Option<u64>,
    pub uptime_s: Option<u64>,
    pub last_exit: Option<String>,
    pub signed: bool,
    /// The licence run gate's code when it refused the last start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub licence_refusal: Option<String>,
    /// The checkout grant that covered the running instance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub licence_grant: Option<String>,
}

pub struct Supervisor {
    pub root: PathBuf,
    /// Edge-node roster (COG-010), checked into via `/fleet/heartbeat`.
    pub fleet: crate::fleet::Fleet,
    records: HashMap<String, CogRecord>,
    running: HashMap<String, Running>,
    restarts: HashMap<String, u32>,
    last_exit: HashMap<String, String>,
    backoff_until: HashMap<String, Instant>,
    gate: Option<Arc<dyn CognitumRunGate>>,
    refusals: HashMap<String, &'static str>,
    /// (len, mtime) -> (sha256, blake3) of each cog binary last hashed, so a refused cog is not
    /// re-read at every backoff. A same-length swap inside the mtime granularity yields at worst a
    /// stale refusal (the gate is asked again with the old hashes), never a permit: a permit always
    /// follows a fresh read and hash of the bytes that then run.
    hash_cache: HashMap<PathBuf, (u64, SystemTime, String, String)>,
}

impl Supervisor {
    pub fn new(root: PathBuf) -> Self {
        // Copies left by an earlier host process; any instance still running keeps its inode.
        let _ = std::fs::remove_dir_all(root.join(RUN_DIR));
        let records = load_records(&root).into_iter().map(|r| (r.id.clone(), r)).collect();
        Supervisor {
            root,
            fleet: crate::fleet::Fleet::default(),
            records,
            running: HashMap::new(),
            restarts: HashMap::new(),
            last_exit: HashMap::new(),
            backoff_until: HashMap::new(),
            gate: None,
            refusals: HashMap::new(),
            hash_cache: HashMap::new(),
        }
    }

    /// Ask `gate` before every spawn (ADR-106 start-time licence check).
    pub fn set_licence_gate(&mut self, gate: Arc<dyn CognitumRunGate>) {
        self.gate = Some(gate);
    }

    /// Re-read records from disk (after an install/add outside the process).
    pub fn reload(&mut self) {
        self.records = load_records(&self.root).into_iter().map(|r| (r.id.clone(), r)).collect();
    }

    pub fn ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.records.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    /// Mark a cog enabled (desired-running), persist it, and spawn immediately.
    pub fn start(&mut self, id: &str) -> Result<(), String> {
        let rec = self.records.get_mut(id).ok_or_else(|| format!("no such cog '{id}'"))?;
        rec.enabled = true;
        let rec = rec.clone();
        save_record(&self.root, &rec).map_err(|e| e.to_string())?;
        self.records.insert(id.to_string(), rec);
        self.backoff_until.remove(id);
        if !self.running.contains_key(id) {
            self.spawn(id)?;
        }
        Ok(())
    }

    /// Mark a cog disabled, persist, and stop it now.
    pub fn stop(&mut self, id: &str) -> Result<(), String> {
        if let Some(rec) = self.records.get_mut(id) {
            rec.enabled = false;
            let rec = rec.clone();
            save_record(&self.root, &rec).map_err(|e| e.to_string())?;
            self.records.insert(id.to_string(), rec);
        }
        if let Some(mut r) = self.running.remove(id) {
            let _ = r.child.kill();
            let _ = r.child.wait();
            self.last_exit.insert(id.to_string(), format!("stopped at {}", unix_now()));
        }
        Ok(())
    }

    fn spawn(&mut self, id: &str) -> Result<(), String> {
        let rec = self.records.get(id).ok_or_else(|| format!("no such cog '{id}'"))?;
        let bin = rec.binary_path(&self.root);
        if !bin.exists() {
            return Err(format!("binary missing: {}", bin.display()));
        }
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(rec.dir(&self.root).join("host.log"))
            .map_err(|e| format!("open log: {e}"))?;
        let (mut blake3, mut grant_id, mut licensed_bytes) = (None, None, None);
        if let Some(gate) = self.gate.clone() {
            // A refusal for a file whose size and mtime are unchanged needs no new read.
            let meta = std::fs::metadata(&bin).map_err(|e| format!("stat {}: {e}", bin.display()))?;
            let key = (meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH));
            if let Some((l, m, sha, b3)) = self.hash_cache.get(&bin)
                && (*l, *m) == key
                && let Err(r) = check_start_hashed(gate.as_ref(), rec, sha, b3)
            {
                eprintln!("[cog-host] {id}: {r}");
                self.refusals.insert(id.to_string(), r.code);
                return Err(r.reason);
            }
            // Hash the file that is about to run; the gate alone decides.
            let bytes = read_capped(&bin)?;
            let (sha, b3) = hashes(&bytes);
            self.hash_cache.insert(bin.clone(), (key.0, key.1, sha.clone(), b3.clone()));
            match check_start_hashed(gate.as_ref(), rec, &sha, &b3) {
                Ok(lic) => {
                    self.refusals.remove(id);
                    if let Some(l) = &lic {
                        eprintln!("[cog-host] licence permit {id} {}: grant {} approval {}", rec.version, l.permit.grant_id, l.permit.approval_id);
                        licensed_bytes = Some(bytes);
                    }
                    grant_id = lic.map(|l| l.permit.grant_id);
                    blake3 = Some(b3);
                }
                Err(r) => {
                    eprintln!("[cog-host] {id}: {r}");
                    self.refusals.insert(id.to_string(), r.code);
                    return Err(r.reason);
                }
            }
        }
        let make = |exe: &Path| -> Result<Command, String> {
            let (out, err) = (log.try_clone().map_err(|e| format!("clone log: {e}"))?, log.try_clone().map_err(|e| format!("clone log: {e}"))?);
            let mut cmd = Command::new(exe);
            cmd.args(&rec.args).current_dir(rec.dir(&self.root)).stdin(Stdio::null()).stdout(Stdio::from(out)).stderr(Stdio::from(err));
            Ok(cmd)
        };
        let spawned = match (&licensed_bytes, &blake3) {
            // Licensed start: run exactly the bytes that were checked.
            (Some(bytes), Some(b3)) => {
                #[cfg(target_os = "linux")]
                let via_memfd = match memfd_exec(id, bytes) {
                    Some((exe, _keep)) => {
                        use std::os::unix::process::CommandExt;
                        let mut cmd = make(&exe)?;
                        cmd.arg0(&bin);
                        spawn_retry(&mut cmd).ok()
                    }
                    None => None,
                };
                #[cfg(not(target_os = "linux"))]
                let via_memfd: Option<Child> = None;
                match via_memfd {
                    Some(c) => Ok(c),
                    None => {
                        let exe = content_copy(&self.root, b3, bytes)?;
                        spawn_retry(&mut make(&exe)?)
                    }
                }
            }
            _ => spawn_retry(&mut make(&bin)?),
        };
        let child = spawned.map_err(|e| format!("spawn {id}: {e}"))?;
        self.running.insert(id.to_string(), Running { child, started: Instant::now(), blake3, grant_id });
        Ok(())
    }

    /// Stop every running cog whose binary hash is now revoked (the start-time check would refuse
    /// it). A lapsed grant does not stop a running cog; only its next start is refused.
    fn sweep_revoked(&mut self) {
        let Some(gate) = &self.gate else { return };
        let revoked: Vec<String> = self
            .running
            .iter()
            .filter(|(_, r)| r.blake3.as_deref().is_some_and(|b| gate.revoked(b)))
            .map(|(id, _)| id.clone())
            .collect();
        for id in revoked {
            if let Some(mut r) = self.running.remove(&id) {
                let _ = r.child.kill();
                let _ = r.child.wait();
                eprintln!("[cog-host] {id}: stopped, its binary hash is revoked");
                self.last_exit.insert(id.clone(), format!("stopped: licence run gate [hash_revoked] at {}", unix_now()));
                self.refusals.insert(id.clone(), "hash_revoked");
                let n = self.restarts.entry(id.clone()).or_insert(0);
                *n += 1;
                self.backoff_until.insert(id, Instant::now() + backoff(*n));
            }
        }
    }

    /// One supervision step: reap exits, restart enabled cogs past their backoff. Call ~1/s.
    pub fn tick(&mut self) {
        self.sweep_revoked();
        // reap
        let ids: Vec<String> = self.running.keys().cloned().collect();
        for id in ids {
            let exited = match self.running.get_mut(&id).unwrap().child.try_wait() {
                Ok(Some(status)) => Some(format!("{status} at {}", unix_now())),
                Ok(None) => None,
                Err(e) => Some(format!("wait error: {e}")),
            };
            if let Some(reason) = exited {
                self.running.remove(&id);
                self.last_exit.insert(id.clone(), reason);
                let enabled = self.records.get(&id).map(|r| r.enabled).unwrap_or(false);
                if enabled {
                    let n = self.restarts.entry(id.clone()).or_insert(0);
                    *n += 1;
                    let delay = backoff(*n);
                    self.backoff_until.insert(id.clone(), Instant::now() + delay);
                }
            }
        }
        // (re)start enabled cogs that aren't running and are past backoff
        let to_start: Vec<String> = self
            .records
            .values()
            .filter(|r| r.enabled && !self.running.contains_key(&r.id))
            .filter(|r| self.backoff_until.get(&r.id).map(|t| Instant::now() >= *t).unwrap_or(true))
            .map(|r| r.id.clone())
            .collect();
        for id in to_start {
            if let Err(e) = self.spawn(&id) {
                self.last_exit.insert(id.clone(), format!("spawn failed: {e}"));
                let n = self.restarts.entry(id.clone()).or_insert(0);
                *n += 1;
                self.backoff_until.insert(id, Instant::now() + backoff(*n));
            }
        }
    }

    pub fn status(&self) -> Vec<CogStatus> {
        let mut out: Vec<CogStatus> = self
            .records
            .values()
            .map(|r| {
                let run = self.running.get(&r.id);
                let pid = run.map(|x| x.child.id());
                CogStatus {
                    id: r.id.clone(),
                    version: r.version.clone(),
                    source: format!("{:?}", r.source).to_lowercase(),
                    enabled: r.enabled,
                    running: run.is_some(),
                    pid,
                    restarts: self.restarts.get(&r.id).copied().unwrap_or(0),
                    rss_kb: pid.and_then(rss_kb),
                    uptime_s: run.map(|x| x.started.elapsed().as_secs()),
                    last_exit: self.last_exit.get(&r.id).cloned(),
                    signed: r.signed,
                    licence_refusal: self.refusals.get(&r.id).map(|c| c.to_string()),
                    licence_grant: run.and_then(|x| x.grant_id.clone()),
                }
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Stop everything (on shutdown). Leaves `enabled` flags on disk as they were.
    pub fn stop_all_running(&mut self) {
        for (_id, mut r) in self.running.drain() {
            let _ = r.child.kill();
            let _ = r.child.wait();
        }
    }
}

fn backoff(restarts: u32) -> Duration {
    let secs = 1u64.checked_shl(restarts.min(5)).unwrap_or(32);
    Duration::from_secs(secs).min(MAX_BACKOFF)
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Resident set size in KiB from `/proc/<pid>/statm` (Linux). None off-Linux or if unreadable.
fn rss_kb(pid: u32) -> Option<u64> {
    let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(resident_pages * 4) // 4 KiB pages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{save_record, Source};
    use std::io::Write;
    use std::path::Path;

    #[test]
    fn content_copy_is_0700_and_reused() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"\x7fELF-not-really";
        let b3 = hashes(bytes).1;
        let p = content_copy(root.path(), &b3, bytes).unwrap();
        assert_eq!(p, root.path().join(RUN_DIR).join(&b3));
        assert_eq!(std::fs::read(&p).unwrap(), bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o700);
            assert_eq!(std::fs::metadata(root.path().join(RUN_DIR)).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let ino = |p: &Path| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                std::fs::metadata(p).unwrap().ino()
            }
            #[cfg(not(unix))]
            {
                let _ = p;
                0u64
            }
        };
        let before = ino(&p);
        assert_eq!(content_copy(root.path(), &b3, bytes).unwrap(), p);
        assert_eq!(ino(&p), before, "an intact copy is reused, not rewritten");
        // A copy that no longer hashes right is replaced.
        std::fs::write(&p, b"tampered").unwrap();
        content_copy(root.path(), &b3, bytes).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), bytes);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn memfd_exec_is_sealed_and_runs_the_bytes() {
        use std::os::fd::AsRawFd;
        let Ok(elf) = std::fs::read("/bin/true") else { return };
        assert!(!elf.starts_with(b"#!"));
        let (path, f) = memfd_exec("seal-test", &elf).expect("memfd_exec");
        // SAFETY: plain fcntl on a descriptor this test owns.
        let seals = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GET_SEALS) };
        let full = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
        assert_eq!(seals & full, full, "all four seals");
        // The sealed file cannot be written any more.
        assert!(std::io::Write::write_all(&mut &f, b"x").is_err());
        let st = Command::new(&path).status().expect("exec through /proc/self/fd");
        assert!(st.success());
        // Scripts are not memfd-run (the interpreter would open a closed fd).
        assert!(memfd_exec("script", b"#!/bin/sh\n").is_none());
    }

    fn dummy_cog(root: &Path, id: &str, body: &str) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join(format!("cog-{id}-arm"));
        let mut f = std::fs::File::create(&bin).unwrap();
        writeln!(f, "#!/bin/sh\n{body}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        save_record(
            root,
            &CogRecord {
                id: id.into(),
                version: "0".into(),
                source: Source::Local,
                enabled: false,
                binary: format!("cog-{id}-arm"),
                args: vec![],
                signed: false,
            },
        )
        .unwrap();
    }

    #[test]
    fn backoff_is_capped() {
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(3), Duration::from_secs(8));
        assert_eq!(backoff(100), MAX_BACKOFF);
    }

    #[test]
    #[cfg(unix)]
    fn starts_supervises_and_stops_a_cog() {
        let root = std::env::temp_dir().join(format!("sup-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        dummy_cog(&root, "sleeper", "sleep 30");
        let mut s = Supervisor::new(root.clone());
        assert_eq!(s.running_count(), 0);

        s.start("sleeper").unwrap();
        assert_eq!(s.running_count(), 1);
        let st = &s.status()[0];
        assert!(st.running && st.enabled && st.pid.is_some());

        s.stop("sleeper").unwrap();
        assert_eq!(s.running_count(), 0);
        assert!(!s.status()[0].running && !s.status()[0].enabled);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn restarts_an_enabled_cog_that_exits() {
        let root = std::env::temp_dir().join(format!("sup-restart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        dummy_cog(&root, "flappy", "exit 0"); // exits immediately
        let mut s = Supervisor::new(root.clone());
        s.start("flappy").unwrap(); // spawns once
        std::thread::sleep(Duration::from_millis(200));
        s.tick(); // reap the exit -> restart counter increments, backoff set
        assert!(s.status()[0].restarts >= 1, "expected a restart to be recorded");
        assert!(s.status()[0].last_exit.is_some());
        s.stop("flappy").unwrap();
        s.stop_all_running();
        let _ = std::fs::remove_dir_all(&root);
    }
}
