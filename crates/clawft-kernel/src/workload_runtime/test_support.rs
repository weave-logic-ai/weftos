//! Shared fixtures for workload runtime tests: real signed packages built
//! with the card-07 packer and verifier, and an in-memory credential store.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use clawft_types::secret::SecretString;
use ed25519_dalek::SigningKey;

use super::seed_http::SeedCredentials;
use super::types::{RuntimeError, VerifiedWorkload};
use crate::workload_pkg::codec::hex_encode;
use crate::workload_pkg::{
    CogPackInput, DirSource, KeyOrigin, PackageSource, TrustAnchors, VerifyPolicy, key_id_for,
    pack_cog, sign_envelope, verify_dir, write_manifest,
};

/// A signed, verified workload and the tempdir holding its package.
pub struct SignedFixture {
    pub _tmp: tempfile::TempDir,
    pub root: PathBuf,
    pub workload: VerifiedWorkload,
}

/// Build, sign (operator key), verify and load a cog package.
pub fn signed_workload(cog_toml: &str, binaries: &[(&str, &[u8])]) -> SignedFixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let cog_dir = root.join("src");
    std::fs::create_dir_all(&cog_dir).unwrap();
    std::fs::write(cog_dir.join("cog.toml"), cog_toml).unwrap();
    let mut bins = Vec::new();
    for (arch, bytes) in binaries {
        let p = root.join(format!("bin-{arch}"));
        std::fs::write(&p, bytes).unwrap();
        bins.push((arch.to_string(), p));
    }
    let input = CogPackInput {
        cog_dir,
        binaries: bins,
        source: PackageSource {
            repo: Some("cogs-fork".into()),
            commit: Some("8970f99".into()),
            release_url: None,
        },
        cognitum_record: None,
    };
    let pkg = root.join("pkg");
    let mut env = pack_cog(&input, &pkg).unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let pk = key.verifying_key().to_bytes();
    sign_envelope(&mut env, &key, &key_id_for(&pk)).unwrap();
    write_manifest(&pkg, &env).unwrap();
    let mut anchors = TrustAnchors::default();
    anchors
        .push_signer(&key_id_for(&pk), &hex_encode(&pk), KeyOrigin::Operator)
        .unwrap();
    let verified = verify_dir(&pkg, &anchors, &VerifyPolicy::default()).unwrap();
    let workload = VerifiedWorkload::from_package(&verified, &DirSource::new(&pkg)).unwrap();
    SignedFixture {
        _tmp: tmp,
        root,
        workload,
    }
}

/// A minimal ELF header for `machine` (enough for arch classification).
pub fn fake_elf(machine: u16) -> Vec<u8> {
    let mut b = vec![0u8; 64];
    b[..4].copy_from_slice(b"\x7fELF");
    b[4] = 2; // 64-bit
    b[5] = 1; // little-endian
    b[18..20].copy_from_slice(&machine.to_le_bytes());
    b
}

/// Mode bits of `p`.
pub fn mode_of(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// In-memory [`SeedCredentials`].
#[derive(Default)]
pub struct MemoryCredentials(Mutex<HashMap<String, String>>);

impl MemoryCredentials {
    /// Store holding `token` for `node`.
    pub fn with(node: &str, token: &str) -> Self {
        let s = Self::default();
        s.0.lock().unwrap().insert(node.into(), token.into());
        s
    }

    /// Raw stored value (tests only).
    pub fn raw(&self, node: &str) -> Option<String> {
        self.0.lock().unwrap().get(node).cloned()
    }
}

impl SeedCredentials for MemoryCredentials {
    fn get(&self, node_id: &str) -> Result<SecretString, RuntimeError> {
        self.raw(node_id)
            .map(SecretString::new)
            .ok_or_else(|| RuntimeError::InvalidConfig(format!("no credential for {node_id}")))
    }

    fn put(&self, node_id: &str, token: SecretString) -> Result<(), RuntimeError> {
        self.0
            .lock()
            .unwrap()
            .insert(node_id.into(), token.expose().into());
        Ok(())
    }
}
