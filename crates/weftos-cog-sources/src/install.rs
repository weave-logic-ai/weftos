//! Verified fetch and install of one cog from one source (ADR-105 sections 3-5).
//!
//! Order of checks, always before the binary is written anywhere:
//!
//! - `cognitum`: licence entitlement (refused with `cog_unlicensed` or
//!   `licence_expired` before any download), then sha256 against the hash the
//!   registry lists. Cognitum binaries are not signed, so the install is not
//!   `placement_eligible`: ADR-100 section 6.3 still requires an operator to
//!   hash and sign them into a `cogpkg` before governed placement.
//! - `weftos` / `private`: size, sha256 and the Ed25519 signature against the
//!   keys pinned for that source. Unsigned, signed by another key, or a hash
//!   mismatch are all refused.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use weftos_cog_host::{install_verified, CogRecord, Source as HostSource, VerifiedInstall};
use weftos_cog_repo::{sha256_hex, verify_artifact, Artifact, VerifyError};

use crate::config::{is_weftos_anchor, valid_cog_id, CogLicence, SourceKind};
use crate::error::{Result, SourceError};
use crate::fetch::{join, Reader, MAX_BINARY_BYTES};
use crate::licence::entitlement;
use crate::resolve::{Listing, LoadedSource, SourceCog};

/// File written next to an installed cog recording where it came from.
pub const PROVENANCE_FILE: &str = "provenance.json";

/// Where an installed cog came from and what vouched for it. Written to
/// `<cog dir>/provenance.json` and usable as a chain event payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// Source name (`weftos`, `cognitum`, `acme-private`, ...).
    pub source: String,
    /// `weftos` | `cognitum` | `private`.
    pub kind: String,
    /// Registry location the cog was listed in.
    pub registry: String,
    /// Cog id.
    pub cog_id: String,
    /// Version as listed.
    pub version: String,
    /// Arch installed.
    pub arch: String,
    /// sha256 of the binary, hex.
    pub sha256: String,
    /// `ed25519-signed` or `cognitum-sha256`.
    pub trust: String,
    /// Raw public key that verified the signature (signed sources).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer_pubkey: Option<String>,
    /// `ed25519:` + first 16 hex of sha256(pubkey).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer_key_id: Option<String>,
    /// Licence account that allowed a Cognitum install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence_account: Option<String>,
    /// True when the verifying key is the WeaveLogic release key or a compiled-in
    /// WeftOS signer. Only then may the cog be labelled WeaveLogic on install.
    #[serde(default)]
    pub weftos_anchor: bool,
    /// False for Cognitum binaries until an operator hashes and signs them (ADR-100 6.3).
    pub placement_eligible: bool,
    /// RFC 3339 time of the fetch.
    pub fetched_at: String,
}

impl Provenance {
    /// Chain event payload: kind `cog.source.resolved`. Carries hashes and
    /// key ids only, never a secret.
    pub fn chain_payload(&self) -> serde_json::Value {
        serde_json::json!({ "kind": "cog.source.resolved", "provenance": self })
    }
}

/// What the caller supplies for a fetch.
pub struct FetchCtx<'a> {
    /// Reader for registries and binaries.
    pub reader: &'a dyn Reader,
    /// Effective licences (project first, then user).
    pub licences: &'a [CogLicence],
    /// Clock, injected so expiry is testable.
    pub now: DateTime<Utc>,
    /// Extra keys trusted by `weftos` sources (the compiled-in WeftOS signer set).
    pub extra_weftos_keys: &'a [String],
}

/// A downloaded binary that passed every check.
#[derive(Debug, Clone)]
pub struct Fetched {
    /// Binary bytes.
    pub bytes: Vec<u8>,
    /// Provenance to record.
    pub provenance: Provenance,
    /// The cog as listed.
    pub cog: SourceCog,
}

