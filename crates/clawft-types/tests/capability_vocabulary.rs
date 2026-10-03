//! The committed vocabulary file (`config/capabilities.toml`) and the doc
//! table generated from it (card mesh-placement-02).
//!
//! The vocabulary loads only through its governance pin
//! (`config/capabilities.pin.toml`, ADR-099 decision 6), so a free edit to
//! the vocabulary fails every test here, including doc regeneration.
//! `vocabulary_doc_is_current` fails when the committed table is stale.
//! Regenerate with `WEFTOS_REGEN_DOCS=1 scripts/build.sh test clawft-types`.

use std::path::PathBuf;

use clawft_types::placement::{
    Capability, CapabilityId, Provenance, Vocabulary, VocabularyError, VocabularyPin,
};

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_path(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn pin() -> VocabularyPin {
    VocabularyPin::from_toml_str(&read("config/capabilities.pin.toml")).expect("pin parses")
}

/// The only way the committed vocabulary is loaded: through its pin.
fn load() -> Vocabulary {
    Vocabulary::from_toml_pinned(&read("config/capabilities.toml"), &pin()).expect(
        "config/capabilities.toml does not match its governance pin; vocabulary changes go \
         through the governed config.set path (ADR-099 decision 6), not free edits",
    )
}

#[test]
fn committed_vocabulary_matches_its_governance_pin() {
    let v = load();
    let p = pin();
    // The baseline pin carries the genesis marker; the content it names is
    // the committed vocabulary.
    let mut expected = VocabularyPin::of(&v);
    expected.event_hash = Some("genesis".into());
    assert_eq!(expected, p);
    // The committed pin is exactly what its serializer writes.
    assert_eq!(read("config/capabilities.pin.toml"), p.to_toml_string());
}

#[test]
fn free_edit_to_committed_vocabulary_does_not_load() {
    let text = read("config/capabilities.toml");
    let edited = format!("{text}\n[ids.\"accel.gpu.anything\"]\nsummary = \"added freely\"\n");
    assert!(
        Vocabulary::from_toml_str(&edited).is_ok(),
        "the edit itself is well formed"
    );
    assert!(matches!(
        Vocabulary::from_toml_pinned(&edited, &pin()),
        Err(VocabularyError::DigestMismatch { .. })
    ));
    let bumped = text.replacen("version = 1", "version = 2", 1);
    assert_ne!(bumped, text);
    assert!(matches!(
        Vocabulary::from_toml_pinned(&bumped, &pin()),
        Err(VocabularyError::DigestMismatch { .. })
    ));
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
