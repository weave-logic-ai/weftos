//! Containment, trust, lazy-window, tampered-registry and advertising
//! hardening tests (review round 1). Fake directories in temp dirs only.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use clawft_types::placement::{AttrValue, CapabilityState};

use super::tests::{adopt_fake, fake_model, input, new_reg, operator};
use super::*;
use crate::revocation::{RevocationKind, RevocationList};
use crate::workload_pkg::TrustAnchors;
use crate::workload_pkg::codec::hex_encode;

fn set_mtime(path: &Path, t: SystemTime) {
    fs::OpenOptions::new().write(true).open(path).unwrap().set_modified(t).unwrap();
}

fn mtime(path: &Path) -> SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

fn untrusted_msg(r: Result<ResolvedModel, ModelError>) -> String {
    r.expect_err("must be refused").to_string()
}

// ── symlink containment ──────────────────────────────────────────

#[cfg(unix)]
mod links {
    use std::os::unix::fs::symlink;

    use super::*;

    fn outside(tmp: &Path) -> std::path::PathBuf {
        let secret = tmp.join("outside/secret.bin");
        fs::create_dir_all(secret.parent().unwrap()).unwrap();
        fs::write(&secret, vec![0xAAu8; 64]).unwrap();
        secret
    }

    #[test]
    fn a_link_to_a_file_outside_the_root_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let secret = outside(tmp.path());
        let dir = tmp.path().join("quant");
        fake_model(&dir);
        symlink(&secret, dir.join("stolen.bin")).unwrap();
        let err = scan_dir(&dir, input("Fake")).unwrap_err();
        assert!(err.to_string().contains("outside the model root"), "{err}");
    }

    #[test]
    fn a_link_to_a_directory_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        outside(tmp.path());
        let dir = tmp.path().join("quant");
        fake_model(&dir);
        symlink(tmp.path().join("outside"), dir.join("sub")).unwrap();
        let err = scan_dir(&dir, input("Fake")).unwrap_err();
        assert!(err.to_string().contains("symlinked directory"), "{err}");
    }

    #[test]
    fn an_hf_snapshot_link_may_not_leave_its_repo_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let secret = outside(tmp.path());
        let repo = tmp.path().join("models--o--n");
        let snap = repo.join("snapshots/abc");
        fs::create_dir_all(&snap).unwrap();
        fs::create_dir_all(repo.join("blobs")).unwrap();
        symlink(&secret, snap.join("model.safetensors")).unwrap();
        assert!(scan_dir(&snap, input("Fake")).is_err());
    }

    #[test]
    fn a_link_loop_is_skipped_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("quant");
        fake_model(&dir);
        symlink(dir.join("b.bin"), dir.join("a.bin")).unwrap();
        symlink(dir.join("a.bin"), dir.join("b.bin")).unwrap();
        let s = scan_dir(&dir, input("Fake")).unwrap();
        assert_eq!(s.body.shards.len(), 2, "only the real shards");
    }

    #[test]
    fn an_ollama_blob_that_is_a_link_outside_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let secret = outside(tmp.path());
        let models = tmp.path().join("ollama");
        let digest = "ab".repeat(32);
        fs::create_dir_all(models.join("blobs")).unwrap();
        symlink(&secret, models.join(format!("blobs/sha256-{digest}"))).unwrap();
        let mdir = models.join("manifests/registry.ollama.ai/library/tiny");
        fs::create_dir_all(&mdir).unwrap();
        let m = serde_json::json!({"layers":[{"mediaType":"application/vnd.ollama.image.model",
            "digest":format!("sha256:{digest}"),"size":64}]});
        fs::write(mdir.join("latest"), m.to_string()).unwrap();
        let err = scan_ollama(&models, "tiny", "latest", input("tiny")).unwrap_err();
        assert!(err.to_string().contains("outside the model root"), "{err}");
    }

    #[test]
    fn a_link_swapped_in_after_adoption_is_refused_and_never_served() {
        let tmp = tempfile::tempdir().unwrap();
        let secret = outside(tmp.path());
        fs::write(&secret, vec![2u8; 2048]).unwrap(); // same size as shard 2
        let dir = tmp.path().join("quant");
        fake_model(&dir);
        let reg = new_reg();
        adopt_fake(&reg, &dir, "Fake");
        let shard = dir.join("model-00002-of-00002.safetensors");
        fs::remove_file(&shard).unwrap();
        symlink(&secret, &shard).unwrap();
        let c = reg.check("Fake", CheckMode::Lazy).unwrap();
        assert!(matches!(c.state, ModelState::Refused { .. }), "{:?}", c.state);
        assert!(untrusted_msg(reg.resolve("Fake")).contains("outside the model root"));
        let caps = model_capabilities(&reg, &TierResolver::with_external_roots(vec![]));
        assert!(matches!(caps[0].attrs.get("shards"), Some(AttrValue::List(l)) if l.is_empty()));
    }

    #[cfg(feature = "mesh")]
    #[test]
    fn seeding_rechecks_containment() {
        use crate::artifact_store::ArtifactStore;
        use crate::mesh_artifact::{ArtifactExchange, ExchangeConfig};
        use crate::model_manifest::sharing::seed_model;
        let tmp = tempfile::tempdir().unwrap();
        let secret = outside(tmp.path());
        fs::write(&secret, vec![1u8; 4096]).unwrap();
        let dir = tmp.path().join("quant");
        fake_model(&dir);
        let reg = new_reg();
        let (key, kid, anchors) = operator();
        let mut inp = input("Shared");
        inp.redistributable = true;
        reg.adopt(scan_dir(&dir, inp).unwrap(), &key, &kid, &anchors, false).unwrap();
        let shard = dir.join("model-00001-of-00002.safetensors");
        fs::remove_file(&shard).unwrap();
        symlink(&secret, &shard).unwrap();
        let ex = ArtifactExchange::new("n", Arc::new(ArtifactStore::new_memory()), ExchangeConfig::default()).unwrap();
        assert!(seed_model(&ex, &reg, "Shared", &anchors).is_err());
    }
}

