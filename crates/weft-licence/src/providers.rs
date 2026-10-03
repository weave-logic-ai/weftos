//! The two things phase 4 (Cognitum) will replace, behind traits, plus the
//! phase 2 implementations: an operator-signed declared licence and a
//! registry fetcher (see `registry.rs`).

use std::path::PathBuf;

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use weft_licence_wire::{
    SignedEnvelope, envelope_key, parse_canonical, sha256_hex, sign_envelope, verify_envelope,
};

/// Domain tag of the operator-signed licence file.
pub const LICENCE_DOMAIN: &str = "weft-licence-v1/licence";

/// What a licence grants for one cog right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entitlement {
    /// sha256 of the licence reference (no plaintext account is ever sent).
    pub ref_sha256: String,
    /// Licence expiry, unix seconds; `None` when it never expires.
    pub expires: Option<u64>,
}

/// Why no entitlement was granted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenceCheckError {
    /// No licence covers the cog.
    Unlicensed,
    /// A covering licence exists but has expired.
    Expired,
    /// The licence source could not be read or verified.
    Unreadable(String),
}

/// The licence check. Phase 4 replaces this with a Cognitum-signed entitlement
/// (questions C1, C3 to Cognitum); until then it is [`LocalDeclaredLicence`].
pub trait LicenceProvider: Send + Sync {
    /// Does a licence cover `cog_id` for `mesh_id` at `now`?
    fn entitlement(&self, cog_id: &str, mesh_id: &str, now: u64) -> Result<Entitlement, LicenceCheckError>;
}

/// One registry entry as the proxy needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CogEntry {
    /// Cog id.
    pub cog_id: String,
    /// The version the registry lists.
    pub version: String,
    /// Registry the entry came from (recorded in the grant).
    pub registry: String,
    /// sha256 of the registry entry used.
    pub manifest_sha256: String,
    /// Binaries the registry lists.
    pub artifacts: Vec<EntryArtifact>,
}

/// One listed binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryArtifact {
    /// Architecture.
    pub arch: String,
    /// Size in bytes (an upper bound when the registry only gives KiB).
    pub size: u64,
    /// Registry sha256, lower-case hex.
    pub sha256: String,
}

/// Why a registry lookup or download failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The registry does not list the cog.
    NotFound,
    /// The registry lists another version than the one asked for.
    VersionUnavailable(String),
    /// No binary for this architecture (registry coverage is question C7).
    ArchUnavailable(String),
    /// The registry gives no size, so the 64 MiB limit cannot be checked.
    SizeUnknown,
    /// The download failed.
    Failed(String),
    /// The bytes did not match the registry sha256 or size.
    Verify(String),
}

/// The cog fetch. Cognitum's registry is public today (question C2); this
/// trait is the seam if that changes.
pub trait CogFetcher: Send + Sync {
    /// Resolve `version` (`"latest"` or an exact version) to a registry entry.
    fn resolve(&self, cog_id: &str, version: &str) -> Result<CogEntry, FetchError>;
    /// Download `arch` of `entry`, verified against the registry sha256 and size.
    fn fetch(&self, entry: &CogEntry, arch: &str) -> Result<Vec<u8>, FetchError>;
}

/// Signs a statement with the Seed's DEVICE key (question C4: Cognitum has not
/// said whether a local process can call the device key over HTTP).
///
/// STUB. Phase 4 uses this to cross-certify the grant key with the device key.
pub trait DeviceSigner: Send + Sync {
    /// Sign `msg`; `None` when no device signing is available.
    fn sign(&self, msg: &[u8]) -> Option<Vec<u8>>;
    /// Short label for the identity endpoint.
    fn label(&self) -> &'static str;
}

/// The phase 2 stand-in: no device key.
#[derive(Debug, Default, Clone, Copy)]
pub struct StubDeviceSigner;

impl DeviceSigner for StubDeviceSigner {
    fn sign(&self, _: &[u8]) -> Option<Vec<u8>> {
        None // STUB (C4)
    }
    fn label(&self) -> &'static str {
        "stub"
    }
}

/// The operator-declared licence: signed by a pinned operator key, scoped to
/// one mesh. Not a Cognitum proof until phase 4 (ADR-106 section 10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredLicence {
    /// Always 1.
    pub v: u32,
    /// The mesh this licence is declared for.
    pub mesh_id: String,
    /// Always `cognitum`.
    pub source: String,
    /// The account reference. Only its hash leaves the Seed.
    pub account_ref: String,
    /// Covered cog ids; `"*"` covers all.
    pub cogs: Vec<String>,
    /// Expiry, unix seconds; absent means none.
    pub expires: Option<u64>,
    /// When the operator signed it.
    pub issued_at: u64,
}

/// Sign a declared licence as the operator.
pub fn sign_licence(rec: &DeclaredLicence, operator: &SigningKey) -> SignedEnvelope {
    sign_envelope(LICENCE_DOMAIN, rec, operator).expect("licence serializes")
}

/// Says whether a signer key is a pinned operator key.
pub type OperatorCheck = Box<dyn Fn(&[u8; 32]) -> bool + Send + Sync>;

/// Reads and re-verifies the operator-signed licence file on every check, so
/// replacing the file takes effect at once.
pub struct LocalDeclaredLicence {
    path: PathBuf,
    operator_ok: OperatorCheck,
}

impl LocalDeclaredLicence {
    /// `operator_ok` says whether a key is a pinned operator key.
    pub fn new(path: PathBuf, operator_ok: OperatorCheck) -> Self {
        Self { path, operator_ok }
    }

    fn read(&self) -> Result<DeclaredLicence, LicenceCheckError> {
        let un = |m: &str| LicenceCheckError::Unreadable(m.to_string());
        let raw = crate::fsio::read_capped(&self.path, 64 * 1024)
            .map_err(|e| un(&e.to_string()))?
            .ok_or(LicenceCheckError::Unlicensed)?;
        let env: SignedEnvelope = serde_json::from_slice(&raw).map_err(|_| un("not an envelope"))?;
        let pk = envelope_key(&env).map_err(|_| un("bad key"))?;
        if !(self.operator_ok)(&pk) {
            return Err(un("signer is not a pinned operator key"));
        }
        verify_envelope(LICENCE_DOMAIN, &env, &pk).map_err(|_| un("bad signature"))?;
        let rec: DeclaredLicence = parse_canonical(&env.payload).map_err(|_| un("not canonical"))?;
        if rec.v != 1 || rec.source != "cognitum" {
            return Err(un("version or source"));
        }
        Ok(rec)
    }
}

impl LicenceProvider for LocalDeclaredLicence {
    fn entitlement(&self, cog_id: &str, mesh_id: &str, now: u64) -> Result<Entitlement, LicenceCheckError> {
        let rec = self.read()?;
        if rec.mesh_id != mesh_id || !rec.cogs.iter().any(|c| c == "*" || c == cog_id) {
            return Err(LicenceCheckError::Unlicensed);
        }
        if rec.expires.is_some_and(|e| e <= now) {
            return Err(LicenceCheckError::Expired);
        }
        Ok(Entitlement { ref_sha256: sha256_hex(rec.account_ref.as_bytes()), expires: rec.expires })
    }
}
