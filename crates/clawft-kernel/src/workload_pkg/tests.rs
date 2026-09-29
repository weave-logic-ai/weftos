//! Acceptance tests for mesh-placement-07: pack anomaly-detect for aarch64 +
//! armv7, sign, verify, and the distinct negative cases.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::codec::hex_encode;
use super::cognitum::{PROVENANCE_SCHEMA, canonical_statement};
use super::*;
use crate::artifact_store::ArtifactStore;

/// Trimmed anomaly-detect `cog.toml` (same `[cog]` identity as upstream).
const COG_TOML: &str = r#"[cog]
id = "anomaly-detect"
name = "Anomaly Detection"
version = "1.2.0"
category = "health"
binary = "cog-anomaly-detect-arm"
hardware_requirement = ["pi-zero-2w", "v0-appliance"]

[console]
allowed_commands = ["--once"]
max_runtime_secs = 15
output_limit_bytes = 65536
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    pkg: PathBuf,
}

fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

fn anchors_for(keys: &[&SigningKey]) -> TrustAnchors {
    let mut a = TrustAnchors::default();
    for k in keys {
        let pk = k.verifying_key().to_bytes();
        a.push_signer(&key_id_for(&pk), &hex_encode(&pk), KeyOrigin::Operator)
            .unwrap();
    }
    a
}

fn fixture(record: Option<&Value>) -> Fixture {
    fixture_arches(record, &["aarch64", "armv7"])
}

fn fixture_arches(record: Option<&Value>, arches: &[&str]) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let cog_dir = root.join("vendor/cogs/src/cogs/anomaly-detect");
    std::fs::create_dir_all(&cog_dir).unwrap();
    std::fs::write(cog_dir.join("cog.toml"), COG_TOML).unwrap();
    std::fs::write(root.join("a64"), b"\x7fELF aarch64 anomaly-detect").unwrap();
    std::fs::write(root.join("a32"), b"\x7fELF armv7 anomaly-detect").unwrap();
    let cognitum_record = record.map(|r| {
        let p = root.join("record.json");
        std::fs::write(&p, serde_json::to_vec(r).unwrap()).unwrap();
        p
    });
    let input = CogPackInput {
        cog_dir,
        binaries: arches
            .iter()
            .map(|a| {
                (
                    a.to_string(),
                    root.join(if *a == "armv7" { "a32" } else { "a64" }),
                )
            })
            .collect(),
        source: PackageSource {
            repo: Some("cogs-fork".into()),
            commit: Some("8970f99".into()),
            release_url: None,
        },
        cognitum_record,
    };
    let pkg = root.join("pkg");
    let env = pack_cog(&input, &pkg).unwrap();
    write_manifest(&pkg, &env).unwrap();
    Fixture {
        _tmp: tmp,
        root,
        pkg,
    }
}

fn sign_pkg(pkg: &Path, k: &SigningKey) {
    let mut env =
        ManifestEnvelope::from_bytes(&std::fs::read(pkg.join(MANIFEST_FILE)).unwrap()).unwrap();
    sign_envelope(&mut env, k, &key_id_for(&k.verifying_key().to_bytes())).unwrap();
    write_manifest(pkg, &env).unwrap();
}

fn verify(pkg: &Path, anchors: &TrustAnchors) -> Result<VerifiedPackage, VerifyError> {
    verify_dir(pkg, anchors, &VerifyPolicy::default())
}

fn edit_manifest(pkg: &Path, f: impl FnOnce(&mut Value)) {
    let p = pkg.join(MANIFEST_FILE);
    let mut v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    f(&mut v);
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
}

#[test]
fn packing_anomaly_detect_for_two_arches_yields_a_signed_record_that_verifies() {
    let fx = fixture(None);
    let k = key(1);
    sign_pkg(&fx.pkg, &k);

    let v = verify(&fx.pkg, &anchors_for(&[&k])).expect("signed package verifies");
    assert_eq!(v.body.id, "anomaly-detect");
    assert_eq!(v.body.version, "1.2.0");
    let arches: Vec<&str> = v.body.binaries.keys().map(String::as_str).collect();
    assert_eq!(arches, ["aarch64", "armv7"]);
    assert_eq!(v.body.binaries["armv7"].path, "armv7/cog-anomaly-detect");
    assert_eq!(
        v.body.cog_toml.blake3,
        blake3::hash(COG_TOML.as_bytes()).to_hex().to_string()
    );
    assert_eq!(
        std::fs::read_to_string(fx.pkg.join("cog.toml")).unwrap(),
        COG_TOML,
        "cog.toml unmodified"
    );
    assert_eq!(v.signers.len(), 1);
    assert_eq!(v.signers[0].origin, KeyOrigin::Operator);
    assert_eq!(v.envelope.schema, MANIFEST_SCHEMA);
    assert_eq!(v.envelope.kind, KIND_COG);
}