// ── trust at resolve ─────────────────────────────────────────────

#[test]
fn resolve_refuses_without_trust_and_after_the_key_is_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let reg = new_reg();
    adopt_fake(&reg, &dir, "Fake");
    assert!(reg.resolve("Fake").is_ok());
    reg.set_trust(ModelTrust::new(TrustAnchors::default()));
    assert!(untrusted_msg(reg.resolve("Fake")).contains("attestation does not verify"));
    let bare = ModelRegistry::in_memory();
    let (_, _, anchors) = operator();
    let src = new_reg();
    let (a, _, _) = adopt_fake(&src, &dir, "Fake");
    bare.attach(&a.envelope, &dir, &anchors, false).unwrap();
    assert!(untrusted_msg(bare.resolve("Fake")).contains("no trust anchors"));
}

#[test]
fn revoked_package_signer_or_shard_hash_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let (key, _, anchors) = operator();
    for (kind, which) in [(RevocationKind::Package, 0), (RevocationKind::SignerKey, 1), (RevocationKind::ArtifactHash, 2)] {
        let list = Arc::new(RevocationList::new(tmp.path().join(format!("rev{which}.json"))));
        let reg = new_reg();
        let (a, _, _) = adopt_fake(&reg, &dir, "Fake");
        reg.set_trust(ModelTrust { anchors: anchors.clone(), revocations: Some(list.clone()) });
        assert!(reg.resolve("Fake").is_ok());
        let subject = match which {
            0 => a.package_id.clone(),
            1 => hex_encode(&key.verifying_key().to_bytes()),
            _ => a.verified.body.shards[0].blake3.clone(),
        };
        list.revoke_subject(kind, &subject, "test").unwrap();
        assert!(untrusted_msg(reg.resolve("Fake")).contains("revoked"), "{kind:?}");
        let caps = model_capabilities(&reg, &TierResolver::with_external_roots(vec![]));
        assert!(matches!(caps[0].attrs.get("shards"), Some(AttrValue::List(l)) if l.is_empty()), "{kind:?} advertises nothing");
    }
}

// ── lazy window and first-resolve full check ─────────────────────

#[test]
fn same_stamp_tampering_is_a_documented_lazy_window_closed_by_full_and_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let path = tmp.path().join("run/registry.json");
    let reg = ModelRegistry::open(&path).unwrap().with_trust(ModelTrust::new(operator().2));
    adopt_fake(&reg, &dir, "Fake");
    assert!(reg.resolve("Fake").is_ok());
    let shard = dir.join("model-00001-of-00002.safetensors");
    let before = mtime(&shard);
    fs::write(&shard, vec![9u8; 4096]).unwrap(); // same size
    set_mtime(&shard, before); // same mtime
    // Lazy resolve cannot see it (the documented window) ...
    assert!(reg.resolve("Fake").is_ok());
    // ... an explicit full check does, and refuses the model.
    assert!(matches!(reg.check("Fake", CheckMode::Full).unwrap().state, ModelState::Refused { .. }));
    // A fresh process (reopened registry) does a full check on first resolve.
    fs::write(&shard, vec![1u8; 4096]).unwrap();
    set_mtime(&shard, before);
    assert!(reg.check("Fake", CheckMode::Full).unwrap().state.is_ready());
    fs::write(&shard, vec![9u8; 4096]).unwrap();
    set_mtime(&shard, before);
    let restarted = ModelRegistry::open(&path).unwrap().with_trust(ModelTrust::new(operator().2));
    assert!(restarted.resolve("Fake").is_err(), "first resolve after a start hashes every byte");
}