fn key_id(pubkey_hex: &str) -> String {
    let raw = hex::decode(pubkey_hex).unwrap_or_default();
    let h = hex::encode(Sha256::digest(&raw));
    format!("ed25519:{}", &h[..16])
}

/// Fetch and verify `cog_id` from `loaded` for `arch`.
pub fn fetch_verified(loaded: &LoadedSource, cog_id: &str, arch: &str, ctx: &FetchCtx<'_>) -> Result<Fetched> {
    if !valid_cog_id(cog_id) {
        return Err(SourceError::Config(format!("bad cog id {cog_id:?}")));
    }
    let src = &loaded.source;
    let cog = loaded
        .cog(cog_id)
        .ok_or_else(|| SourceError::NotFound { source_name: Some(src.name.clone()), id: cog_id.into() })?;
    let fetched_at = ctx.now.to_rfc3339();
    match (&loaded.listing, src.kind) {
        (Listing::Cognitum(reg), SourceKind::Cognitum) => {
            // Licence first: an unlicensed install must not even download.
            let lic = entitlement(ctx.licences, &src.name, cog_id, ctx.now)?;
            if arch != "arm" {
                return Err(SourceError::NoArtifact { id: cog_id.into(), arch: arch.into(), have: vec!["arm".into()] });
            }
            let entry = reg.cogs.iter().find(|c| c.id == cog_id).expect("listed above");
            let want = entry.sha256.clone().unwrap_or_default().to_ascii_lowercase();
            if want.len() != 64 || !want.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(SourceError::NoPinnedHash { id: cog_id.into(), source_name: src.name.clone() });
            }
            let url = reg.arm_binary_url(cog_id).ok_or_else(|| SourceError::Fetch {
                location: src.url.clone(),
                msg: "registry has no binary_base_url".into(),
            })?;
            if !src.allow_insecure && !url.starts_with("https://") {
                return Err(SourceError::Insecure { id: cog_id.into(), source_name: src.name.clone(), location: url });
            }
            let bytes = ctx.reader.read(&url, MAX_BINARY_BYTES)?;
            let got = sha256_hex(&bytes);
            if got != want {
                return Err(SourceError::Verify {
                    id: cog_id.into(),
                    source_name: src.name.clone(),
                    reason: format!("sha256 {got} != registry {want}"),
                });
            }
            if let Some(n) = entry.binary_size
                && n != bytes.len() as u64
            {
                return Err(SourceError::Verify {
                    id: cog_id.into(),
                    source_name: src.name.clone(),
                    reason: format!("size {} != registry {n}", bytes.len()),
                });
            }
            Ok(Fetched {
                provenance: Provenance {
                    source: src.name.clone(),
                    kind: src.kind.label().into(),
                    registry: src.url.clone(),
                    cog_id: cog_id.into(),
                    version: cog.version.clone(),
                    arch: arch.into(),
                    sha256: got,
                    trust: "cognitum-sha256".into(),
                    signer_pubkey: None,
                    signer_key_id: None,
                    licence_account: Some(lic.account.clone()),
                    weftos_anchor: false,
                    placement_eligible: false,
                    fetched_at,
                },
                bytes,
                cog,
            })
        }
        (Listing::Signed { base, registry }, SourceKind::Weftos | SourceKind::Private) => {
            let entry = registry.cogs.iter().find(|c| c.id == cog_id).expect("listed above");
            let art: &Artifact = entry.artifacts.get(arch).ok_or_else(|| SourceError::NoArtifact {
                id: cog_id.into(),
                arch: arch.into(),
                have: entry.artifacts.keys().cloned().collect(),
            })?;
            if art.sig.trim().is_empty() {
                return Err(SourceError::Unsigned { id: cog_id.into(), source_name: src.name.clone() });
            }
            let keys = src.effective_keys(ctx.extra_weftos_keys);
            let bytes = ctx.reader.read(&join(base, &art.path), MAX_BINARY_BYTES)?;
            let signer = verify_with_any(&bytes, art, &keys).map_err(|reason| SourceError::Verify {
                id: cog_id.into(),
                source_name: src.name.clone(),
                reason,
            })?;
            let anchor = src.kind == SourceKind::Weftos && is_weftos_anchor(&signer, ctx.extra_weftos_keys);
            Ok(Fetched {
                provenance: Provenance {
                    source: src.name.clone(),
                    kind: src.kind.label().into(),
                    registry: src.url.clone(),
                    cog_id: cog_id.into(),
                    version: cog.version.clone(),
                    arch: arch.into(),
                    sha256: art.sha256.clone(),
                    trust: "ed25519-signed".into(),
                    signer_key_id: Some(key_id(&signer)),
                    signer_pubkey: Some(signer),
                    licence_account: None,
                    weftos_anchor: anchor,
                    placement_eligible: true,
                    fetched_at,
                },
                bytes,
                cog,
            })
        }
        _ => Err(SourceError::Config(format!("source '{}' listing does not match its kind", src.name))),
    }
}