#[test]
fn package_id_is_stable_across_additional_signatures() {
    let fx = fixture(None);
    let before = verify_id(&fx.pkg);
    sign_pkg(&fx.pkg, &key(1));
    sign_pkg(&fx.pkg, &key(2));
    assert_eq!(before, verify_id(&fx.pkg));
    let v = verify(&fx.pkg, &anchors_for(&[&key(1), &key(2)])).unwrap();
    assert_eq!(v.signers.len(), 2);
}

fn verify_id(pkg: &Path) -> String {
    ManifestEnvelope::from_bytes(&std::fs::read(pkg.join(MANIFEST_FILE)).unwrap())
        .unwrap()
        .package_id()
        .unwrap()
}

#[test]
fn tampered_binary_fails_as_file_tampered() {
    let fx = fixture(None);
    let k = key(1);
    sign_pkg(&fx.pkg, &k);
    std::fs::write(
        fx.pkg.join("armv7/cog-anomaly-detect"),
        b"\x7fELF armv7 anomaly-detecT",
    )
    .unwrap();
    let err = verify(&fx.pkg, &anchors_for(&[&k])).unwrap_err();
    assert!(
        matches!(&err, VerifyError::HashMismatch { path, .. } if path == "armv7/cog-anomaly-detect"),
        "{err}"
    );
    assert_eq!(err.code(), "file-tampered");
}

#[test]
fn tampered_manifest_fails_as_bad_signature() {
    let fx = fixture(None);
    let k = key(1);
    sign_pkg(&fx.pkg, &k);
    edit_manifest(&fx.pkg, |v| v["body"]["version"] = json!("9.9.9"));
    let err = verify(&fx.pkg, &anchors_for(&[&k])).unwrap_err();
    assert!(matches!(err, VerifyError::BadSignature { .. }), "{err}");
}

#[test]
fn wrong_signer_fails_as_untrusted_signer() {
    let fx = fixture(None);
    sign_pkg(&fx.pkg, &key(7));
    let err = verify(&fx.pkg, &anchors_for(&[&key(1)])).unwrap_err();
    assert!(matches!(err, VerifyError::UntrustedSigner { .. }), "{err}");
}

#[test]
fn missing_signature_fails_as_missing_signature() {
    let fx = fixture(None);
    let err = verify(&fx.pkg, &anchors_for(&[&key(1)])).unwrap_err();
    assert_eq!(err, VerifyError::MissingSignature);
}

#[test]
fn negative_cases_have_distinct_error_codes() {
    let k = key(1);
    let anchors = anchors_for(&[&k]);
    let mut codes = BTreeSet::new();

    let fx = fixture(None);
    codes.insert(verify(&fx.pkg, &anchors).unwrap_err().code()); // missing
    sign_pkg(&fx.pkg, &key(9));
    codes.insert(verify(&fx.pkg, &anchors).unwrap_err().code()); // wrong signer
    sign_pkg(&fx.pkg, &k);
    std::fs::write(fx.pkg.join("aarch64/cog-anomaly-detect"), b"evil").unwrap();
    codes.insert(verify(&fx.pkg, &anchors).unwrap_err().code()); // tampered file
    edit_manifest(&fx.pkg, |v| v["body"]["id"] = json!("anomaly-detecx"));
    codes.insert(verify(&fx.pkg, &anchors).unwrap_err().code()); // tampered manifest
    assert_eq!(codes.len(), 4, "{codes:?}");
}

#[test]
fn pinned_key_under_a_different_key_id_is_rejected() {
    let fx = fixture(None);
    let k = key(1);
    let mut env =
        ManifestEnvelope::from_bytes(&std::fs::read(fx.pkg.join(MANIFEST_FILE)).unwrap()).unwrap();
    sign_envelope(&mut env, &k, "someone-else").unwrap();
    write_manifest(&fx.pkg, &env).unwrap();
    assert!(matches!(
        verify(&fx.pkg, &anchors_for(&[&k])),
        Err(VerifyError::BadSignature { .. })
    ));
}