// ── tampered registry file ───────────────────────────────────────

fn registry_with_model(tmp: &Path) -> (std::path::PathBuf, String) {
    let dir = tmp.join("quant");
    fake_model(&dir);
    let path = tmp.join("run/registry.json");
    let reg = ModelRegistry::open(&path).unwrap();
    let (a, _, _) = adopt_fake(&reg, &dir, "Fake");
    (path, a.package_id)
}

fn edit(path: &Path, f: impl FnOnce(&mut serde_json::Value)) {
    let mut v: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    f(&mut v);
    fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
}

#[test]
fn a_tampered_registry_is_refused_at_open_or_at_resolve() {
    // A file hash edited in the file list no longer agrees with the body.
    let tmp = tempfile::tempdir().unwrap();
    let (path, id) = registry_with_model(tmp.path());
    edit(&path, |v| v["models"][&id]["files"][0]["blake3"] = serde_json::json!("0".repeat(64)));
    assert!(ModelRegistry::open(&path).is_err());

    // A body edit changes the package id: the registry key no longer matches.
    let tmp = tempfile::tempdir().unwrap();
    let (path, id) = registry_with_model(tmp.path());
    edit(&path, |v| v["models"][&id]["envelope"]["body"]["shards"][0]["blake3"] = serde_json::json!("1".repeat(64)));
    assert!(ModelRegistry::open(&path).is_err());

    // An attacker who re-keys the entry and fixes the file list is still
    // caught: the signature does not cover the edited body.
    let tmp = tempfile::tempdir().unwrap();
    let (path, id) = registry_with_model(tmp.path());
    let evil = "1".repeat(64);
    let new_id = std::cell::RefCell::new(String::new());
    edit(&path, |v| {
        let mut entry = v["models"][&id].clone();
        entry["envelope"]["body"]["shards"][0]["blake3"] = serde_json::json!(evil);
        entry["files"][0]["blake3"] = serde_json::json!(evil);
        let env: crate::workload_pkg::ManifestEnvelope = serde_json::from_value(entry["envelope"].clone()).unwrap();
        *new_id.borrow_mut() = env.package_id().unwrap();
        v["models"] = serde_json::json!({ new_id.borrow().clone(): entry });
    });
    let reg = ModelRegistry::open(&path).unwrap().with_trust(ModelTrust::new(operator().2));
    assert!(untrusted_msg(reg.resolve("Fake")).contains("attestation does not verify"));
}

// ── attach carries tokenizer and template paths ──────────────────

#[test]
fn attach_checks_the_tokenizer_and_template_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let src = new_reg();
    let (a, _, anchors) = adopt_fake(&src, &dir, "Fake");
    assert_eq!(a.verified.body.tokenizer_path.as_deref(), Some("tokenizer.json"));
    assert_eq!(a.verified.body.template_path.as_deref(), Some("tokenizer_config.json"));
    let other = new_reg();
    other.attach(&a.envelope, &dir, &anchors, false).unwrap();
    let r = other.resolve("Fake").unwrap();
    assert_eq!(r.tokenizer, Some(dir.canonicalize().unwrap().join("tokenizer.json")));
    fs::write(dir.join("tokenizer.json"), b"{\"tok\":2}").unwrap();
    assert!(untrusted_msg(other.resolve("Fake")).contains("refused"), "swapped tokenizer");
    fs::write(dir.join("tokenizer.json"), b"{\"tok\":1}").unwrap();
    other.check("Fake", CheckMode::Full).unwrap();
    fs::write(dir.join("tokenizer_config.json"), b"{\"chat_template\":\"evil\"}").unwrap();
    assert!(other.check("Fake", CheckMode::Full).unwrap().state != ModelState::Ready, "swapped template");
}

