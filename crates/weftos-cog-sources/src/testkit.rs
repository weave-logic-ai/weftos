//! Test fixtures: temp registries signed with test keys. Never real keys, never the network.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use ed25519_dalek::{Signer, SigningKey};
use weftos_cog_repo::{sha256_hex, Artifact, CogEntry, Registry, SCHEMA};

use crate::config::{CogLicence, CogSource, LicensedCogs, SourceKind};
use crate::error::Result;
use crate::fetch::{FsReader, Reader};

pub fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

pub fn pub_hex(k: &SigningKey) -> String {
    hex::encode(k.verifying_key().to_bytes())
}

pub fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 2, 12, 0, 0).unwrap()
}

/// Binary bytes for a fake cog.
pub fn bin(id: &str) -> Vec<u8> {
    format!("\x7fELF fake cog {id}").into_bytes()
}

/// Write a COG-008 repo under `dir` signed with `signer`; returns the registry.json path.
pub fn signed_repo(dir: &Path, signer: &SigningKey, cogs: &[&str]) -> PathBuf {
    let mut entries = Vec::new();
    for id in cogs {
        let bytes = bin(id);
        let rel = format!("cogs/arm/cog-{id}-arm");
        std::fs::create_dir_all(dir.join("cogs/arm")).unwrap();
        std::fs::write(dir.join(&rel), &bytes).unwrap();
        entries.push(CogEntry {
            id: (*id).into(),
            name: format!("{id} name"),
            version: "1.0.0".into(),
            category: "health".into(),
            description: format!("{id} cog"),
            hardware_requirement: vec!["pi-zero-2w".into()],
            artifacts: BTreeMap::from([(
                "arm".to_string(),
                Artifact {
                    path: rel,
                    size: bytes.len() as u64,
                    sha256: sha256_hex(&bytes),
                    sig: hex::encode(signer.sign(&bytes).to_bytes()),
                    manifest_path: "cogs/arm/manifest.json".into(),
                },
            )]),
        });
    }
    let reg = Registry { schema: SCHEMA, repo: "fixture".into(), updated: "2026-10-02".into(), cogs: entries };
    let p = dir.join("registry.json");
    std::fs::write(&p, serde_json::to_vec_pretty(&reg).unwrap()).unwrap();
    p
}

/// Write a Cognitum-style registry + binaries under `dir`; returns the app-registry.json path.
pub fn cognitum_repo(dir: &Path, cogs: &[&str]) -> PathBuf {
    std::fs::create_dir_all(dir.join("cogs/arm")).unwrap();
    let mut list = Vec::new();
    for id in cogs {
        let b = bin(id);
        std::fs::write(dir.join(format!("cogs/arm/cog-{id}-arm")), &b).unwrap();
        list.push(serde_json::json!({"id": id, "name": id, "category": "health", "version": "1.0.0",
            "size_kb": 1, "sha256": sha256_hex(&b), "binary_size": b.len()}));
    }
    let reg = serde_json::json!({"version": "2.3.2", "updated": "2026-10-01",
        "binary_base_url": dir.to_string_lossy(), "cogs": list});
    let p = dir.join("app-registry.json");
    std::fs::write(&p, serde_json::to_vec(&reg).unwrap()).unwrap();
    p
}

pub fn source(name: &str, kind: SourceKind, url: &Path, keys: &[String], priority: i32) -> CogSource {
    CogSource { name: name.into(), kind, url: url.to_string_lossy().into(), pinned_keys: keys.to_vec(), priority, enabled: true, allow_insecure: kind == SourceKind::Cognitum }
}

pub fn licence(source: &str, cogs: LicensedCogs, expires: Option<&str>) -> CogLicence {
    CogLicence { source: source.into(), cogs, account: "acct-test".into(), expires: expires.map(String::from) }
}

/// Counts reads whose location contains `needle` (to prove nothing was downloaded).
pub struct CountingReader {
    pub needle: String,
    pub hits: RefCell<u32>,
}

impl Reader for CountingReader {
    fn read(&self, location: &str, max: u64) -> Result<Vec<u8>> {
        if location.contains(&self.needle) {
            *self.hits.borrow_mut() += 1;
        }
        FsReader.read(location, max)
    }
}
