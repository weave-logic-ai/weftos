use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::workload_pkg::KIND_COG;

/// Test-only kind that accepts any body.
struct Echo;

impl WorkloadKind for Echo {
    fn id(&self) -> &'static str {
        "echo"
    }
    fn validate(&self, _: &ManifestEnvelope) -> Result<(), ManifestError> {
        Ok(())
    }
    #[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
    fn load(&self, _: &Path, _: &TrustAnchors) -> Result<VerifiedWorkload, PlaneError> {
        Err(PlaneError::Package("echo has no package".into()))
    }
    #[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
    fn spec(&self, _: &VerifiedWorkload) -> Result<WorkloadSpec, String> {
        Err("echo has no spec".into())
    }
    #[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
    fn adapters(&self) -> &'static [&'static str] {
        &["native"]
    }
}

#[test]
fn builtin_registry_has_cog() {
    let r = KindRegistry::builtin();
    assert_eq!(r.ids(), vec![KIND_COG]);
    assert_eq!(r.get(KIND_COG).unwrap().id(), KIND_COG);
    assert!(r.require(KIND_COG).is_ok());
}

#[test]
fn unknown_kind_is_a_structured_error() {
    let r = KindRegistry::builtin();
    assert!(r.get("model").is_none());
    assert_eq!(r.require("model").err(), Some(UnknownKind("model".into())));
    let env = ManifestEnvelope::new("model", json!({}));
    let err = validate_envelope(&r, &env).unwrap_err();
    assert!(matches!(&err, ManifestError::UnknownKind(UnknownKind(k)) if k == "model"));
    assert_eq!(err.to_string(), "unknown workload kind \"model\"");
}

#[test]
fn registering_a_kind_makes_it_resolvable() {
    let mut r = KindRegistry::builtin();
    r.register(Arc::new(Echo)).unwrap();
    assert_eq!(r.ids(), vec![KIND_COG, "echo"]);
    let env = ManifestEnvelope::new("echo", json!({"anything": 1}));
    assert!(validate_envelope(&r, &env).is_ok());
    // The builtin registry stays untouched (no global state).
    assert!(KindRegistry::builtin().get("echo").is_none());
}

#[test]
fn duplicate_ids_are_rejected() {
    let mut r = KindRegistry::builtin();
    assert_eq!(
        r.register(Arc::new(CogKind)).err(),
        Some(DuplicateKind(KIND_COG.into()))
    );
    r.register(Arc::new(Echo)).unwrap();
    assert!(r.register(Arc::new(Echo)).is_err());
}

#[test]
fn cog_kind_rejects_a_malformed_body_like_cog_body() {
    let r = KindRegistry::builtin();
    let env = ManifestEnvelope::new(KIND_COG, json!({"nope": true}));
    let via_registry = validate_envelope(&r, &env).unwrap_err().to_string();
    assert_eq!(via_registry, env.cog_body().unwrap_err().to_string());
}

#[test]
fn peek_reads_the_declared_kind() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(peek_manifest_kind(dir.path()), None);
    std::fs::write(
        dir.path().join(crate::workload_pkg::MANIFEST_FILE),
        r#"{"kind":"echo"}"#,
    )
    .unwrap();
    assert_eq!(peek_manifest_kind(dir.path()).as_deref(), Some("echo"));
    std::fs::write(
        dir.path().join(crate::workload_pkg::MANIFEST_FILE),
        "not json",
    )
    .unwrap();
    assert_eq!(peek_manifest_kind(dir.path()), None);
}
