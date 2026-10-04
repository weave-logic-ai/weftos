//! Read-only `/proc` and log-file facts for `/status`: which TCP ports a running cog listens on,
//! and the size and age of its output log.
//!
//! The `/proc/net/tcp*` tables are read once and cached for a few seconds, and are read by the HTTP
//! layer after it has released the supervisor lock, so a slow `/proc` read never stalls supervision.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TABLE_TTL: Duration = Duration::from_secs(3);

/// The text of `/proc/net/tcp` and `/proc/net/tcp6`, read once.
#[derive(Default)]
pub struct NetTables {
    tcp: String,
    tcp6: String,
}

impl NetTables {
    pub fn read(proc_root: &Path) -> Self {
        let rd = |f: &str| std::fs::read_to_string(proc_root.join(f)).unwrap_or_default();
        Self { tcp: rd("net/tcp"), tcp6: rd("net/tcp6") }
    }
}

type TableCache = Mutex<Option<(Instant, Arc<NetTables>)>>;
static CACHE: TableCache = Mutex::new(None);

/// The net tables for `/proc`, re-read at most every few seconds.
pub fn cached_tables() -> Arc<NetTables> {
    let mut c = CACHE.lock().unwrap();
    if let Some((at, t)) = c.as_ref()
        && at.elapsed() < TABLE_TTL
    {
        return Arc::clone(t);
    }
    let t = Arc::new(NetTables::read(Path::new("/proc")));
    *c = Some((Instant::now(), Arc::clone(&t)));
    t
}

/// Listening TCP ports of `pid`: socket inodes from `<proc>/<pid>/fd`, matched against the LISTEN
/// rows (state `0A`) of the net tables. Sorted, de-duplicated.
pub fn listen_ports(tables: &NetTables, proc_root: &Path, pid: u32) -> Vec<u16> {
    let Ok(rd) = std::fs::read_dir(proc_root.join(pid.to_string()).join("fd")) else { return Vec::new() };
    let inodes: HashSet<String> = rd
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .filter_map(|l| l.to_str()?.strip_prefix("socket:[")?.strip_suffix(']').map(str::to_string))
        .collect();
    let mut ports: Vec<u16> = [&tables.tcp, &tables.tcp6].into_iter().flat_map(|t| parse_listen_rows(t, &inodes)).collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

fn parse_listen_rows(table: &str, inodes: &HashSet<String>) -> Vec<u16> {
    table
        .lines()
        .skip(1)
        .filter_map(|l| {
            let c: Vec<&str> = l.split_whitespace().collect();
            // sl local_address rem_address st tx:rx tr:when retrnsmt uid timeout inode
            let (local, st, inode) = (c.get(1)?, c.get(3)?, c.get(9)?);
            (*st == "0A" && inodes.contains(*inode)).then(|| u16::from_str_radix(local.rsplit(':').next()?, 16).ok()).flatten()
        })
        .collect()
}

/// (size, seconds since last write) of a cog's log, `None` when there is no log yet.
pub fn log_stats(path: &Path) -> (Option<u64>, Option<u64>) {
    let Ok(m) = std::fs::metadata(path) else { return (None, None) };
    let age = m.modified().ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs());
    (Some(m.len()), age)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_rows_match_only_this_process_listeners() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:1F5C 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111 1 0 100 0 0 10 0\n\
   1: 00000000:2648 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 222 1 0 100 0 0 10 0\n\
   2: 0100007F:D000 0100007F:1F5C 01 00000000:00000000 00:00000000 00000000  1000        0 111 1 0 100 0 0 10 0\n";
        let mine: HashSet<String> = ["111".to_string()].into();
        assert_eq!(parse_listen_rows(table, &mine), vec![8028], "0x1F5C listening, inode 111; the ESTABLISHED row is skipped");
    }

    #[test]
    fn listen_ports_reads_a_proc_tree_once() {
        let d = tempfile::tempdir().unwrap();
        let fd = d.path().join("42/fd");
        std::fs::create_dir_all(&fd).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("socket:[333]", fd.join("3")).unwrap();
        std::fs::create_dir_all(d.path().join("net")).unwrap();
        std::fs::write(d.path().join("net/tcp"), "hdr\n   0: 0100007F:1F54 00000000:0000 0A 0:0 0:0 0 1000 0 333 1\n").unwrap();
        std::fs::write(d.path().join("net/tcp6"), "hdr\n").unwrap();
        let t = NetTables::read(d.path());
        #[cfg(unix)]
        assert_eq!(listen_ports(&t, d.path(), 42), vec![8020]);
        assert!(listen_ports(&t, d.path(), 99).is_empty());
        // a missing /proc (non-Linux) is empty, not an error
        let none = Path::new("/nonexistent");
        assert!(listen_ports(&NetTables::read(none), none, 1).is_empty());
    }

    #[test]
    fn log_stats_reports_real_size_and_age_or_nothing() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("host.log");
        assert_eq!(log_stats(&p), (None, None));
        std::fs::write(&p, b"0123456789").unwrap();
        let (bytes, age) = log_stats(&p);
        assert_eq!(bytes, Some(10));
        assert!(age.is_some_and(|a| a < 5));
    }
}
