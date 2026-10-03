//! macOS probes against a scripted Apple-silicon Mac (OrbStack + Apple
//! container + llama-server), mirroring the acceptance machine.

use clawft_types::placement::{AttrValue, CapabilityState, NodeFacts, Provenance};

use super::fake_host::FakeHost;
use super::probe::{Collected, ProbeConfig, build_facts, probe_capabilities};

const GIB128: i64 = 137_438_953_472;

const DOCKER_INFO: &str = r#"{"ServerVersion":"29.4.0","OperatingSystem":"OrbStack","Architecture":"aarch64","OSType":"linux"}"#;
const BUILDX: &str = "Name: orbstack\nPlatforms:        linux/arm64, linux/amd64, linux/amd64/v2, linux/riscv64, linux/386\n";
const VM_BINFMT: &str = "aarch64 enabled\nqemu-aarch64 enabled\nqemu-arm enabled\nqemu-x86_64 enabled\nrosetta enabled\nmac-macho-arm64 enabled\n";
const SP_DISPLAYS: &str = r#"{"SPDisplaysDataType":[{"_name":"Apple M5 Max","spdisplays_mtlgpufamilysupport":"spdisplays_metal4","spdisplays_vendor":"sppci_vendor_Apple","sppci_cores":"40","sppci_model":"Apple M5 Max"}]}"#;
const DF_ROOT: &str = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s1s1 1948404040 13339416 481307340 3% /\n";
const DF_HD: &str = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s1s1 1948404040 13339416 481307340 3% /\n";
const DF_EXT: &str = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk4 1000 10 990 1% /Volumes/EXTDRIVE\n";

fn mac() -> FakeHost {
    FakeHost::new("macos", "aarch64")
        .out("sysctl -n machdep.cpu.brand_string", "Apple M5 Max\n")
        .out("sysctl -n hw.ncpu", "18\n")
        .out("sysctl -n hw.memsize", "137438953472\n")
        .out("sw_vers -productVersion", "27.0.1\n")
        .out("vm_stat", "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free: 1000.\nPages inactive: 2000.\nPages speculative: 100.\n")
        .out("df -kP /", DF_ROOT)
        .dir("/Volumes", &["EXTDRIVE", "Macintosh HD"])
        .out("df -kP /Volumes/Macintosh HD", DF_HD)
        .out("df -kP /Volumes/EXTDRIVE", DF_EXT)
        .out("system_profiler SPDisplaysDataType -json", SP_DISPLAYS)
        .path("/System/Library/Frameworks/CoreML.framework")
        .out("docker info --format {{json .}}", DOCKER_INFO)
        .out("docker buildx inspect", BUILDX)
        .out("docker image inspect --format {{.Id}} alpine:3.20", "sha256:abc\n")
        .out(&format!("docker run --rm --privileged --pull=never --network=none --entrypoint /bin/sh alpine:3.20 -c {}", super::runtimes::BINFMT_LIST), VM_BINFMT)
        .out("container --version", "container CLI version 1.0.0 (build: release)\n")
        .out("container system status", "status running\n")
        .out("llama-server --version", "version: 9820 (3fc4e1052)\nbuilt with AppleClang\n")
        .tool("mlx_lm.server")
        .out("ollama --version", "ollama version is 0.30.11\n")
}

