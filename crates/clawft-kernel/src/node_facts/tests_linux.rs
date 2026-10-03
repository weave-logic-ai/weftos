//! Linux probes against scripted ARM boards: a Pi 5 with Docker engine,
//! host binfmt emulation and a Coral Edge TPU, and a 32-bit Pi Zero 2 W.

use clawft_types::placement::{
    AttrValue, Capability, CapabilityId, Provenance, Requirement, match_all,
};

use super::fake_host::FakeHost;
use super::linux::{binfmt_arch, emulated_from_listing};
use super::probe::{ProbeConfig, build_facts, normalize_arch, parse_df, probe_capabilities};

const BINFMT: &str = "/proc/sys/fs/binfmt_misc";

fn pi5() -> FakeHost {
    FakeHost::new("linux", "aarch64")
        .file("/proc/cpuinfo", "processor\t: 0\nprocessor\t: 1\nprocessor\t: 2\nprocessor\t: 3\n")
        .file("/proc/device-tree/model", "Raspberry Pi 5 Model B Rev 1.0\0")
        .file("/etc/os-release", "PRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\nID=debian\n")
        .out("uname -r", "6.6.51+rpt-rpi-2712\n")
        .file("/proc/meminfo", "MemTotal:        8245376 kB\nMemFree:  100 kB\nMemAvailable:    6000000 kB\n")
        .out("df -kP /", "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/mmcblk0p2 60000000 10000000 50000000 17% /\n")
        .file("/proc/mounts", "/dev/mmcblk0p2 / ext4 rw 0 0\n/dev/sda1 /media/pi/USB\\040DRIVE vfat rw 0 0\n")
        .out("df -kP /media/pi/USB DRIVE", "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/sda1 1000 10 990 1% /media/pi/USB DRIVE\n")
        .dir(BINFMT, &["qemu-x86_64", "qemu-arm", "qemu-riscv64", "register", "status"])
        .file(&format!("{BINFMT}/qemu-x86_64"), "enabled\ninterpreter /usr/bin/qemu-x86_64\n")
        .file(&format!("{BINFMT}/qemu-arm"), "enabled\n")
        .file(&format!("{BINFMT}/qemu-riscv64"), "disabled\n")
        .out(
            "docker info --format {{json .}}",
            r#"{"ServerVersion":"27.3.1","OperatingSystem":"Debian GNU/Linux 12 (bookworm)","Architecture":"aarch64"}"#,
        )
}

fn with_coral_pcie(h: FakeHost) -> FakeHost {
    h.dir("/dev", &["null", "apex_0", "tty"])
}

fn with_coral_usb(h: FakeHost) -> FakeHost {
    h.out("lsusb", "Bus 001 Device 002: ID 18d1:9302 Google Inc.\nBus 001 Device 001: ID 1d6b:0002 Linux Foundation 2.0 root hub\n")
}