#[test]
fn trusted_signature_is_not_masked_by_an_untrusted_one() {
    let fx = fixture(None);
    sign_pkg(&fx.pkg, &key(9));
    sign_pkg(&fx.pkg, &key(1));
    assert!(verify(&fx.pkg, &anchors_for(&[&key(1)])).is_ok());
}

#[test]
fn missing_file_and_unsafe_paths_are_rejected() {
    let fx = fixture(None);
    let k = key(1);
    sign_pkg(&fx.pkg, &k);
    std::fs::remove_file(fx.pkg.join("cog.toml")).unwrap();
    assert!(matches!(
        verify(&fx.pkg, &anchors_for(&[&k])),
        Err(VerifyError::FileMissing { .. })
    ));

    edit_manifest(&fx.pkg, |v| {
        v["body"]["binaries"]["armv7"]["path"] = json!("../../etc/passwd")
    });
    let err = verify(&fx.pkg, &anchors_for(&[&k])).unwrap_err();
    assert_eq!(err.code(), "manifest-invalid", "{err}");
}

#[test]
fn pack_refuses_non_empty_output_and_unknown_arch() {
    let fx = fixture(None);
    let input = CogPackInput {
        cog_dir: fx.root.join("vendor/cogs/src/cogs/anomaly-detect"),
        binaries: vec![("aarch64".into(), fx.root.join("a64"))],
        source: PackageSource {
            commit: Some("8970f99".into()),
            ..Default::default()
        },
        cognitum_record: None,
    };
    assert!(matches!(
        pack_cog(&input, &fx.pkg),
        Err(PackError::Input(_))
    ));
    let bad = CogPackInput {
        binaries: vec![("mips".into(), fx.root.join("a64"))],
        ..input
    };
    assert!(matches!(
        pack_cog(&bad, &fx.root.join("other")),
        Err(PackError::Input(_))
    ));
}

#[test]
fn stored_package_reverifies_from_a_reopened_file_store() {
    let fx = fixture(None);
    let k = key(1);
    let anchors = anchors_for(&[&k]);
    sign_pkg(&fx.pkg, &k);
    let base = fx.root.join("store");
    let stored = {
        let store = ArtifactStore::new_file(base.clone());
        store_package(&store, &fx.pkg, &anchors, &VerifyPolicy::default()).unwrap()
    };
    assert_eq!(stored.files.len(), 3);

    let reopened = ArtifactStore::open_file(base.clone()).unwrap();
    assert_eq!(reopened.count(), 4);
    let v = verify_stored(
        &reopened,
        &stored.manifest_hash,
        &anchors,
        &VerifyPolicy::default(),
    )
    .unwrap();
    assert_eq!(v.package_id, stored.package_id);

    // Corrupt a stored binary on disk: verification from the store fails.
    let (_, h) = stored
        .files
        .iter()
        .find(|(p, _)| p.starts_with("armv7/"))
        .unwrap();
    std::fs::write(base.join(&h[..2]).join(h), b"corrupt").unwrap();
    let err = verify_stored(
        &reopened,
        &stored.manifest_hash,
        &anchors,
        &VerifyPolicy::default(),
    )
    .unwrap_err();
    assert_eq!(err.code(), "file-tampered", "{err}");
}

#[test]
fn unverified_package_is_never_stored() {
    let fx = fixture(None);
    sign_pkg(&fx.pkg, &key(9));
    let store = ArtifactStore::new_memory();
    let err = store_package(
        &store,
        &fx.pkg,
        &anchors_for(&[&key(1)]),
        &VerifyPolicy::default(),
    )
    .unwrap_err();
    assert!(matches!(err, VerifyError::UntrustedSigner { .. }));
    assert_eq!(store.count(), 0);
}

// ── Cognitum ADR-154/155 release record (optional verifier) ──────────────

#[test]
fn canonical_statement_matches_upstream_python_golden() {
    let rec = json!({"cogId":"anomaly-detect","version":"1.2.0","seededAt":"2026-01-01T00:00:00Z",
        "artifactRef":{"kind":"edge-binary","binaryDigest":format!("sha256:{}", "0".repeat(64)),
            "binaryName":"cog-anomaly-detect-arm64","targetHardware":["v0-appliance"]},
        "rollbackCompatibility":{"compatibleWith":[],"migrationsReversible":true},"stateSchemaVersion":"1",
        "provenance":{"signatureAlgorithm":"ed25519","signingKeyId":"test-key","builtAt":"2026-07-29T00:00:00Z",
            "detachedSignature":{"x":1}},
        "lifecycle":"available","residency":{"allowedRegions":["on-device"]}});
    let st = canonical_statement(&rec).unwrap();
    // Produced by upstream's json.dumps(sort_keys=True, separators=(",", ":")).
    assert_eq!(
        hex_encode(&Sha256::digest(&st)),
        "bae105e1f1c7adaf1ffb45ffe876f200ca9e31acddcd59a9a7e356487a403a0e"
    );
}