fn strs(v: Option<&AttrValue>) -> Vec<String> {
    match v {
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
fn apple_silicon_mac_facts_match_the_acceptance_machine() {
    let c = probe_capabilities(&mac(), &ProbeConfig::default());
    let f = build_facts("n-abc123", 1_000, 600, 1, c);
    f.validate().unwrap();
    let one = |id: &str| {
        f.find(id)
            .next()
            .cloned()
            .unwrap_or_else(|| panic!("missing {id}"))
    };

    assert_eq!(
        one("cpu.arch.aarch64").attrs["model"],
        AttrValue::from("Apple M5 Max")
    );
    assert_eq!(one("os.macos").attrs["version"], AttrValue::from("27.0.1"));
    let unified = one("mem.unified");
    assert_eq!(unified.attrs["total"], AttrValue::Int(GIB128));
    assert_eq!(unified.attrs["free"], AttrValue::Int(3_100 * 16_384));

    let gpu = one("accel.gpu.metal");
    assert_eq!(gpu.provenance, Provenance::Probed);
    assert_eq!(gpu.attrs["unified"], AttrValue::Bool(true));
    assert_eq!(gpu.attrs["cores"], AttrValue::Int(40));
    assert_eq!(gpu.attrs["sdk_version"], AttrValue::from("4"));

    let ane = one("accel.npu.ane");
    assert_eq!(
        ane.provenance,
        Provenance::Claimed,
        "ANE is inferred, never probed"
    );

    let docker = one("runtime.container.docker");
    assert_eq!(docker.attrs["variant"], AttrValue::from("orbstack"));
    assert_eq!(strs(docker.attrs.get("arches_native")), vec!["aarch64"]);
    let emu = strs(docker.attrs.get("arches_emulated"));
    assert!(emu.contains(&"armv7".to_string()), "{emu:?}");
    assert!(emu.contains(&"x86_64".to_string()));
    assert!(!emu.contains(&"aarch64".to_string()));

    let apple = one("runtime.container.apple");
    assert_eq!(strs(apple.attrs.get("arches_native")), vec!["aarch64"]);
    assert_eq!(apple.state, CapabilityState::Available);
    assert_eq!(apple.attrs["version"], AttrValue::from("1.0.0"));

    assert_eq!(
        one("runtime.infer.llamacpp").attrs["version"],
        AttrValue::from("9820")
    );
    assert!(f.find("runtime.infer.mlx-lm").next().is_some());
    assert_eq!(
        one("runtime.infer.ollama").attrs["version"],
        AttrValue::from("0.30.11")
    );
    assert!(f.find("format.gguf").next().is_some());
    assert_eq!(
        one("format.coreml").provenance,
        Provenance::Claimed,
        "a format known only via the claimed ANE is claimed"
    );
    assert_eq!(one("format.gguf").provenance, Provenance::Probed);

    let ext: Vec<_> = f.find("store.tier.external").collect();
    assert_eq!(ext.len(), 1, "the root alias is not an external drive");
    assert_eq!(ext[0].attrs["mounted"], AttrValue::Bool(true));
    // No Rosetta on this host: the native runtime claims no emulation.
    assert!(strs(one("runtime.native").attrs.get("arches_emulated")).is_empty());
    // No vendor tools: nothing else is assumed.
    assert!(f.find("accel.gpu.cuda").next().is_none());
    assert!(f.find("accel.tpu.coral").next().is_none());
}

#[test]
fn without_a_local_probe_image_armv7_is_not_claimed() {
    let mut h = mac();
    h.outputs
        .remove("docker image inspect --format {{.Id}} alpine:3.20");
    let c = probe_capabilities(&h, &ProbeConfig::default());
    let docker = c
        .caps
        .iter()
        .find(|c| c.id.as_str() == "runtime.container.docker")
        .unwrap();
    let emu = strs(docker.attrs.get("arches_emulated"));
    assert!(
        !emu.contains(&"armv7".to_string()),
        "buildx alone does not show arm/v7"
    );
    assert!(emu.contains(&"x86_64".to_string()));
    assert!(
        c.notes
            .iter()
            .any(|n| n.note.contains("probe image not present"))
    );
}

#[test]
fn unreachable_engine_and_stopped_container_service_are_degraded() {
    let mut h = mac();
    h.outputs.remove("docker info --format {{json .}}");
    h.outputs.remove("container system status");
    let c = probe_capabilities(&h, &ProbeConfig::default());
    let state = |id: &str| c.caps.iter().find(|c| c.id.as_str() == id).unwrap().state;
    assert_eq!(state("runtime.container.docker"), CapabilityState::Degraded);
    assert_eq!(state("runtime.container.apple"), CapabilityState::Degraded);
}

#[test]
fn rosetta_adds_x86_64_emulation_to_native_and_apple_container() {
    let h = mac().path("/Library/Apple/usr/share/rosetta/rosetta");
    let c = probe_capabilities(&h, &ProbeConfig::default());
    for id in ["runtime.native", "runtime.container.apple"] {
        let cap = c.caps.iter().find(|c| c.id.as_str() == id).unwrap();
        assert_eq!(
            strs(cap.attrs.get("arches_emulated")),
            vec!["x86_64"],
            "{id}"
        );
    }
}

#[test]
fn image_refs_are_validated_before_they_reach_docker() {
    use super::probe::valid_image_ref;
    for ok in ["alpine:3.20", "registry.local:5000/team/probe:1", "a@sha256:abcd", "x"] {
        assert!(valid_image_ref(ok), "{ok}");
    }
    for bad in ["", "-v", "--privileged", "a b", "a;b", "a\nb", "$(x)", "/abs", ".hidden", &"a".repeat(256)] {
        assert!(!valid_image_ref(bad), "{bad:?}");
    }
    // A bad configured image is never run.
    let h = mac();
    let cfg = ProbeConfig { docker_probe_image: Some("--privileged".into()), ..Default::default() };
    probe_capabilities(&h, &cfg);
    assert!(h.log.lock().unwrap().iter().all(|c| !c.contains("--privileged") || !c.starts_with("docker run")));
}

#[test]
fn the_privileged_emulation_probe_runs_once_per_ttl() {
    use super::probe::EmulationCache;
    let h = mac();
    let cfg = ProbeConfig {
        emulation_cache: Some(std::sync::Arc::new(EmulationCache::new(std::time::Duration::from_secs(3600)))),
        ..Default::default()
    };
    let runs = |h: &FakeHost| h.log.lock().unwrap().iter().filter(|c| c.starts_with("docker run")).count();
    let first = probe_capabilities(&h, &cfg);
    let second = probe_capabilities(&h, &cfg);
    assert_eq!(runs(&h), 1, "second probe reuses the cached listing");
    let emu = |c: &Collected| strs(c.caps.iter().find(|x| x.id.as_str() == "runtime.container.docker").unwrap().attrs.get("arches_emulated"));
    assert_eq!(emu(&first), emu(&second));
    // Without a cache every probe runs it.
    let h2 = mac();
    probe_capabilities(&h2, &ProbeConfig::default());
    probe_capabilities(&h2, &ProbeConfig::default());
    assert_eq!(runs(&h2), 2);
    // An expired entry is run again.
    let h3 = mac();
    let short = ProbeConfig {
        emulation_cache: Some(std::sync::Arc::new(EmulationCache::new(std::time::Duration::ZERO))),
        ..Default::default()
    };
    probe_capabilities(&h3, &short);
    probe_capabilities(&h3, &short);
    assert_eq!(runs(&h3), 2);
}

#[test]
fn the_live_refresh_rereads_only_free_memory() {
    let h = mac();
    let facts = build_facts("n", 1_000, 600, 1, probe_capabilities(&h, &ProbeConfig::default()));
    let live_host = FakeHost::new("macos", "aarch64").out("vm_stat", "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free: 500.\nPages inactive: 400.\nPages speculative: 100.\n");
    let live = super::probe::refresh_live(&live_host, &facts, 1_030);
    assert_eq!(live.issued_at, 1_030);
    let free = |f: &NodeFacts| f.find("mem.unified").next().unwrap().attrs["free"].clone();
    assert_eq!(free(&live), AttrValue::Int(1_000 * 16_384));
    assert_ne!(free(&live), free(&facts));
    // Everything else is the base, unchanged, and no other command ran.
    assert_eq!(live.capabilities.len(), facts.capabilities.len());
    assert_eq!(live_host.log.lock().unwrap().as_slice(), ["vm_stat"]);
}
