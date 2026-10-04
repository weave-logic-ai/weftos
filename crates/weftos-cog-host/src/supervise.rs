//! The supervisor: keeps every `enabled` cog running, restarts it on exit with capped backoff, and
//! reports status. No cap on how many run at once — that is governed by the hardware, not policy.
//!
//! With a licence gate set ([`Supervisor::set_licence_gate`], ADR-106 phase 3) every spawn first
//! hashes the binary file and asks [`crate::licence::check_start`]; a refusal is a failed spawn
//! (backoff, retried) carrying the gate's code. Every tick, a running cog whose binary hash is
//! revoked is stopped.

use crate::licence::{check_start, hashes};
use crate::{load_records, save_record, CogRecord};
use clawft_kernel::licence::CognitumRunGate;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_BACKOFF: Duration = Duration::from_secs(30);

struct Running {
    child: Child,
    started: Instant,
    /// BLAKE3 of the started binary (only hashed when a licence gate is set).
    blake3: Option<String>,
    /// The grant that covered a licensed start.
    grant_id: Option<String>,
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
}

impl Supervisor {
    pub fn new(root: PathBuf) -> Self {
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
        let errlog = log.try_clone().map_err(|e| format!("clone log: {e}"))?;
        let (mut blake3, mut grant_id) = (None, None);
        if let Some(gate) = &self.gate {
            // Hash the file that is about to run; the gate alone decides.
            let bytes = std::fs::read(&bin).map_err(|e| format!("read {}: {e}", bin.display()))?;
            match check_start(gate.as_ref(), rec, &bytes) {
                Ok(lic) => {
                    self.refusals.remove(id);
                    if let Some(l) = &lic {
                        eprintln!("[cog-host] licence permit {id} {}: grant {} approval {}", rec.version, l.permit.grant_id, l.permit.approval_id);
                    }
                    grant_id = lic.map(|l| l.permit.grant_id);
                    blake3 = Some(hashes(&bytes).1);
                }
                Err(r) => {
                    eprintln!("[cog-host] {id}: {r}");
                    self.refusals.insert(id.to_string(), r.code);
                    return Err(r.reason);
                }
            }
        }
        let child = Command::new(&bin)
            .args(&rec.args)
            .current_dir(rec.dir(&self.root))
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(errlog))
            .spawn()
            .map_err(|e| format!("spawn {id}: {e}"))?;
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