fn b64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut s = String::new();
    for c in bytes.chunks(3) {
        let n = c.iter().fold(0u32, |acc, b| (acc << 8) | *b as u32) << (8 * (3 - c.len()));
        for i in 0..=c.len() {
            s.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    s
}

/// A release record for `binary`, signed like upstream with `k`.
fn cognitum_record(k: &SigningKey, binary: &[u8]) -> Value {
    let mut rec = json!({"cogId":"anomaly-detect","version":"1.2.0",
        "artifactRef":{"kind":"edge-binary","binaryDigest":format!("sha256:{}", hex_encode(&Sha256::digest(binary))),
            "binaryName":"cog-anomaly-detect-arm64","targetHardware":["v0-appliance"]},
        "provenance":{"signatureAlgorithm":"ed25519","signingKeyId":"cogs-release-test"}});
    let st = canonical_statement(&rec).unwrap();
    rec["provenance"]["detachedSignature"] = json!({"schema": PROVENANCE_SCHEMA, "algorithm": "ed25519",
        "keyId": "cogs-release-test", "payloadDigest": format!("sha256:{}", hex_encode(&Sha256::digest(&st))),
        "signature": b64url(&k.sign(&st).to_bytes())});
    rec
}

fn cognitum_anchors(k: &SigningKey) -> TrustAnchors {
    let file = TrustFile {
        schema: trust::TRUST_FILE_SCHEMA.into(),
        operator_keys: vec![],
        cognitum_release_keys: vec![trust::TrustFileKey {
            key_id: "cogs-release-test".into(),
            public_key: hex_encode(k.verifying_key().as_bytes()),
        }],
    };
    TrustAnchors::from_trust_file(&file).unwrap()
}

#[test]
fn cognitum_record_counts_as_signature_only_when_enabled() {
    let ck = key(42);
    let fx = fixture_arches(
        Some(&cognitum_record(&ck, b"\x7fELF aarch64 anomaly-detect")),
        &["aarch64"],
    );
    let anchors = cognitum_anchors(&ck);
    assert_eq!(
        verify(&fx.pkg, &anchors).unwrap_err(),
        VerifyError::MissingSignature
    );
    let on = VerifyPolicy {
        accept_cognitum_release: true,
    };
    let v = verify_dir(&fx.pkg, &anchors, &on).unwrap();
    assert_eq!(v.signers[0].origin, KeyOrigin::CognitumRelease);
}

#[test]
fn cognitum_record_rejected_when_unbound_forged_or_unpinned() {
    let ck = key(42);
    let on = VerifyPolicy {
        accept_cognitum_release: true,
    };

    // Record binds aarch64 only; the unbound armv7 binary must not ride along.
    let partial = fixture(Some(&cognitum_record(
        &ck,
        b"\x7fELF aarch64 anomaly-detect",
    )));
    let err = verify_dir(&partial.pkg, &cognitum_anchors(&ck), &on).unwrap_err();
    assert_eq!(err.code(), "cognitum-record-rejected", "{err}");

    let unbound = fixture(Some(&cognitum_record(&ck, b"some other binary")));
    let err = verify_dir(&unbound.pkg, &cognitum_anchors(&ck), &on).unwrap_err();
    assert_eq!(err.code(), "cognitum-record-rejected", "{err}");

    let mut forged = cognitum_record(&ck, b"\x7fELF aarch64 anomaly-detect");
    forged["lifecycle"] = json!("withdrawn"); // not bound by us, only by the signature
    let fx = fixture_arches(Some(&forged), &["aarch64"]);
    assert_eq!(
        verify_dir(&fx.pkg, &cognitum_anchors(&ck), &on)
            .unwrap_err()
            .code(),
        "cognitum-record-rejected"
    );

    let good = fixture_arches(
        Some(&cognitum_record(&ck, b"\x7fELF aarch64 anomaly-detect")),
        &["aarch64"],
    );
    assert_eq!(
        verify_dir(&good.pkg, &cognitum_anchors(&key(43)), &on)
            .unwrap_err()
            .code(),
        "cognitum-record-rejected"
    );
}
