//! Optional Cognitum ADR-154/155 release-record verifier (COG-001 section
//! 6.2). Off unless [`super::VerifyPolicy::accept_cognitum_release`] is set.
//!
//! The record's detached signature is Ed25519 (PureEdDSA) over the canonical
//! statement `{"release":<record minus seededAt and
//! provenance.detachedSignature>,"schema":"cognitum.cog.release-provenance.v1"}`
//! with `payloadDigest = sha256:<hex of that statement>`, matching upstream
//! `scripts/cog_release_provenance_lib.py::canonical_payload`.
//!
//! Scope: the operator pins the Cognitum release key directly by key id.
//! The upstream trust-registry v3 quorum, key status windows and builder /
//! workflow allow-lists are not re-evaluated here. A record binds the cog
//! id, version and one binary digest; it does not cover `cog.toml`, so a
//! record-only package runs with an unsigned manifest unless an operator
//! also signs the envelope.

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::canonical::canonical_json;
use super::codec::{base64_decode, hex_encode, is_lower_hex};
use super::manifest::CogPackageBody;
use super::trust::{PinnedKey, TrustAnchors};
use super::verify::{AcceptedSigner, FileSource, VerifyError, check_file};

/// Attestation kind carried in a cog package for a Cognitum record.
pub const COGNITUM_RECORD_KIND: &str = "cognitum.cog.release-record.v1";
/// Upstream statement / envelope schema.
pub const PROVENANCE_SCHEMA: &str = "cognitum.cog.release-provenance.v1";
/// Upstream canonical statement size limit.
const MAX_STATEMENT_BYTES: usize = 64 * 1024;

/// Facts a verified record binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordFacts {
    /// `cogId`.
    pub cog_id: String,
    /// `version`.
    pub version: String,
    /// `artifactRef.binaryDigest` (`sha256:<hex>`).
    pub binary_digest: String,
    /// Signing key id.
    pub key_id: String,
}

/// Canonical upstream statement bytes for `record`.
pub fn canonical_statement(record: &Value) -> Result<Vec<u8>, String> {
    let mut unsigned = record
        .as_object()
        .ok_or("record must be an object")?
        .clone();
    unsigned.remove("seededAt");
    let prov = unsigned
        .get_mut("provenance")
        .and_then(Value::as_object_mut)
        .ok_or("record.provenance must be an object")?;
    prov.remove("detachedSignature");
    let statement =
        serde_json::json!({ "schema": PROVENANCE_SCHEMA, "release": Value::Object(unsigned) });
    let bytes = canonical_json(&statement).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_STATEMENT_BYTES {
        return Err("canonical release statement is too large".into());
    }
    Ok(bytes)
}

/// Verify a release record against pinned Cognitum keys.
pub fn verify_release_record(record: &Value, keys: &[PinnedKey]) -> Result<RecordFacts, String> {
    require_printable_ascii(record, 0)?;
    let env = record
        .pointer("/provenance/detachedSignature")
        .and_then(Value::as_object)
        .ok_or("provenance.detachedSignature missing")?;
    let mut fields: Vec<&str> = env.keys().map(String::as_str).collect();
    fields.sort_unstable();
    if fields != ["algorithm", "keyId", "payloadDigest", "schema", "signature"] {
        return Err("detachedSignature has unexpected fields".into());
    }
    let field = |k: &str| {
        env.get(k)
            .and_then(Value::as_str)
            .ok_or(format!("detachedSignature.{k} must be a string"))
    };
    if field("schema")? != PROVENANCE_SCHEMA || field("algorithm")? != "ed25519" {
        return Err("detachedSignature schema/algorithm mismatch".into());
    }
    let key_id = field("keyId")?;
    let key = keys
        .iter()
        .find(|k| k.key_id == key_id)
        .ok_or_else(|| format!("release key {key_id} is not pinned"))?;
    let sig_text = field("signature")?;
    if sig_text.len() != 86 {
        return Err("signature must be 86 base64url chars".into());
    }
    let sig: [u8; 64] = base64_decode(sig_text)
        .and_then(|v| v.try_into().ok())
        .ok_or("signature is not 64-byte base64url")?;

    let statement = canonical_statement(record)?;
    let digest = format!("sha256:{}", hex_encode(&Sha256::digest(&statement)));
    if field("payloadDigest")? != digest {
        return Err("payloadDigest does not match the canonical statement".into());
    }
    let vk = ed25519_dalek::VerifyingKey::from_bytes(&key.public_key).map_err(|e| e.to_string())?;
    vk.verify_strict(&statement, &ed25519_dalek::Signature::from_bytes(&sig))
        .map_err(|_| format!("signature from {key_id} does not verify"))?;

    let text = |p: &str| {
        record
            .pointer(p)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or(format!("record{p} missing"))
    };
    let binary_digest = text("/artifactRef/binaryDigest")?;
    if !(binary_digest.starts_with("sha256:") && is_lower_hex(&binary_digest[7..], 64)) {
        return Err("artifactRef.binaryDigest must be sha256:<64 hex>".into());
    }
    Ok(RecordFacts {
        cog_id: text("/cogId")?,
        version: text("/version")?,
        binary_digest,
        key_id: key_id.to_string(),
    })
}

