//! The committed vocabulary file (`config/capabilities.toml`) and the doc
//! table generated from it (card mesh-placement-02).
//!
//! `vocabulary_doc_is_current` fails when the committed table is stale.
//! Regenerate with `WEFTOS_REGEN_DOCS=1 scripts/build.sh test clawft-types`.

use std::path::PathBuf;

use clawft_types::placement::{Capability, CapabilityId, Provenance, Vocabulary};

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn load() -> Vocabulary {
    let text =
        std::fs::read_to_string(repo_path("config/capabilities.toml")).expect("read vocabulary");
    Vocabulary::from_toml_str(&text).expect("committed vocabulary parses")
}

#[test]
fn committed_vocabulary_covers_the_card_families() {
    let v = load();
    for fam in [
        "cpu", "os", "runtime", "accel", "format", "mem", "feed", "store", "trust", "perf",
    ] {
        assert!(v.families().contains_key(fam), "family {fam} missing");
    }
    for class in [
        "accel.gpu.",
        "accel.npu.",
        "accel.tpu.",
        "accel.tsu.",
        "accel.other.",
    ] {
        assert!(
            v.ids().keys().any(|k| k.starts_with(class)),
            "accelerator class {class} missing"
        );
    }
    for id in [
        "accel.gpu.metal",
        "accel.npu.ane",
        "accel.tpu.coral",
        "mem.unified",
        "mem.system",
        "mem.vram",
        "runtime.container.docker",
        "feed.esp32-csi-udp",
        "perf.cog.cycle_ms",
        "perf.infer.tok_s",
        "trust.tier.pinned",
        "format.gguf",
    ] {
        assert!(v.lookup(id).is_some(), "{id} missing");
    }
    assert_eq!(
        v.lookup("perf.infer.tok_s").unwrap().min_provenance,
        Some(Provenance::Measured)
    );
    assert!(v.lookup("accel.tsu.extropic").is_some());
    assert!(v.lookup("accel.other.fpga").is_some());
}

#[test]
fn committed_vocabulary_does_not_gate_unknown_ids() {
    let v = load();
    let c = Capability::new(
        CapabilityId::new("sensor.lidar.velodyne").unwrap(),
        Provenance::Probed,
    );
    assert_eq!(v.validate_capability(&c).len(), 1);
    let x = Capability::new(
        CapabilityId::new("x.hw.radar").unwrap(),
        Provenance::Claimed,
    );
    assert!(v.validate_capability(&x).is_empty());
}

#[test]
fn vocabulary_doc_is_current() {
    let generated = load().to_markdown();
    let doc = repo_path("docs/reference/capability-vocabulary.md");
    if std::env::var_os("WEFTOS_REGEN_DOCS").is_some() {
        std::fs::write(&doc, &generated).expect("write generated doc");
    }
    let committed = std::fs::read_to_string(&doc).unwrap_or_default();
    assert!(
        committed == generated,
        "docs/reference/capability-vocabulary.md is stale; regenerate with \
         WEFTOS_REGEN_DOCS=1 scripts/build.sh test clawft-types"
    );
}
