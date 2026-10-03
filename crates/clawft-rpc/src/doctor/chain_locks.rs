//! `chain.lock` holders and legacy-chain adoption evidence (ADR-103 R6).
//!
//! The question an operator must answer before `--adopt-legacy-chain` is
//! whether a lock-unaware daemon is still writing the legacy chain. A
//! daemon that predates `kernel.lock`/`chain.lock` holds neither, so the
//! only evidence is circumstantial: a recently modified chain, and running
//! kernel daemons that own no lock. Everything here is read-only.

use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use super::daemon::{ProcTable, daemon_kind};
use super::{Component, Finding, Severity};

/// A chain modified this recently is assumed live (same window as the
/// kernel's own adoption refusal, `chain_storage::LEGACY_ACTIVE_WINDOW`).
const ACTIVE_WINDOW: Duration = Duration::from_secs(120);

/// Pid recorded in a lock file (`kernel.lock` and `chain.lock` both carry it).
pub(super) fn recorded_pid(lock: &Path) -> Option<u32> {
    let t = std::fs::read_to_string(lock).ok()?;
    t.split_whitespace().next()?.parse().ok()
}

/// Whether the holder named in `lock` is running: `held`, `free`, or `unknown`.
fn lock_state(procs: &ProcTable, pid: Option<u32>) -> &'static str {
    match pid {
        Some(_) if !procs.ok => "unknown",
        Some(p) if procs.alive(p) => "held",
        Some(_) => "free",
        None => "unknown",
    }
}

/// Finding for the `chain.lock` beside `checkpoint`, if the file exists.
pub(super) fn chain_lock(procs: &ProcTable, checkpoint: &Path) -> Option<(Finding, Value)> {
    let lock = checkpoint.with_extension("lock");
    if !lock.is_file() {
        return None;
    }
    let pid = recorded_pid(&lock);
    let state = lock_state(procs, pid);
    let msg = match (state, pid) {
        ("held", Some(p)) => format!("{} held by pid {p}", lock.display()),
        ("free", Some(p)) => format!(
            "{} names pid {p}, which is not running (the lock is released on exit; harmless)",
            lock.display()
        ),
        (_, Some(p)) => format!("{} names pid {p} (liveness unknown)", lock.display()),
        _ => format!("{} present (holder pid not recorded)", lock.display()),
    };
    let f = Finding::new(
        Component::Runtime,
        format!("chain_lock:{}", lock.display()),
        Severity::Ok,
        msg,
    );
    Some((f, json!({ "path": lock, "pid": pid, "state": state })))
}

/// Seconds since `p` was last modified, if it exists.
fn age_secs(p: &Path) -> Option<u64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(SystemTime::now().duration_since(m).unwrap_or_default().as_secs())
}

/// Running kernel daemons that hold no `kernel.lock` in any known runtime
/// dir: the candidates for "old daemon that never takes the locks".
fn lock_unaware_daemons(procs: &ProcTable, lock_files: &[std::path::PathBuf]) -> Vec<u32> {
    let holders: Vec<u32> = lock_files.iter().filter_map(|l| recorded_pid(l)).collect();
    procs
        .rows
        .iter()
        .filter(|r| daemon_kind(&r.command).is_some() && !holders.contains(&r.pid))
        .map(|r| r.pid)
        .collect()
}

/// Adoption evidence for the legacy chain at `chain` (`~/.clawft/chain.json`),
/// given every `kernel.lock` file of the candidate runtime dirs.
pub(super) fn legacy_adoption(
    procs: &ProcTable,
    chain: &Path,
    adopted: bool,
    lock_files: &[std::path::PathBuf],
) -> (Vec<Finding>, Value) {
    let c = Component::Runtime;
    let mut out = Vec::new();
    let age = [chain.to_path_buf(), chain.with_extension("rvf")]
        .iter()
        .filter_map(|p| age_secs(p))
        .min();
    let recent = age.is_some_and(|a| a < ACTIVE_WINDOW.as_secs());
    let chain_held = lock_state(procs, recorded_pid(&chain.with_extension("lock"))) == "held";
    let candidates = if procs.ok { lock_unaware_daemons(procs, lock_files) } else { Vec::new() };
    let data = json!({
        "last_modified_secs_ago": age,
        "lock_unaware_daemon_pids": candidates,
    });
    if recent && !chain_held {
        out.push(
            Finding::new(c, format!("legacy_chain_active:{}", chain.display()), Severity::Warn, format!(
                "legacy chain {} was modified {}s ago and no kernel holds its chain.lock: a lock-unaware daemon is probably still writing it",
                chain.display(),
                age.unwrap_or_default()
            ))
            .remedy("stop the old daemon (weaver kernel stop) before `weaver kernel start --adopt-legacy-chain`; adopting under a live writer forks the chain"),
        );
    }
    if !adopted && !candidates.is_empty() {
        let list = candidates.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
        out.push(
            Finding::new(c, format!("legacy_chain_writers:{}", chain.display()), Severity::Warn, format!(
                "running kernel daemon(s) holding no kernel.lock: pid {list}; a daemon from before ADR-103 may own the legacy chain {}",
                chain.display()
            ))
            .remedy("check `weaver doctor --component daemon` for their version and cwd, stop them, then adopt with `weaver kernel start --adopt-legacy-chain` or `weaver migrate user-chain`"),
        );
    }
    (out, data)
}

