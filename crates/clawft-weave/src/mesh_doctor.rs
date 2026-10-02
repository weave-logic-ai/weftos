//! `weaver doctor` checks for the machine mesh service (ADR-103 Phase 3,
//! package H). Findings use the `runtime` component with ids `mesh.*`.
//!
//! [`findings`] is pure over a [`MeshProbe`]; [`gather`] fills the probe from
//! the running system and only reads (a socket status call, `stat`, `lsof`).
//! Nothing is repaired and nothing signals a process.

use std::path::PathBuf;

use clawft_mesh_local::proto::{PROTO_MAX, PROTO_MIN};
use clawft_rpc::doctor::{Component, Finding, Severity};
use serde_json::Value;

use crate::service_units_system::{SERVICE_EXE, STATE_DIR};

/// Mode and owner of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    pub mode: u32,
    pub uid: u32,
}

/// Everything the checks read, injectable.
#[derive(Debug, Clone, Default)]
pub struct MeshProbe {
    /// `service.json` loaded (None: no service has run here).
    pub record_pubkey: Option<[u8; 32]>,
    pub record_present: bool,
    pub service_uid: Option<u32>,
    /// `Status` reply, or why it failed.
    pub status: Option<Value>,
    pub reach_error: Option<String>,
    /// `JournalVerify` reply (admin only; None when not asked or refused).
    pub journal: Option<Value>,
    /// Pinned key from `~/.weftos/mesh/machine.pub`.
    pub pinned: Option<[u8; 32]>,
    /// `<state>/node.key` stat, when this user can see it.
    pub box_key: Option<FileStat>,
    /// `~/.weftos/run/node.key` exists.
    pub user_node_key: bool,
    /// Distinct pids listening on the mesh port (None: could not tell).
    pub listener_pids: Option<Vec<u32>>,
    /// Executable of the running service, when known.
    pub service_exe: Option<PathBuf>,
}

fn f(id: &str, sev: Severity, msg: impl Into<String>) -> Finding {
    Finding::new(Component::Runtime, format!("mesh.{id}"), sev, msg)
}

fn user_writable(p: &std::path::Path, home: &std::path::Path) -> bool {
    p.starts_with(home) || p.starts_with("/tmp") || p.starts_with("/private/tmp")
}

