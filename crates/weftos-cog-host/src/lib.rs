//! weft-cog-host (COG-009): the on-appliance cog store + record model.
//!
//! A host root (default `~/.weftos/cogs`) holds one directory per cog:
//! `<root>/<id>/{cog-<id>-arm, manifest.json, cog.json}`. `cog.json` is our record — where the cog
//! came from, its version, whether it should be running (`enabled`), and its args. The agent's own
//! `/var/lib/cognitum/apps` is untouched; this is a separate, uncapped lifecycle that runs cogs which
//! still POST to the agent store on `:80`.

pub mod auth;
pub mod dex;
pub mod fleet;
pub mod hw_http;
pub mod introspect;
pub mod licence;
pub mod mesh;
pub mod mesh_keys;
pub mod network;
pub mod proc;
pub mod supervise;
pub mod usb;
pub mod usb_identify;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use weftos_cog_repo::{sha256_hex, verify_artifact_unrevoked, weavelogic_key, Artifact, RevokedKeys};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[serde(alias = "weavelogic")]
    WeaveLogic,
    #[serde(alias = "cognitum")]
    Cognitum,
    /// Added by hand / staged locally.
    Local,
}

/// The persisted record for one installed cog (`<root>/<id>/cog.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CogRecord {
    pub id: String,
    #[serde(default)]
    pub version: String,
    pub source: Source,
    /// Desired state: true means the supervisor keeps it running (and restarts it on exit).
    #[serde(default)]
    pub enabled: bool,
    /// Binary filename inside the cog dir (e.g. `cog-bridge-arm`).
    pub binary: String,
    /// CLI args the cog is launched with (e.g. `["--interval", "1"]`).
    #[serde(default)]
    pub args: Vec<String>,
    /// Whether the WeaveLogic signature was verified at install time (always true for WeaveLogic).
    #[serde(default)]
    pub signed: bool,
}

impl CogRecord {
    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join(&self.id)
    }
    pub fn binary_path(&self, root: &Path) -> PathBuf {
        self.dir(root).join(&self.binary)
    }
}

/// Resolve the host root: `$WEFTOS_COG_ROOT`, else `~/.weftos/cogs`.
pub fn default_root() -> PathBuf {
    if let Ok(r) = std::env::var("WEFTOS_COG_ROOT") {
        return PathBuf::from(r);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".weftos").join("cogs")
}

