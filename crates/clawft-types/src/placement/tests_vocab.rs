//! Vocabulary: warnings only, never gating; patterns; governance digest pin.

use super::capability::{Capability, CapabilityId, Provenance};
use super::perf;
use super::requirement::{AttrPredicate as P, Requirement};
use super::vocabulary::{VocabWarning, Vocabulary, VocabularyError, digest_of};

const SMALL: &str = r#"
[meta]
version = 3
title = "test vocabulary"

[families.accel]
title = "Accelerators"
order = 2

[families.perf]
title = "Perf"
order = 1

[ids."accel.gpu.metal"]
summary = "Metal | GPU"
attrs = { unified = "bool", mem_bytes = "int", formats = "list" }

[ids."accel.tsu.<vendor>"]
summary = "TSU"

[ids."perf.cog.cycle_ms"]
summary = "cycle"
attrs = { cog_id = "string", value = "number" }
min_provenance = "measured"
"#;

fn vocab() -> Vocabulary {
    Vocabulary::from_toml_str(SMALL).unwrap()
}
fn cap(id: &str, p: Provenance) -> Capability {
    Capability::new(CapabilityId::new(id).unwrap(), p)
}

#[test]
fn unknown_ids_warn_but_still_match() {
    let v = vocab();
    let c = cap("sensor.lidar.velodyne", Provenance::Probed);
    assert_eq!(
        v.validate_capability(&c),
        vec![VocabWarning::UnknownId("sensor.lidar.velodyne".into())]
    );
    let r = Requirement::exact(c.id.clone());
    assert_eq!(v.validate_requirement(&r).len(), 1);
    assert!(
        r.matches(std::slice::from_ref(&c)),
        "vocabulary must not gate matching"
    );
}

#[test]
fn experimental_ids_draw_no_warning() {
    let v = vocab();
    assert!(
        v.validate_capability(&cap("x.acme.widget", Provenance::Claimed))
            .is_empty()
    );
    assert!(
        v.validate_requirement(&Requirement::prefix("x.acme").unwrap())
            .is_empty()
    );
    assert_eq!(
        v.validate_requirement(&Requirement::prefix("sensor").unwrap()),
        vec![VocabWarning::UnknownPrefix("sensor".into())]
    );
    assert!(
        v.validate_requirement(&Requirement::prefix("accel.gpu").unwrap())
            .is_empty()
    );
}

#[test]
fn placeholder_patterns_cover_vendor_ids() {
    let v = vocab();
    assert!(v.lookup("accel.tsu.extropic").is_some());
    assert!(v.lookup("accel.tsu.extropic.v2").is_none());
    assert!(
        v.validate_capability(&cap("accel.tsu.extropic", Provenance::Probed))
            .is_empty()
    );
}

#[test]
fn attribute_type_and_provenance_warnings() {
    let v = vocab();
    let c = cap("accel.gpu.metal", Provenance::Probed)
        .with_attr("unified", "yes")
        .with_attr("extra", 1i64);
    assert_eq!(
        v.validate_capability(&c),
        vec![VocabWarning::AttrType {
            id: "accel.gpu.metal".into(),
            attr: "unified".into(),
            expected: "bool"
        }]
    );
    let mut claimed = perf::cog_cycle_ms("fall-detect", 900.0).unwrap();
    claimed.provenance = Provenance::Probed;
    assert_eq!(
        v.validate_capability(&claimed),
        vec![VocabWarning::Provenance {
            id: "perf.cog.cycle_ms".into(),
            expected: Provenance::Measured
        }]
    );
    assert!(
        v.validate_capability(&perf::cog_cycle_ms("fall-detect", 900.0).unwrap())
            .is_empty()
    );
    let r = Requirement::exact(CapabilityId::new("accel.gpu.metal").unwrap())
        .with_where(P::gte("formats", 1.0));
    assert_eq!(v.validate_requirement(&r).len(), 1);
    let ok = Requirement::exact(CapabilityId::new("accel.gpu.metal").unwrap())
        .with_where(P::has("formats", "gguf"));
    assert!(v.validate_requirement(&ok).is_empty());
}

#[test]
fn malformed_vocabulary_files_are_rejected() {
    let cases = [
        SMALL.replace("[families.perf]", "[families.PERF]"),
        SMALL.replace("\"accel.tsu.<vendor>\"", "\"gpu.tsu.x\""),
        SMALL.replace("\"accel.tsu.<vendor>\"", "\"accel.tsu.<Vendor>\""),
        SMALL.replace("unified = \"bool\"", "unified = \"boolean\""),
        SMALL.replace("title = \"test vocabulary\"", "title = \"t\"\nextra = 1"),
        SMALL
            .replace("[families.perf]", "[families.x]")
            .replace("perf.cog", "x.cog"),
    ];
    for (i, text) in cases.iter().enumerate() {
        assert!(
            Vocabulary::from_toml_str(text).is_err(),
            "case {i} should be rejected"
        );
    }
    let huge = "#".repeat(super::vocabulary::MAX_VOCAB_BYTES + 1);
    assert!(matches!(
        Vocabulary::from_toml_str(&huge),
        Err(VocabularyError::TooLarge(_))
    ));
}

#[test]
fn governance_pin_refuses_free_edits() {
    let pinned = digest_of(SMALL);
    assert_eq!(pinned.len(), 64);
    assert_eq!(
        Vocabulary::from_toml_pinned(SMALL, &pinned)
            .unwrap()
            .digest(),
        pinned
    );
    let edited = SMALL.replace("summary = \"TSU\"", "summary = \"TSU edited\"");
    assert!(matches!(
        Vocabulary::from_toml_pinned(&edited, &pinned),
        Err(VocabularyError::DigestMismatch { .. })
    ));
}

#[test]
fn governed_change_must_bump_version() {
    let cur = vocab();
    let same_version =
        Vocabulary::from_toml_str(&SMALL.replace("summary = \"TSU\"", "summary = \"T\"")).unwrap();
    assert!(matches!(
        cur.change_to(&same_version),
        Err(VocabularyError::VersionNotIncreased { from: 3, to: 3 })
    ));
    let next = Vocabulary::from_toml_str(&SMALL.replace("version = 3", "version = 4")).unwrap();
    let ch = cur.change_to(&next).unwrap();
    assert_eq!((ch.from_version, ch.to_version), (3, 4));
    assert_eq!(ch.from_digest, cur.digest());
    assert_eq!(ch.to_digest, next.digest());
    assert_ne!(ch.from_digest, ch.to_digest);
}

#[test]
fn markdown_is_grouped_by_family_order_and_escaped() {
    let md = vocab().to_markdown();
    let perf_at = md.find("## Perf (`perf`)").unwrap();
    let accel_at = md.find("## Accelerators (`accel`)").unwrap();
    assert!(perf_at < accel_at, "family order must drive section order");
    assert!(md.contains("| `accel.gpu.metal` | Metal \\| GPU |"));
    assert!(md.contains(
        "| `perf.cog.cycle_ms` | cycle | `cog_id`: string, `value`: number | measured |"
    ));
    assert!(md.contains(&digest_of(SMALL)));
}
