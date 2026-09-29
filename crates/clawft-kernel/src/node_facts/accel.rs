//! Vendor accelerator probes, run only when the vendor's tool or device
//! node is present: NVIDIA (`nvidia-smi`), Hailo (`hailortcli`) and the
//! Coral Edge TPU (`/dev/apex_*` PCIe device nodes, or a USB id seen by
//! `lsusb`). Apple Metal and the ANE are probed in `macos`.

use clawft_types::placement::Provenance;

use super::host::ProbeHost;
use super::probe::{Collected, bytes_attr, cap, str_list};

/// Coral USB ids: before firmware load (Global Unichip) and after (Google).
const CORAL_USB_IDS: &[&str] = &["1a6e:089a", "18d1:9302"];

/// Run the vendor accelerator probes.
pub fn probe(host: &dyn ProbeHost, c: &mut Collected) {
    cuda(host, c);
    hailo(host, c);
    coral(host, c);
}

fn cuda(host: &dyn ProbeHost, c: &mut Collected) {
    if host.which("nvidia-smi").is_none() {
        return;
    }
    let Some(out) = host.run(
        "nvidia-smi",
        &[
            "--query-gpu=name,memory.total,memory.free",
            "--format=csv,noheader,nounits",
        ],
    ) else {
        c.note(
            "accel.gpu.cuda",
            "nvidia-smi present but failed; no CUDA GPU advertised",
        );
        return;
    };
    for (i, line) in out.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        let mib = |s: Option<&&str>| {
            s.and_then(|v| v.parse::<u64>().ok())
                .map(|m| m * 1024 * 1024)
        };
        let (total, free) = (mib(f.get(1)), mib(f.get(2)));
        let device = format!("cuda:{i}");
        if let Some(mut g) = cap("accel.gpu.cuda", Provenance::Probed) {
            g = g
                .with_attr("vendor", "nvidia")
                .with_attr("device", device.as_str())
                .with_attr("unified", false);
            if let Some(name) = f.first() {
                g = g.with_attr("model", *name);
            }
            if let Some(t) = total {
                g = g.with_attr("mem_bytes", bytes_attr(t));
            }
            c.push(g);
        }
        if let (Some(t), Some(v)) = (total, cap("mem.vram", Provenance::Probed)) {
            let mut v = v
                .with_attr("total", bytes_attr(t))
                .with_attr("device", device.as_str());
            if let Some(fr) = free {
                v = v.with_attr("free", bytes_attr(fr));
            }
            c.push(v);
        }
    }
}

fn hailo(host: &dyn ProbeHost, c: &mut Collected) {
    if host.which("hailortcli").is_none() {
        return;
    }
    let Some(out) = host.run("hailortcli", &["scan"]) else {
        c.note("accel.npu.hailo", "hailortcli present but scan failed");
        return;
    };
    for dev in out
        .lines()
        .filter_map(|l| l.split("Device:").nth(1))
        .map(str::trim)
    {
        if let Some(h) = cap("accel.npu.hailo", Provenance::Probed) {
            c.push(
                h.with_attr("device", dev)
                    .with_attr("formats", str_list(&["hef"]))
                    .exclusive(),
            );
        }
    }
}

fn coral(host: &dyn ProbeHost, c: &mut Collected) {
    let mut devices: Vec<String> = host
        .list_dir("/dev")
        .unwrap_or_default()
        .into_iter()
        .filter(|n| n.starts_with("apex_"))
        .map(|n| format!("pcie:{n}"))
        .collect();
    if host.which("lsusb").is_some() {
        let usb = host.run("lsusb", &[]).unwrap_or_default();
        let n = usb
            .lines()
            .filter(|l| CORAL_USB_IDS.iter().any(|id| l.contains(id)))
            .count();
        devices.extend((0..n).map(|i| format!("usb:{i}")));
    }
    for dev in devices {
        if let Some(t) = cap("accel.tpu.coral", Provenance::Probed) {
            c.push(
                t.with_attr("device", dev.as_str())
                    .with_attr("formats", str_list(&["tflite"]))
                    .exclusive(),
            );
            c.note(
                "accel.tpu.coral",
                format!("{dev}: Edge TPU device seen; single client, so exclusive"),
            );
        }
    }
}