/// Load every `<root>/<id>/cog.json` into records, sorted by id.
pub fn load_records(root: &Path) -> Vec<CogRecord> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else {
        return out;
    };
    for e in rd.flatten() {
        let cj = e.path().join("cog.json");
        let Ok(bytes) = std::fs::read(&cj) else { continue };
        let Ok(rec) = serde_json::from_slice::<CogRecord>(&bytes) else { continue };
        out.push(rec);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

pub fn save_record(root: &Path, rec: &CogRecord) -> std::io::Result<()> {
    let dir = rec.dir(root);
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec_pretty(rec).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(dir.join("cog.json"), json)
}

/// An install request: the manager fetched the binary (TLS in the browser/native) and uploads the
/// bytes here; the host verifies before writing. Keeps the appliance host free of a TLS stack.
#[derive(Clone, Debug, Deserialize)]
pub struct InstallReq {
    pub id: String,
    #[serde(default)]
    pub version: String,
    pub source: Source,
    #[serde(default)]
    pub args: Vec<String>,
    /// True for WeaveLogic cogs — the Ed25519 signature is then mandatory and checked.
    #[serde(default)]
    pub signed: bool,
    /// Expected sha256 of the binary, hex (from the registry).
    pub sha256: String,
    /// Ed25519 signature over the binary bytes, hex (required when `signed`).
    #[serde(default)]
    pub sig: Option<String>,
    /// base64 of the cog binary.
    pub binary_b64: String,
    #[serde(default)]
    pub enable: bool,
}

/// Cog ids are lower-case alphanumerics separated by single hyphens (max 64). The id names a
/// directory under the host root, so anything else (`../x`, `a/b`, an empty id) is refused.
pub fn valid_cog_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Verify and install an uploaded cog. Signed cogs must pass Ed25519 against the pinned WeaveLogic
/// key (signed-only, COG-008); unsigned (e.g. Cognitum mirror) must at least match their sha256.
///
/// A signature from a key on the operator's revocation list is refused. The list is read from the
/// runtime dir (see [`RevokedKeys::default_path`]); a list that exists but cannot be read, or a
/// runtime dir that cannot be located, fails the install rather than reading as "nothing revoked".
pub fn install(root: &Path, req: &InstallReq) -> Result<CogRecord, String> {
    let revoked = if req.signed { RevokedKeys::load_default()? } else { RevokedKeys::none() };
    install_with(root, req, &revoked)
}

/// [`install`] against an explicit revocation list.
pub fn install_with(root: &Path, req: &InstallReq, revoked: &RevokedKeys) -> Result<CogRecord, String> {
    if !valid_cog_id(&req.id) {
        return Err(format!("bad cog id {:?}", req.id));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(req.binary_b64.trim())
        .map_err(|e| format!("base64 decode: {e}"))?;
    if bytes.is_empty() {
        return Err("empty binary".into());
    }
    let art = Artifact {
        path: String::new(),
        size: bytes.len() as u64,
        sha256: req.sha256.clone(),
        sig: req.sig.clone().unwrap_or_default(),
        manifest_path: String::new(),
    };
    if req.signed {
        if art.sig.is_empty() {
            return Err("cog is marked signed but no signature was provided".into());
        }
        verify_artifact_unrevoked(&bytes, &art, &weavelogic_key(), revoked).map_err(|e| format!("refusing {}: {e}", req.id))?;
    } else {
        let got = sha256_hex(&bytes);
        if got != req.sha256 {
            return Err(format!("refusing {}: sha256 {got} != registry {}", req.id, req.sha256));
        }
    }

    install_verified(
        root,
        &VerifiedInstall {
            id: &req.id,
            version: &req.version,
            source: req.source,
            signed: req.signed,
            args: &req.args,
            enable: req.enable,
            bytes: &bytes,
        },
    )
}

/// A binary whose trust was already decided by the caller (a source resolver that checked
/// the signature against the key pinned for that source, or a sha256 against a registry).
/// [`install`] performs the WeaveLogic / sha256 checks and then lands here; other callers
/// (`weftos-cog-sources`, private registries) verify with their own pinned keys first.
pub struct VerifiedInstall<'a> {
    pub id: &'a str,
    pub version: &'a str,
    pub source: Source,
    /// Recorded in `cog.json`: whether a signature was verified.
    pub signed: bool,
    pub args: &'a [String],
    pub enable: bool,
    pub bytes: &'a [u8],
}

/// Write an already-verified cog binary and its record under `root`. Does no verification.
pub fn install_verified(root: &Path, v: &VerifiedInstall<'_>) -> Result<CogRecord, String> {
    if !valid_cog_id(v.id) {
        return Err(format!("bad cog id {:?}", v.id));
    }
    let binary = format!("cog-{}-arm", v.id);
    let dir = root.join(v.id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {dir:?}: {e}"))?;
    // Write to a temp file then rename over the target: a rename replaces the path even while the
    // old binary is running (the process keeps the old inode), avoiding ETXTBSY / "Text file busy".
    let final_path = dir.join(&binary);
    let tmp_path = dir.join(format!("{binary}.new"));
    std::fs::write(&tmp_path, v.bytes).map_err(|e| format!("write binary: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp_path, &final_path).map_err(|e| format!("install binary: {e}"))?;
    let rec = CogRecord {
        id: v.id.to_string(),
        version: v.version.to_string(),
        source: v.source,
        enabled: v.enable,
        binary,
        args: v.args.to_vec(),
        signed: v.signed,
    };
    save_record(root, &rec).map_err(|e| e.to_string())?;
    Ok(rec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trips_and_resolves_paths() {
        let rec = CogRecord {
            id: "bridge".into(),
            version: "0.1.0".into(),
            source: Source::WeaveLogic,
            enabled: true,
            binary: "cog-bridge-arm".into(),
            args: vec!["--interval".into(), "1".into()],
            signed: true,
        };
        let s = serde_json::to_string(&rec).unwrap();
        let back: CogRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(rec, back);
        let root = Path::new("/tmp/r");
        assert_eq!(rec.binary_path(root), Path::new("/tmp/r/bridge/cog-bridge-arm"));
    }

    #[test]
    fn source_accepts_aliases() {
        let r: CogRecord = serde_json::from_str(
            r#"{"id":"x","source":"weavelogic","binary":"cog-x-arm"}"#,
        )
        .unwrap();
        assert_eq!(r.source, Source::WeaveLogic);
        assert!(!r.enabled && r.args.is_empty());
    }

    fn b64(b: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    #[test]
    fn install_unsigned_checks_sha256() {
        let root = std::env::temp_dir().join(format!("cog-host-inst-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bytes = b"not-really-a-binary-but-fine".to_vec();
        let good = InstallReq {
            id: "demo".into(),
            version: "1".into(),
            source: Source::Cognitum,
            args: vec![],
            signed: false,
            sha256: sha256_hex(&bytes),
            sig: None,
            binary_b64: b64(&bytes),
            enable: false,
        };
        let rec = install(&root, &good).unwrap();
        assert_eq!(rec.id, "demo");
        assert!(root.join("demo/cog-demo-arm").exists());

        // wrong sha256 is refused
        let bad = InstallReq { sha256: "00".repeat(32), ..good.clone() };
        assert!(install(&root, &bad).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_refuses_path_like_ids() {
        let root = std::env::temp_dir().join(format!("cog-host-inst3-{}", std::process::id()));
        let bytes = b"x".to_vec();
        for id in ["../x", "a/b", "", "UP", "-a", "a--b", "x/../../y"] {
            let req = InstallReq {
                id: id.into(),
                version: "1".into(),
                source: Source::Cognitum,
                args: vec![],
                signed: false,
                sha256: sha256_hex(&bytes),
                sig: None,
                binary_b64: b64(&bytes),
                enable: false,
            };
            assert!(install(&root, &req).unwrap_err().contains("bad cog id"), "{id}");
            let v = VerifiedInstall { id, version: "1", source: Source::Local, signed: true, args: &[], enable: false, bytes: &bytes };
            assert!(install_verified(&root, &v).is_err(), "{id}");
        }
        assert!(!root.exists(), "nothing was created");
    }

    #[test]
    fn install_signed_without_signature_is_refused() {
        let root = std::env::temp_dir().join(format!("cog-host-inst2-{}", std::process::id()));
        let bytes = b"x".to_vec();
        let req = InstallReq {
            id: "s".into(),
            version: "1".into(),
            source: Source::WeaveLogic,
            args: vec![],
            signed: true,
            sha256: sha256_hex(&bytes),
            sig: None,
            binary_b64: b64(&bytes),
            enable: false,
        };
        let e = install_with(&root, &req, &RevokedKeys::none()).unwrap_err();
        assert!(e.contains("signed"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn signed_req(id: &str) -> (InstallReq, Vec<u8>) {
        let bytes = b"signed-binary".to_vec();
        let req = InstallReq {
            id: id.into(),
            version: "1".into(),
            source: Source::WeaveLogic,
            args: vec![],
            signed: true,
            sha256: sha256_hex(&bytes),
            // Not a valid signature: a revoked key is refused before the signature is checked.
            sig: Some("00".repeat(64)),
            binary_b64: b64(&bytes),
            enable: false,
        };
        (req, bytes)
    }

    #[test]
    fn install_refuses_a_revoked_release_key() {
        let root = std::env::temp_dir().join(format!("cog-host-rev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (req, _) = signed_req("rev");
        // Not revoked: it gets as far as the signature, which is bogus.
        let e = install_with(&root, &req, &RevokedKeys::none()).unwrap_err();
        assert!(!e.contains("revoked"), "{e}");
        let revoked = RevokedKeys::from_keys([weftos_cog_repo::WEAVELOGIC_PUBKEY_HEX]);
        let e = install_with(&root, &req, &revoked).unwrap_err();
        assert!(e.contains("revoked"), "{e}");
        assert!(!root.exists(), "nothing was installed");
    }

    #[test]
    fn install_fails_closed_on_an_unreadable_revocation_list() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("revoked_subjects.json"), "{not json").unwrap();
        let root = dir.path().join("cogs");
        let (req, _) = signed_req("bad");
        // Only this test reads the process environment through `install`.
        // SAFETY: no other test in this crate reads or writes this variable.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var("WEFTOS_RUNTIME_DIR", dir.path());
        }
        let e = install(&root, &req).unwrap_err();
        assert!(e.contains("parse"), "{e}");
        assert!(!root.exists());
    }

    #[test]
    fn load_records_reads_a_root() {
        let root = std::env::temp_dir().join(format!("cog-host-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let rec = CogRecord {
            id: "a".into(),
            version: "1".into(),
            source: Source::Local,
            enabled: false,
            binary: "cog-a-arm".into(),
            args: vec![],
            signed: false,
        };
        save_record(&root, &rec).unwrap();
        let got = load_records(&root);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "a");
        let _ = std::fs::remove_dir_all(&root);
    }
}
