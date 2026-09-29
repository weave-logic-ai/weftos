//! macOS probes: CPU, OS version, unified memory, storage, Metal GPU and
//! the Apple Neural Engine.
//!
//! Sources: `sysctl`, `sw_vers`, `vm_stat`, `df`, and
//! `system_profiler SPDisplaysDataType -json`. The ANE has no public query
//! API; its presence is inferred from an Apple-silicon chip plus
//! `CoreML.framework`, so it is advertised as `claimed` with a note, never
//! `probed` or `measured`, and its busy state is not known.

use clawft_types::placement::{AttrValue, Provenance};

use super::host::ProbeHost;
use super::probe::{Collected, bytes_attr, cap, parse_df, str_list};

const COREML: &str = "/System/Library/Frameworks/CoreML.framework";

fn sysctl(host: &dyn ProbeHost, key: &str) -> Option<String> {
    host.run("sysctl", &["-n", key])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Free memory from `vm_stat`: (free + inactive + speculative) pages.
pub fn parse_vm_stat(out: &str) -> Option<u64> {
    let page: u64 = out
        .lines()
        .next()?
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let pages = |label: &str| -> u64 {
        out.lines()
            .find(|l| l.starts_with(label))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().trim_end_matches('.').parse().ok())
            .unwrap_or(0)
    };
    let n = pages("Pages free") + pages("Pages inactive") + pages("Pages speculative");
    Some(n.saturating_mul(page))
}

/// Run the macOS probes.
pub fn probe(host: &dyn ProbeHost, arch: &str, c: &mut Collected) {
    let brand = sysctl(host, "machdep.cpu.brand_string");
    let apple_silicon =
        arch == "aarch64" && brand.as_deref().is_some_and(|b| b.starts_with("Apple"));

    if let Some(mut cp) = cap(&format!("cpu.arch.{arch}"), Provenance::Probed) {
        if let Some(n) = sysctl(host, "hw.ncpu").and_then(|s| s.parse::<i64>().ok()) {
            cp = cp.with_attr("cores", n);
        }
        if let Some(b) = &brand {
            cp = cp.with_attr("model", b.as_str());
        }
        c.push(cp);
    }

    if let Some(mut os) = cap("os.macos", Provenance::Probed) {
        if let Some(v) = host.run("sw_vers", &["-productVersion"]) {
            os = os.with_attr("version", v.trim());
        }
        c.push(os);
    }

    let total = sysctl(host, "hw.memsize").and_then(|s| s.parse::<u64>().ok());
    let free = host.run("vm_stat", &[]).as_deref().and_then(parse_vm_stat);
    if let Some(total) = total {
        let ids: &[&str] = if apple_silicon {
            &["mem.unified", "mem.system"]
        } else {
            &["mem.system"]
        };
        for id in ids {
            if let Some(mut m) = cap(id, Provenance::Probed) {
                m = m.with_attr("total", bytes_attr(total));
                if let Some(f) = free {
                    m = m.with_attr("free", bytes_attr(f.min(total)));
                }
                c.push(m);
            }
        }
        if apple_silicon {
            c.note(
                "mem.unified",
                "sysctl hw.memsize; Apple silicon: CPU, GPU and ANE share one pool",
            );
        }
    }

    storage(host, c);
    metal(host, apple_silicon, total, c);
    if apple_silicon {
        ane(host, brand.as_deref().unwrap_or("Apple silicon"), c);
    }
    if let Some(dm) = cap("node.class.dev-mac", Provenance::Claimed) {
        c.push(dm);
    }
}

fn storage(host: &dyn ProbeHost, c: &mut Collected) {
    if let Some((free, mount)) = host.run("df", &["-kP", "/"]).as_deref().and_then(parse_df)
        && let Some(s) = cap("store.tier.internal", Provenance::Probed)
    {
        c.push(
            s.with_attr("free", bytes_attr(free))
                .with_attr("mount", mount.as_str()),
        );
    }
    for vol in host.list_dir("/Volumes").unwrap_or_default() {
        let path = format!("/Volumes/{vol}");
        let Some((free, mount)) = host
            .run("df", &["-kP", &path])
            .as_deref()
            .and_then(parse_df)
        else {
            continue;
        };
        // `/Volumes/Macintosh HD` is an alias of `/`, not a drive.
        if mount != path {
            continue;
        }
        if let Some(s) = cap("store.tier.external", Provenance::Probed) {
            c.push(
                s.with_attr("mounted", true)
                    .with_attr("free", bytes_attr(free))
                    .with_attr("mount", mount.as_str()),
            );
        }
    }
}

fn metal(host: &dyn ProbeHost, apple_silicon: bool, total: Option<u64>, c: &mut Collected) {
    let Some(json) = host.run("system_profiler", &["SPDisplaysDataType", "-json"]) else {
        c.note(
            "accel.gpu.metal",
            "system_profiler unavailable; GPU not advertised",
        );
        return;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) else {
        c.note("accel.gpu.metal", "system_profiler output was not JSON");
        return;
    };
    let gpus = v["SPDisplaysDataType"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for g in gpus {
        let Some(family) = g["spdisplays_mtlgpufamilysupport"].as_str() else {
            continue; // no Metal support reported: do not advertise
        };
        let Some(mut m) = cap("accel.gpu.metal", Provenance::Probed) else {
            continue;
        };
        let version = family.trim_start_matches("spdisplays_metal");
        m = m
            .with_attr("sdk", "metal")
            .with_attr("sdk_version", version)
            .with_attr("unified", apple_silicon);
        if let Some(model) = g["sppci_model"].as_str() {
            m = m.with_attr("device", model);
        }
        if let Some(vendor) = g["spdisplays_vendor"].as_str() {
            let vendor = vendor
                .trim_start_matches("sppci_vendor_")
                .to_ascii_lowercase();
            m = m.with_attr("vendor", vendor.as_str());
        }
        if let Some(cores) = g["sppci_cores"]
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
        {
            m = m.with_attr("cores", cores);
        }
        if let (true, Some(t)) = (apple_silicon, total) {
            m = m.with_attr("mem_bytes", bytes_attr(t));
        }
        c.push(m);
        c.note(
            "accel.gpu.metal",
            "system_profiler SPDisplaysDataType (Metal family, cores); mem_bytes is the unified pool, not extra memory",
        );
    }
}

fn ane(host: &dyn ProbeHost, chip: &str, c: &mut Collected) {
    if !host.exists(COREML) {
        c.note(
            "accel.npu.ane",
            "CoreML.framework not found; ANE not advertised",
        );
        return;
    }
    if let Some(a) = cap("accel.npu.ane", Provenance::Claimed) {
        c.push(
            a.with_attr("device", chip)
                .with_attr("unified", true)
                .with_attr("formats", str_list(&["coreml"]))
                .with_attr("utilisation_known", AttrValue::Bool(false)),
        );
        c.note(
            "accel.npu.ane",
            "claimed: inferred from Apple-silicon chip + CoreML.framework; no public API reports ANE busy state or throughput",
        );
    }
}
