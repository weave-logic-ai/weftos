//! Linux probes: CPU, distro/kernel, memory, storage, board class and
//! binfmt emulation.
//!
//! Sources: `/proc/cpuinfo`, `/proc/device-tree/model`, `/etc/os-release`,
//! `uname -r`, `/proc/meminfo`, `/proc/mounts`, `df`, and
//! `/proc/sys/fs/binfmt_misc`.

use clawft_types::placement::Provenance;

use super::host::ProbeHost;
use super::probe::{Collected, bytes_attr, cap, normalize_arch, note_external_withheld, parse_df, public_mount};

const BINFMT: &str = "/proc/sys/fs/binfmt_misc";
/// Mount roots treated as external (removable) drives.
const EXTERNAL_ROOTS: &[&str] = &["/media/", "/mnt/", "/run/media/"];

/// Arch a binfmt handler emulates, or `None` for handlers that are not
/// CPU emulators (`register`, `status`, Mach-O shims, ...).
pub fn binfmt_arch(name: &str) -> Option<String> {
    if name.starts_with("rosetta") {
        return Some("x86_64".into());
    }
    let arch = name.strip_prefix("qemu-")?;
    Some(match arch {
        "arm" => "armv7".into(),
        other => normalize_arch(other),
    })
    .filter(|a: &String| !a.is_empty())
}

/// Parse `name enabled|disabled` lines into emulated arches, excluding the
/// native one. Sorted and deduplicated.
pub fn emulated_from_listing(lines: &str, native: &str) -> Vec<String> {
    let mut out: Vec<String> = lines
        .lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let name = parts.next()?;
            match parts.next() {
                Some("enabled") | None => binfmt_arch(name),
                _ => None,
            }
        })
        .filter(|a| a != native)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Emulated arches registered with this kernel's binfmt_misc.
pub fn host_binfmt_emulated(host: &dyn ProbeHost, native: &str) -> Vec<String> {
    let listing: String = host
        .list_dir(BINFMT)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| n != "register" && n != "status")
        .map(|n| {
            let state = host
                .read_file(&format!("{BINFMT}/{n}"))
                .and_then(|s| s.lines().next().map(str::to_string))
                .unwrap_or_default();
            format!("{n} {state}\n")
        })
        .collect();
    emulated_from_listing(&listing, native)
}

fn kv<'a>(text: &'a str, key: &str, sep: char) -> Option<&'a str> {
    text.lines()
        .filter_map(|l| l.split_once(sep))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().trim_matches('"'))
}

/// Current available memory, from `/proc/meminfo` alone (cheap live probe).
pub fn live_free(host: &dyn ProbeHost) -> Option<u64> {
    host.read_file("/proc/meminfo")
        .as_deref()
        .and_then(|t| meminfo_bytes(t, "MemAvailable"))
}

fn meminfo_bytes(text: &str, key: &str) -> Option<u64> {
    kv(text, key, ':')?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
        .map(|kb| kb.saturating_mul(1024))
}

/// Run the Linux probes.
pub fn probe(host: &dyn ProbeHost, arch: &str, c: &mut Collected) {
    let cpuinfo = host.read_file("/proc/cpuinfo").unwrap_or_default();
    let board = host
        .read_file("/proc/device-tree/model")
        .map(|s| s.trim_end_matches('\0').trim().to_string())
        .filter(|s| !s.is_empty());

    if let Some(mut cp) = cap(&format!("cpu.arch.{arch}"), Provenance::Probed) {
        let cores = cpuinfo
            .lines()
            .filter(|l| l.starts_with("processor"))
            .count();
        if cores > 0 {
            cp = cp.with_attr("cores", cores as i64);
        }
        let model = board
            .clone()
            .or_else(|| kv(&cpuinfo, "model name", ':').map(str::to_string));
        if let Some(m) = model {
            cp = cp.with_attr("model", m.as_str());
        }
        c.push(cp);
    }

    if let Some(mut os) = cap("os.linux", Provenance::Probed) {
        if let Some(d) = host
            .read_file("/etc/os-release")
            .as_deref()
            .and_then(|t| kv(t, "PRETTY_NAME", '=').map(str::to_string))
        {
            os = os.with_attr("distro", d.as_str());
        }
        if let Some(k) = host.run("uname", &["-r"]) {
            os = os.with_attr("kernel", k.trim());
        }
        c.push(os);
    }

    if let Some(mi) = host.read_file("/proc/meminfo")
        && let (Some(total), Some(m)) = (
            meminfo_bytes(&mi, "MemTotal"),
            cap("mem.system", Provenance::Probed),
        )
    {
        let mut m = m.with_attr("total", bytes_attr(total));
        if let Some(avail) = meminfo_bytes(&mi, "MemAvailable") {
            m = m.with_attr("free", bytes_attr(avail));
        }
        c.push(m);
    }

    storage(host, c);

    if let Some(b) = &board {
        let class = if b.contains("Raspberry Pi 5") {
            "pi5"
        } else {
            "other"
        };
        if let Some(nc) = cap(&format!("node.class.{class}"), Provenance::Probed) {
            c.push(nc.with_attr("board", b.as_str()));
        }
    }
}

fn storage(host: &dyn ProbeHost, c: &mut Collected) {
    if let Some((free, mount)) = host.run("df", &["-kP", "/"]).as_deref().and_then(parse_df)
        && let Some(s) = cap("store.tier.internal", Provenance::Probed)
    {
        let mut s = s.with_attr("free", bytes_attr(free));
        if let Some(m) = public_mount(&mount) {
            s = s.with_attr("mount", m);
        }
        c.push(s);
    }
    let mounts = host.read_file("/proc/mounts").unwrap_or_default();
    let mut external = 0;
    for line in mounts.lines() {
        let Some(mp) = line.split_whitespace().nth(1) else {
            continue;
        };
        let mp = mp.replace("\\040", " ");
        if !EXTERNAL_ROOTS.iter().any(|r| mp.starts_with(r)) {
            continue;
        }
        let free = host.run("df", &["-kP", &mp]).as_deref().and_then(parse_df);
        // Mount points carry the user and the drive label: they stay local.
        if let Some(mut s) = cap("store.tier.external", Provenance::Probed) {
            s = s.with_attr("mounted", true);
            if let Some((f, _)) = free {
                s = s.with_attr("free", bytes_attr(f));
            }
            c.push(s);
            external += 1;
        }
    }
    note_external_withheld(c, external);
}