/// Turn a probe into findings.
pub fn findings(p: &MeshProbe, home: &std::path::Path) -> Vec<Finding> {
    let mut out = Vec::new();
    // Two listeners on the mesh port is a FAIL whatever else is true.
    if let Some(pids) = &p.listener_pids
        && pids.len() > 1
    {
        out.push(
            f("listeners", Severity::Fail, format!("{} processes listen on 9489 (pids {pids:?}); the service and a collapsed daemon must not both bind it", pids.len()))
                .remedy("weaver kernel stop   (the daemon binds 9489 itself only in collapsed mode)"),
        );
    }
    if !p.record_present {
        out.push(f("service", Severity::Ok, "no machine mesh service on this host (collapsed mode); nothing to check"));
        return out;
    }
    let Some(st) = &p.status else {
        out.push(
            f("service", Severity::Fail, format!("service record present but the service is not reachable: {}", p.reach_error.as_deref().unwrap_or("no answer")))
                .remedy("weaver mesh status"),
        );
        return out;
    };
    out.push(f("service", Severity::Ok, format!("service reachable, node {}", st["node_id"].as_str().unwrap_or("-"))));

    let (lo, hi) = (st["proto"]["min"].as_u64().unwrap_or(0), st["proto"]["max"].as_u64().unwrap_or(0));
    if lo <= u64::from(PROTO_MAX) && u64::from(PROTO_MIN) <= hi {
        out.push(f("proto", Severity::Ok, format!("mesh-local proto window {lo}..={hi} overlaps this weaver ({PROTO_MIN}..={PROTO_MAX})")));
    } else {
        out.push(
            f("proto", Severity::Fail, format!("service proto {lo}..={hi} does not overlap this weaver ({PROTO_MIN}..={PROTO_MAX})"))
                .remedy("update weaver and the service binary to the same build (weaver update, then the printed cp/restart lines)"),
        );
    }

    match (p.pinned, p.record_pubkey) {
        (None, _) => out.push(
            f("pin", Severity::Warn, "no pinned machine key (~/.weftos/mesh/machine.pub); the first contact is trust-on-first-use")
                .remedy("weaver mesh trust   (after comparing the fingerprint out of band)"),
        ),
        (Some(a), Some(b)) if a == b => out.push(f("pin", Severity::Ok, "machine key matches the pin")),
        (Some(_), Some(_)) => out.push(
            f("pin", Severity::Fail, "the service's machine key differs from the pinned key")
                .remedy("verify out of band, then weaver mesh trust --replace"),
        ),
        (Some(_), None) => out.push(f("pin", Severity::Warn, "pin present but service.json is unreadable")),
    }

    if let Some(j) = &p.journal {
        if j["ok"] == true {
            out.push(f("journal", Severity::Ok, format!("journal verifies ({} records)", j["records"])));
        } else {
            out.push(
                f("journal", Severity::Fail, format!("journal does not verify: {}", j["bad"]["reason"]))
                    .remedy("weaver mesh journal verify"),
            );
        }
    } else if st["journal"]["read_only"] == true {
        out.push(f("journal", Severity::Fail, "journal is read-only (quarantined tail)").remedy("weaver mesh journal verify --accept-truncate"));
    } else {
        out.push(f("journal", Severity::Ok, "journal verification needs an admin (`weaver mesh journal verify`); status shows no fault"));
    }

    match p.box_key {
        None => out.push(f("box_key", Severity::Ok, format!("{STATE_DIR}/node.key is not visible to this user (expected: the state directory is 0700, owned by the service account)"))),
        Some(k) if k.mode & 0o077 != 0 => out.push(
            f("box_key", Severity::Fail, format!("box key mode is {:o}, group/world can read it", k.mode & 0o777))
                .remedy(format!("sudo chmod 0600 {STATE_DIR}/node.key")),
        ),
        Some(k) if p.service_uid.is_some_and(|u| u != k.uid) => out.push(
            f("box_key", Severity::Fail, format!("box key is owned by uid {}, the service runs as uid {}", k.uid, p.service_uid.unwrap_or(0)))
                .remedy(format!("sudo chown <service account> {STATE_DIR}/node.key")),
        ),
        Some(_) => out.push(f("box_key", Severity::Ok, "box key mode 0600 and owned by the service account")),
    }

    if p.user_node_key {
        out.push(
            f("node_key_dup", Severity::Warn, "~/.weftos/run/node.key still exists while the service is active (the key is in two places)")
                .remedy("after the Pi/peers still see this node id: rm ~/.weftos/run/node.key"),
        );
    }

    if let Some(exe) = &p.service_exe
        && user_writable(exe, home)
    {
        out.push(
            f("exe", Severity::Fail, format!("the running service executable {} is in a user-writable directory; that user could replace it and take the box key", exe.display()))
                .remedy(format!("run the service from {SERVICE_EXE} (weaver mesh install-service)")),
        );
    }

    match st["unsigned_leaf_peers"].as_array() {
        Some(a) if !a.is_empty() => out.push(f("leaf_unsigned", Severity::Warn, format!("{} leaf peer(s) connected without signed admission: {a:?}", a.len()))),
        _ => {}
    }
    for r in st["force_revoked"].as_array().into_iter().flatten() {
        out.push(
            f("force_revoked", Severity::Warn, format!("uid {} is force-revoked (enforced from force-revoked.json; the journal could not record it)", r["id"]))
                .remedy("fix the journal, then re-run `weaver mesh bind revoke <uid>`"),
        );
    }
    out
}

/// Distinct pids listening on `port`, via `lsof` (None when lsof is absent).
pub fn listener_pids(port: u16) -> Option<Vec<u32>> {
    let out = std::process::Command::new("lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fp"])
        .output()
        .ok()?;
    let mut pids: Vec<u32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix('p')?.parse().ok())
        .collect();
    pids.sort_unstable();
    pids.dedup();
    Some(pids)
}

#[cfg(unix)]
fn stat_of(p: &std::path::Path) -> Option<FileStat> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(p).ok()?;
    Some(FileStat { mode: m.mode(), uid: m.uid() })
}