fn require_printable_ascii(v: &Value, depth: usize) -> Result<(), String> {
    if depth > 16 {
        return Err("record nesting too deep".into());
    }
    let ok = |s: &str| s.bytes().all(|b| (0x20..=0x7e).contains(&b));
    match v {
        Value::String(s) if !ok(s) => Err("record strings must be printable ASCII".into()),
        Value::Array(a) => a
            .iter()
            .try_for_each(|x| require_printable_ascii(x, depth + 1)),
        Value::Object(m) => m.iter().try_for_each(|(k, x)| {
            if ok(k) {
                require_printable_ascii(x, depth + 1)
            } else {
                Err("record keys must be printable ASCII".into())
            }
        }),
        _ => Ok(()),
    }
}

/// If the package carries a Cognitum record, verify it and its binding to
/// the package (cog id, version, and every binary's SHA-256 equal to
/// `binaryDigest`). `Ok(None)` when no record is attached.
pub fn accept_from_package(
    body: &CogPackageBody,
    source: &dyn FileSource,
    anchors: &TrustAnchors,
) -> Result<Option<AcceptedSigner>, VerifyError> {
    let Some(att) = body
        .attestations
        .iter()
        .find(|a| a.kind == COGNITUM_RECORD_KIND)
    else {
        return Ok(None);
    };
    let reject = VerifyError::CognitumRecord;
    if anchors.cognitum.is_empty() {
        return Err(reject("no Cognitum release key is pinned".into()));
    }
    if att.file.size > 256 * 1024 {
        return Err(reject("release record larger than 256 KiB".into()));
    }
    let bytes = source.read(&att.file)?;
    check_file(&att.file, &bytes)?;
    let record: Value =
        serde_json::from_slice(&bytes).map_err(|e| reject(format!("parse: {e}")))?;
    let facts = verify_release_record(&record, &anchors.cognitum).map_err(reject)?;
    if facts.cog_id != body.id || facts.version != body.version {
        return Err(reject(format!(
            "record is for {}@{}, package is {}@{}",
            facts.cog_id, facts.version, body.id, body.version
        )));
    }
    // The record binds exactly one binary and nothing else in the package,
    // so record-only trust requires every binary to be that binary: an
    // extra, unbound arch would otherwise ride along unsigned.
    for bin in body.binaries.values() {
        let content = source.read(bin)?;
        check_file(bin, &content)?;
        if format!("sha256:{}", hex_encode(&Sha256::digest(&content))) != facts.binary_digest {
            return Err(reject(format!(
                "record binaryDigest does not cover {}; record-only trust needs every binary bound",
                bin.path
            )));
        }
    }
    Ok(Some(AcceptedSigner {
        key_id: facts.key_id,
        origin: super::trust::KeyOrigin::CognitumRelease,
    }))
}