fn strs(c: &Capability, attr: &str) -> Vec<String> {
    match c.attrs.get(attr) {
        Some(AttrValue::List(l)) => l
            .iter()
            .filter_map(|x| match x {
                AttrValue::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

#[test]
fn pi5_linux_arm_facts() {
    let c = probe_capabilities(&pi5(), &ProbeConfig::default());
    let f = build_facts("n-pi5aaa", 1_000, 600, 1, c);
    f.validate().unwrap();
    let one = |id: &str| {
        f.find(id)
            .next()
            .unwrap_or_else(|| panic!("missing {id}"))
            .clone()
    };

    let cpu = one("cpu.arch.aarch64");
    assert_eq!(cpu.attrs["cores"], AttrValue::Int(4));
    assert_eq!(
        cpu.attrs["model"],
        AttrValue::from("Raspberry Pi 5 Model B Rev 1.0")
    );
    assert_eq!(
        one("os.linux").attrs["distro"],
        AttrValue::from("Debian GNU/Linux 12 (bookworm)")
    );
    let mem = one("mem.system");
    assert_eq!(mem.attrs["total"], AttrValue::Int(8_245_376 * 1024));
    assert_eq!(mem.attrs["free"], AttrValue::Int(6_000_000 * 1024));
    assert!(
        f.find("mem.unified").next().is_none(),
        "a Pi is not a unified-memory node"
    );
    assert!(f.find("node.class.pi5").next().is_some());

    // Disabled handlers do not count; enabled ones do, for native and a
    // local engine sharing the kernel.
    for id in ["runtime.native", "runtime.container.docker"] {
        assert_eq!(
            strs(&one(id), "arches_emulated"),
            vec!["armv7", "x86_64"],
            "{id}"
        );
    }
    assert_eq!(
        one("runtime.container.docker").attrs["variant"],
        AttrValue::from("engine")
    );

    let ext = one("store.tier.external");
    assert!(
        !ext.attrs.contains_key("mount"),
        "an external drive's label and user are not advertised"
    );
    assert!(
        f.notes.iter().any(|n| n.probe == "store.tier.external" && !n.note.contains("USB")),
        "the omission is explained without naming the drive"
    );
    assert_eq!(ext.attrs["free"], AttrValue::Int(990 * 1024));

    assert!(
        f.find("accel.tpu.coral").next().is_none(),
        "no device, no Coral"
    );
    assert!(f.find("runtime.container.apple").next().is_none());
    assert!(f.find("accel.gpu.metal").next().is_none());
}

#[test]
fn fake_coral_over_pcie_and_usb_is_advertised_exclusive_and_matchable() {
    for host in [with_coral_pcie(pi5()), with_coral_usb(pi5())] {
        let c = probe_capabilities(&host, &ProbeConfig::default());
        let coral: Vec<_> = c
            .caps
            .iter()
            .filter(|c| c.id.as_str() == "accel.tpu.coral")
            .collect();
        assert_eq!(coral.len(), 1);
        assert!(coral[0].exclusive);
        assert_eq!(coral[0].provenance, Provenance::Probed);
        assert_eq!(strs(coral[0], "formats"), vec!["tflite"]);
        assert!(c.caps.iter().any(|c| c.id.as_str() == "format.tflite"));

        // A TFLite workload needing the TPU exclusively matches this node.
        let req = Requirement::exact(CapabilityId::new("accel.tpu.coral").unwrap())
            .exclusive()
            .with_min_provenance(Provenance::Probed);
        assert!(match_all(&[req], &c.caps).is_ok());
    }
}

#[test]
fn pi_zero_32bit_userland_is_armv7() {
    let h = FakeHost::new("linux", "arm")
        .file("/proc/device-tree/model", "Raspberry Pi Zero 2 W Rev 1.0\0")
        .file(
            "/proc/meminfo",
            "MemTotal: 437000 kB\nMemAvailable: 200000 kB\n",
        );
    let c = probe_capabilities(&h, &ProbeConfig::default());
    assert!(c.has("cpu.arch.armv7"));
    assert!(c.has("node.class.other"));
    let native = c
        .caps
        .iter()
        .find(|c| c.id.as_str() == "runtime.native")
        .unwrap();
    assert_eq!(strs(native, "arches_native"), vec!["armv7"]);
}

#[test]
fn measured_perf_and_declared_feeds_keep_honest_provenance() {
    let feed = Capability::new(
        CapabilityId::new("feed.esp32-csi-udp").unwrap(),
        Provenance::Measured,
    )
    .with_attr("lan_id", "lan-a");
    let perf = clawft_types::placement::perf::cog_cycle_ms("fall-detect", 1_000.0).unwrap();
    let not_measured = Capability::new(
        CapabilityId::new("perf.cog.cycle_ms").unwrap(),
        Provenance::Claimed,
    );
    let cfg = ProbeConfig {
        declared_feeds: vec![feed],
        measured: vec![perf, not_measured],
        ..ProbeConfig::default()
    };
    let c = probe_capabilities(&pi5(), &cfg);
    let feed = c
        .caps
        .iter()
        .find(|c| c.id.as_str() == "feed.esp32-csi-udp")
        .unwrap();
    assert_eq!(
        feed.provenance,
        Provenance::Claimed,
        "declared feeds are claimed"
    );
    let perfs: Vec<_> = c.caps.iter().filter(|c| c.id.family() == "perf").collect();
    assert_eq!(perfs.len(), 1);
    assert_eq!(perfs[0].provenance, Provenance::Measured);
    let req =
        clawft_types::placement::perf::require_cog_cycle_within("fall-detect", 2_000.0).unwrap();
    assert!(match_all(&[req], &c.caps).is_ok());
}

#[test]
fn nvidia_gpu_only_with_nvidia_smi() {
    let h = pi5().out(
        "nvidia-smi --query-gpu=name,memory.total,memory.free --format=csv,noheader,nounits",
        "NVIDIA RTX A2000, 12288, 12000\n",
    );
    let c = probe_capabilities(&h, &ProbeConfig::default());
    let g = c
        .caps
        .iter()
        .find(|c| c.id.as_str() == "accel.gpu.cuda")
        .unwrap();
    assert_eq!(g.attrs["mem_bytes"], AttrValue::Int(12_288 * 1024 * 1024));
    assert!(c.has("mem.vram"));
}

#[test]
fn parsers() {
    assert_eq!(binfmt_arch("qemu-arm").as_deref(), Some("armv7"));
    assert_eq!(binfmt_arch("rosetta\u{200b}").as_deref(), Some("x86_64"));
    assert_eq!(binfmt_arch("mac-macho-arm64"), None);
    assert_eq!(
        emulated_from_listing(
            "qemu-aarch64 enabled\nqemu-arm enabled\nqemu-i386 disabled\n",
            "aarch64"
        ),
        vec!["armv7"]
    );
    assert_eq!(normalize_arch("arm64"), "aarch64");
    assert_eq!(binfmt_arch("qemu-mips64el").as_deref(), Some("mips64le"));
    assert_eq!(normalize_arch("armv7l"), "armv7");
    assert_eq!(normalize_arch("AMD64"), "x86_64");
    let df = "Filesystem 1024-blocks Used Available Capacity Mounted on\nmap auto home 0 0 12 100% /Volumes/My Disk\n";
    assert_eq!(parse_df(df), Some((12 * 1024, "/Volumes/My Disk".into())));
    assert_eq!(parse_df("garbage"), None);
}