/// Verify against each pinned key; the size and sha256 checks do not depend
/// on the key, so only a signature rejection moves on to the next key.
fn verify_with_any(bytes: &[u8], art: &Artifact, keys_hex: &[String]) -> std::result::Result<String, String> {
    if keys_hex.is_empty() {
        return Err("no key is pinned for this source".into());
    }
    let mut last = String::new();
    for k in keys_hex {
        let raw: [u8; 32] = match hex::decode(k).ok().and_then(|v| v.try_into().ok()) {
            Some(r) => r,
            None => continue,
        };
        let Ok(vk) = ed25519_dalek::VerifyingKey::from_bytes(&raw) else { continue };
        match verify_artifact(bytes, art, &vk) {
            Ok(()) => return Ok(k.clone()),
            Err(VerifyError::SignatureRejected) => last = "Ed25519 signature is not from any key pinned for this source".into(),
            Err(e) => return Err(e.to_string()),
        }
    }
    Err(if last.is_empty() { "no usable pinned key".into() } else { last })
}

/// Write a verified cog into a cog-host root and record its provenance.
///
/// Provenance is written first (temp file + rename), so a cog that is enabled
/// is never without it. A cog is labelled WeaveLogic only when the verifying
/// key is a WeftOS anchor; a signature from any other pinned key is recorded as
/// a local, signed install.
pub fn install_into_host(root: &Path, f: &Fetched, enable: bool, args: &[String]) -> Result<CogRecord> {
    let p = &f.provenance;
    let (source, signed) = match p.trust.as_str() {
        "ed25519-signed" if p.kind == "weftos" && p.weftos_anchor => (HostSource::WeaveLogic, true),
        "ed25519-signed" => (HostSource::Local, true),
        _ => (HostSource::Cognitum, false),
    };
    let io = |path: &Path, e: std::io::Error| SourceError::Io { path: path.display().to_string(), msg: e.to_string() };
    let dir = root.join(&p.cog_id);
    std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
    let json = serde_json::to_vec_pretty(p).map_err(|e| SourceError::Parse { what: "provenance".into(), msg: e.to_string() })?;
    let path = dir.join(PROVENANCE_FILE);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = dir.join(format!("{PROVENANCE_FILE}.{}.{nanos}.tmp", std::process::id()));
    std::fs::write(&tmp, json).map_err(|e| io(&tmp, e))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(&path, e)
    })?;
    install_verified(
        root,
        &VerifiedInstall { id: &p.cog_id, version: &p.version, source, signed, args, enable, bytes: &f.bytes },
    )
    .map_err(|msg| SourceError::Io { path: root.display().to_string(), msg })
}

/// Read the provenance of an installed cog, if recorded.
pub fn read_provenance(root: &Path, cog_id: &str) -> Option<Provenance> {
    serde_json::from_slice(&std::fs::read(root.join(cog_id).join(PROVENANCE_FILE)).ok()?).ok()
}