/// Anchor records the user daemon ignored because a retired user key sealed
/// them and the chain does not vouch for them (it leaves
/// `<id>.anchor-ignored.txt` in the manifest store). Doctor holds no keys, so
/// the daemon's marker is the evidence.
pub(super) fn ignored_anchor_records(manifests_dir: &Path) -> Vec<Finding> {
    let Ok(rd) = std::fs::read_dir(manifests_dir) else { return Vec::new() };
    let mut ids: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".anchor-ignored.txt").map(str::to_owned))
        .collect();
    ids.sort();
    ids.into_iter()
        .map(|id| {
            Finding::new(
                Component::Runtime,
                format!("anchor_ignored:{id}"),
                Severity::Warn,
                format!("anchor record for project {id} ignored: retired-key seal not corroborated by the user chain"),
            )
            .remedy(format!(
                "the project anchors again from its next statement; if the user chain was lost or reset, check {id}.anchor.json in the manifest store"
            ))
        })
        .collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::doctor::env::{serial, test_env};
    use crate::doctor::runtime::check;

    fn legacy_dir(d: &Path) -> (crate::doctor::env::DoctorEnv, std::path::PathBuf) {
        let mut env = test_env(d);
        let rt = d.join("proj/.weftos/runtime");
        std::fs::create_dir_all(&rt).unwrap();
        env.runtime_dir = rt;
        let legacy = env.home.join(".clawft");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("chain.json"), "{}").unwrap();
        (env, legacy)
    }

    #[test]
    fn recent_legacy_chain_without_lock_warns_of_a_lock_unaware_writer() {
        let _s = serial();
        let d = tempfile::tempdir().unwrap();
        let (mut env, _legacy) = legacy_dir(d.path());
        env.ps_override = Some("777 /bin/weaver kernel start --foreground\n".into());
        let procs = ProcTable::load(&env);
        let (f, data) = check(&env, &procs, false, false);
        let active = f.iter().find(|x| x.id.starts_with("legacy_chain_active:")).expect("active finding");
        assert_eq!(active.severity, Severity::Warn);
        assert!(active.remedy.as_deref().unwrap_or("").contains("--adopt-legacy-chain"));
        let w = f.iter().find(|x| x.id.starts_with("legacy_chain_writers:")).expect("writers finding");
        assert!(w.message.contains("pid 777"), "{}", w.message);
        let e = data["dirs"].as_array().unwrap().iter().find(|e| e["legacy_chain"].is_object()).unwrap().clone();
        assert_eq!(e["legacy_chain"]["evidence"]["lock_unaware_daemon_pids"], json!([777]));
    }

    #[test]
    fn a_daemon_holding_its_kernel_lock_is_not_a_candidate_and_an_old_chain_is_quiet() {
        let _s = serial();
        let d = tempfile::tempdir().unwrap();
        let (mut env, legacy) = legacy_dir(d.path());
        std::fs::write(legacy.join("kernel.lock"), "888\n").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options().write(true).open(legacy.join("chain.json")).unwrap().set_modified(old).unwrap();
        env.ps_override = Some("888 /bin/weaver kernel start --foreground\n".into());
        let procs = ProcTable::load(&env);
        let (f, _) = check(&env, &procs, false, false);
        assert!(!f.iter().any(|x| x.id.starts_with("legacy_chain_active:")));
        assert!(!f.iter().any(|x| x.id.starts_with("legacy_chain_writers:")));
        assert!(f.iter().any(|x| x.id.starts_with("adoption:")));
    }

    #[test]
    fn chain_lock_reports_a_live_or_released_holder() {
        let _s = serial();
        let d = tempfile::tempdir().unwrap();
        let (mut env, legacy) = legacy_dir(d.path());
        std::fs::write(legacy.join("chain.lock"), "999\n").unwrap();
        env.ps_override = Some("999 weaver kernel start\n".into());
        let (f, data) = check(&env, &ProcTable::load(&env), false, false);
        let l = f.iter().find(|x| x.id.starts_with("chain_lock:")).expect("chain lock finding");
        assert!(l.message.contains("held by pid 999"), "{}", l.message);
        let e = data["dirs"].as_array().unwrap().iter().find(|e| e["legacy_chain"].is_object()).unwrap().clone();
        assert_eq!(e["legacy_chain"]["chain_lock"]["state"], "held");
        env.ps_override = Some(String::new());
        let (f, _) = check(&env, &ProcTable::load(&env), false, false);
        let l = f.iter().find(|x| x.id.starts_with("chain_lock:")).unwrap();
        assert!(l.message.contains("not running"), "{}", l.message);
    }

    #[test]
    fn an_ignored_retired_key_anchor_record_is_a_warn() {
        let d = tempfile::tempdir().unwrap();
        assert!(ignored_anchor_records(d.path()).is_empty());
        std::fs::write(d.path().join("01JB8Z3Q0V6X9KQ4M2N7T5R1WA.anchor-ignored.txt"), "x").unwrap();
        std::fs::write(d.path().join("01JB8Z3Q0V6X9KQ4M2N7T5R1WA.anchor.json"), "{}").unwrap();
        let f = ignored_anchor_records(d.path());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Warn);
        assert!(f[0].message.contains("ignored: retired-key seal not corroborated"), "{}", f[0].message);
    }
}