/// Read the system into a probe. Read-only.
pub async fn gather(home: &std::path::Path) -> MeshProbe {
    use crate::commands::mesh_cmd::{socket_of, status_for_doctor, ConnArgs};
    use clawft_mesh_local::proto::ServiceRecord;

    let conn = ConnArgs::default();
    let sock = socket_of(&conn);
    let rec = sock.parent().map(|d| d.join("service.json")).and_then(|p| ServiceRecord::load(&p).ok());
    let mut p = MeshProbe {
        record_present: rec.is_some(),
        record_pubkey: rec.as_ref().map(|r| r.machine_pubkey),
        service_uid: rec.as_ref().map(|r| r.service_uid),
        pinned: std::fs::read_to_string(home.join(".weftos/mesh/machine.pub"))
            .ok()
            .and_then(|t| clawft_mesh_local::hexser::decode::<32>(t.trim().to_ascii_lowercase().as_str())),
        box_key: stat_of(&PathBuf::from(STATE_DIR).join("node.key")),
        user_node_key: home.join(".weftos/run/node.key").exists(),
        listener_pids: listener_pids(9489),
        ..MeshProbe::default()
    };
    if p.record_present {
        match status_for_doctor(&conn).await {
            Ok((status, journal)) => {
                p.status = Some(status);
                p.journal = journal;
            }
            Err(e) => p.reach_error = Some(format!("{e:#}")),
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    fn running() -> MeshProbe {
        MeshProbe {
            record_present: true,
            record_pubkey: Some([7; 32]),
            service_uid: Some(300),
            status: Some(json!({"node_id": "n1", "proto": {"min": 1, "max": 1}, "journal": {}})),
            pinned: Some([7; 32]),
            ..MeshProbe::default()
        }
    }

    fn sev(v: &[Finding], id: &str) -> Severity {
        v.iter().find(|x| x.id == format!("mesh.{id}")).unwrap_or_else(|| panic!("{id}")).severity
    }

    #[test]
    fn healthy_service_has_no_warnings() {
        let mut p = running();
        p.box_key = Some(FileStat { mode: 0o100600, uid: 300 });
        let v = findings(&p, Path::new("/Users/a"));
        assert!(v.iter().all(|x| x.severity == Severity::Ok), "{v:?}");
    }

    #[test]
    fn no_service_is_fine() {
        let v = findings(&MeshProbe::default(), Path::new("/h"));
        assert_eq!(sev(&v, "service"), Severity::Ok);
    }

    #[test]
    fn two_listeners_fail_even_without_a_service() {
        let p = MeshProbe { listener_pids: Some(vec![10, 20]), ..MeshProbe::default() };
        assert_eq!(sev(&findings(&p, Path::new("/h")), "listeners"), Severity::Fail);
        let one = MeshProbe { listener_pids: Some(vec![10]), ..MeshProbe::default() };
        assert!(!findings(&one, Path::new("/h")).iter().any(|x| x.id == "mesh.listeners"));
    }

    #[test]
    fn unreachable_pin_proto_key_and_dup_checks() {
        let mut p = running();
        p.status = None;
        p.reach_error = Some("refused".into());
        assert_eq!(sev(&findings(&p, Path::new("/h")), "service"), Severity::Fail);

        let mut p = running();
        p.pinned = Some([9; 32]);
        p.status = Some(json!({"proto": {"min": 5, "max": 6}}));
        p.box_key = Some(FileStat { mode: 0o100644, uid: 300 });
        p.user_node_key = true;
        let v = findings(&p, Path::new("/h"));
        assert_eq!(sev(&v, "pin"), Severity::Fail);
        assert_eq!(sev(&v, "proto"), Severity::Fail);
        assert_eq!(sev(&v, "box_key"), Severity::Fail);
        assert_eq!(sev(&v, "node_key_dup"), Severity::Warn);

        let mut p = running();
        p.pinned = None;
        assert_eq!(sev(&findings(&p, Path::new("/h")), "pin"), Severity::Warn);
        let mut p = running();
        p.box_key = Some(FileStat { mode: 0o100600, uid: 501 });
        assert_eq!(sev(&findings(&p, Path::new("/h")), "box_key"), Severity::Fail);
    }

    #[test]
    fn journal_exe_and_force_revoked() {
        let mut p = running();
        p.journal = Some(json!({"ok": false, "bad": {"reason": "bad sig"}}));
        p.service_exe = Some(PathBuf::from("/Users/a/.cargo/bin/weaver"));
        p.status = Some(json!({"proto": {"min": 1, "max": 1}, "force_revoked": [{"id": 501}]}));
        let v = findings(&p, Path::new("/Users/a"));
        assert_eq!(sev(&v, "journal"), Severity::Fail);
        assert_eq!(sev(&v, "exe"), Severity::Fail);
        assert_eq!(sev(&v, "force_revoked"), Severity::Warn);
        p.service_exe = Some(PathBuf::from(SERVICE_EXE));
        assert!(!findings(&p, Path::new("/Users/a")).iter().any(|x| x.id == "mesh.exe"));
    }
}