#[test]
fn a_body_with_a_hash_but_no_path_is_invalid() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let mut b = scan_dir(&dir, input("Fake")).unwrap().body;
    b.tokenizer_path = None;
    assert!(b.validate().is_err());
}

// ── advertising: refused and hidden ──────────────────────────────

#[test]
fn a_refused_model_advertises_no_hashes_and_stays_unoffered() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let reg = new_reg();
    let (a, _, _) = adopt_fake(&reg, &dir, "Fake");
    let tiers = TierResolver::with_external_roots(vec![]);
    assert_eq!(model_capabilities(&reg, &tiers)[0].state, CapabilityState::Available);
    let shard = dir.join("model-00001-of-00002.safetensors");
    fs::write(&shard, vec![7u8; 4096]).unwrap();
    set_mtime(&shard, SystemTime::now() + Duration::from_secs(60));
    reg.check("Fake", CheckMode::Lazy).unwrap(); // records the refusal
    let caps = model_capabilities(&reg, &tiers);
    assert_eq!(caps.len(), 1);
    assert_eq!(caps[0].state, CapabilityState::Degraded, "the placer never offers it");
    let json = serde_json::to_string(&caps[0]).unwrap();
    assert!(!json.contains(&a.package_id));
    for h in a.verified.body.shard_hashes() {
        assert!(!json.contains(h), "no shard hash is advertised for a refused model");
    }
}

#[test]
fn a_hidden_model_is_not_advertised_but_still_resolves() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let reg = new_reg();
    adopt_fake(&reg, &dir, "Fake");
    let tiers = TierResolver::with_external_roots(vec![]);
    reg.set_advertised("Fake", false).unwrap();
    assert!(model_capabilities(&reg, &tiers).is_empty());
    assert!(reg.resolve("Fake").is_ok());
    reg.set_advertised("Fake", true).unwrap();
    assert_eq!(model_capabilities(&reg, &tiers).len(), 1);
}

// ── seeding: revocation and key removal ──────────────────────────

#[cfg(feature = "mesh")]
mod seeding {
    use super::*;
    use crate::artifact_store::ArtifactStore;
    use crate::mesh_artifact::{ArtifactExchange, ExchangeConfig};
    use crate::model_manifest::sharing::{SeedModelError, seed_model};

    fn shared() -> (tempfile::TempDir, ModelRegistry, AdoptedModel, TrustAnchors, ed25519_dalek::SigningKey) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("quant");
        fake_model(&dir);
        let reg = new_reg();
        let (key, kid, anchors) = operator();
        let mut inp = input("Shared");
        inp.redistributable = true;
        let a = reg.adopt(scan_dir(&dir, inp).unwrap(), &key, &kid, &anchors, false).unwrap();
        (tmp, reg, a, anchors, key)
    }

    fn exchange(tmp: &Path, list: Option<Arc<RevocationList>>) -> ArtifactExchange {
        let ex = ArtifactExchange::new("n", Arc::new(ArtifactStore::new_memory()), ExchangeConfig::default()).unwrap();
        if let Some(l) = list {
            ex.set_revocations(l);
        }
        let _ = tmp;
        ex
    }

    #[test]
    fn revoked_package_signer_manifest_or_shard_is_not_seeded() {
        for which in 0..4 {
            let (tmp, reg, a, anchors, key) = shared();
            let list = Arc::new(RevocationList::new(tmp.path().join("rev.json")));
            let manifest = a.envelope.to_pretty_json().unwrap();
            let (kind, subject) = match which {
                0 => (RevocationKind::Package, a.package_id.clone()),
                1 => (RevocationKind::SignerKey, hex_encode(&key.verifying_key().to_bytes())),
                2 => (RevocationKind::ArtifactHash, blake3::hash(&manifest).to_hex().to_string()),
                _ => (RevocationKind::ArtifactHash, a.verified.body.shards[1].blake3.clone()),
            };
            list.revoke_subject(kind, &subject, "test").unwrap();
            let ex = exchange(tmp.path(), Some(list));
            let err = seed_model(&ex, &reg, "Shared", &anchors).unwrap_err();
            assert!(matches!(err, SeedModelError::Revoked(_)), "case {which}: {err}");
        }
    }

    #[test]
    fn removing_the_operator_key_stops_sharing() {
        let (tmp, reg, _, _, _) = shared();
        let ex = exchange(tmp.path(), None);
        let err = seed_model(&ex, &reg, "Shared", &TrustAnchors::default()).unwrap_err();
        assert!(err.to_string().contains("attestation does not verify"), "{err}");
    }
}
