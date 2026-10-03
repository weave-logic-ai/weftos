//! Adoption, registry, attestation and advertising tests. Every model is a
//! fake directory in a temp dir; nothing touches a real HF cache, `~/llm`,
//! Ollama data or a model server.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use clawft_types::placement::{AttrValue, CapabilityState};
use ed25519_dalek::SigningKey;

use super::*;
use crate::workload_pkg::codec::hex_encode;
use crate::workload_pkg::{KeyOrigin, TrustAnchors, VerifyError, key_id_for};

pub(super) fn operator() -> (SigningKey, String, TrustAnchors) {
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let pk = key.verifying_key().to_bytes();
    let id = key_id_for(&pk);
    let mut anchors = TrustAnchors::default();
    anchors.push_signer(&id, &hex_encode(&pk), KeyOrigin::Operator).unwrap();
    (key, id, anchors)
}

/// A registry that trusts the test operator key and has no revocations.
pub(super) fn new_reg() -> ModelRegistry {
    ModelRegistry::in_memory().with_trust(ModelTrust::new(operator().2))
}

pub(super) fn input(name: &str) -> AdoptInput {
    AdoptInput {
        name: name.into(),
        format: ModelFormat::Mlx,
        source: ModelSource {
            hf_repo: Some("example-org/example-model".into()),
            hf_revision: Some("0123456789abcdef".into()),
            ollama_tag: None,
        },
        redistributable: false,
    }
}

/// A fake MLX quant directory: two shards, tokenizer, config, a dotfile.
pub(super) fn fake_model(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("model-00001-of-00002.safetensors"), vec![1u8; 4096]).unwrap();
    fs::write(dir.join("model-00002-of-00002.safetensors"), vec![2u8; 2048]).unwrap();
    fs::write(dir.join("tokenizer.json"), b"{\"tok\":1}").unwrap();
    fs::write(dir.join("tokenizer_config.json"), b"{\"chat_template\":\"x\"}").unwrap();
    fs::write(dir.join("config.json"), b"{}").unwrap();
    fs::write(dir.join(".gitattributes"), b"x").unwrap();
}

pub(super) fn listing(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

pub(super) fn adopt_fake(
    reg: &ModelRegistry,
    dir: &Path,
    name: &str,
) -> (AdoptedModel, SigningKey, TrustAnchors) {
    let (key, id, anchors) = operator();
    let scanned = scan_dir(dir, input(name)).unwrap();
    let adopted = reg.adopt(scanned, &key, &id, &anchors, false).unwrap();
    (adopted, key, anchors)
}

fn bump_mtime(path: &Path) {
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() + Duration::from_secs(3600)).unwrap();
}

#[test]
fn adopt_hashes_in_place_without_copying() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let before = listing(&dir);
    let reg = new_reg();
    let (adopted, _, _) = adopt_fake(&reg, &dir, "Fake-Model-4bit");
    assert_eq!(listing(&dir), before, "adoption must not add or move files");
    assert_eq!(listing(tmp.path()), vec!["quant"], "nothing created beside the model");
    let body = adopted.verified.body.clone();
    assert_eq!(body.shards.len(), 2);
    assert_eq!(body.shards[0].path, "model-00001-of-00002.safetensors");
    assert_eq!(body.shards[0].size, 4096);
    assert_eq!(body.shards[0].blake3, blake3::hash(&vec![1u8; 4096]).to_hex().to_string());
    assert_eq!(body.tokenizer_blake3.as_deref(), Some(&*blake3::hash(b"{\"tok\":1}").to_hex()));
    assert!(body.template_blake3.is_some());
    assert!(!body.redistributable, "sharing is opt-in");
    let r = reg.resolve("Fake-Model-4bit").unwrap();
    assert_eq!(r.root, dir.canonicalize().unwrap());
    assert!(r.shards[0].starts_with(&r.root));
    assert_eq!(r.package_id, adopted.package_id);
}

#[cfg(unix)]
#[test]
fn adopts_an_hf_snapshot_of_symlinked_blobs() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("models--org--name");
    let blobs = repo.join("blobs");
    let snap = repo.join("snapshots/abc123");
    fs::create_dir_all(&blobs).unwrap();
    fs::create_dir_all(&snap).unwrap();
    fs::write(blobs.join("aaaa"), vec![9u8; 1000]).unwrap();
    std::os::unix::fs::symlink("../../blobs/aaaa", snap.join("model.safetensors")).unwrap();
    std::os::unix::fs::symlink("../../blobs/missing", snap.join("model-2.safetensors")).unwrap();
    let scanned = scan_dir(&snap, input("Linked-Model")).unwrap();
    assert_eq!(scanned.body.shards.len(), 1, "dangling link skipped, live link followed");
    assert!(scanned.root.ends_with("snapshots/abc123"));
    assert_eq!(scanned.body.shards[0].size, 1000);
}

#[test]
fn unpinned_operator_key_cannot_adopt() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let reg = new_reg();
    let stranger = SigningKey::from_bytes(&[9u8; 32]);
    let (_, _, anchors) = operator();
    let scanned = scan_dir(&dir, input("Fake")).unwrap();
    let err = reg
        .adopt(scanned, &stranger, "ed25519:stranger", &anchors, false)
        .unwrap_err();
    assert!(
        matches!(err, ModelError::Verify(VerifyError::UntrustedSigner { .. })),
        "{err}"
    );
    assert!(reg.entries().is_empty());
}

#[test]
fn hash_mismatch_is_refused_and_stays_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let reg = new_reg();
    adopt_fake(&reg, &dir, "Fake");
    let shard = dir.join("model-00002-of-00002.safetensors");
    // Same size, different bytes, and a changed stamp so the lazy check looks.
    fs::write(&shard, vec![3u8; 2048]).unwrap();
    bump_mtime(&shard);
    let err = reg.resolve("Fake").unwrap_err();
    assert!(matches!(err, ModelError::NotReady { .. }), "{err}");
    assert!(err.to_string().contains("refused"), "{err}");
    // Restoring the bytes does not clear a refusal by itself ...
    fs::write(&shard, vec![2u8; 2048]).unwrap();
    assert!(reg.resolve("Fake").is_err());
    // ... only an operator-run full check that passes does.
    assert!(reg.check("Fake", CheckMode::Full).unwrap().state.is_ready());
    assert!(reg.resolve("Fake").is_ok());
}

#[test]
fn size_mismatch_is_a_mismatch_not_a_missing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let reg = new_reg();
    adopt_fake(&reg, &dir, "Fake");
    fs::write(dir.join("model-00001-of-00002.safetensors"), b"short").unwrap();
    let c = reg.check("Fake", CheckMode::Stat).unwrap();
    assert!(matches!(c.state, ModelState::Refused { .. }), "{:?}", c.state);
}

#[test]
fn rehash_is_lazy_but_full_always_looks() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let reg = new_reg();
    adopt_fake(&reg, &dir, "Fake");
    let rehashed = |c: &ModelCheck| {
        c.files.iter().filter(|(_, o)| *o == FileOutcome::Ok { rehashed: true }).count()
    };
    assert_eq!(rehashed(&reg.check("Fake", CheckMode::Lazy).unwrap()), 0, "stamps match");
    assert_eq!(rehashed(&reg.check("Fake", CheckMode::Stat).unwrap()), 0);
    // A touched file (same bytes, new mtime) is re-hashed once, then trusted.
    bump_mtime(&dir.join("tokenizer.json"));
    assert_eq!(rehashed(&reg.check("Fake", CheckMode::Lazy).unwrap()), 1);
    assert_eq!(rehashed(&reg.check("Fake", CheckMode::Lazy).unwrap()), 0);
    assert_eq!(rehashed(&reg.check("Fake", CheckMode::Full).unwrap()), 4);
}

#[test]
fn registry_persists_and_reopens() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let path = tmp.path().join("run/models/registry.json");
    let id = {
        let reg = ModelRegistry::open(&path).unwrap();
        adopt_fake(&reg, &dir, "Fake").0.package_id
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let reopened = ModelRegistry::open(&path).unwrap();
    reopened.set_trust(ModelTrust::new(operator().2));
    assert_eq!(reopened.resolve("Fake").unwrap().package_id, id);
    assert_eq!(reopened.resolve(&id).unwrap().name, "Fake");
    reopened.remove("Fake").unwrap();
    assert!(matches!(reopened.resolve("Fake"), Err(ModelError::Unknown(_))));
    assert!(dir.join("tokenizer.json").exists(), "removing from the registry keeps the files");
}

#[test]
fn name_conflict_needs_replace() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    fake_model(&a);
    fake_model(&b);
    fs::write(b.join("model-00002-of-00002.safetensors"), vec![5u8; 2048]).unwrap();
    let reg = new_reg();
    let (_, key, anchors) = adopt_fake(&reg, &a, "Fake");
    let kid = key_id_for(&key.verifying_key().to_bytes());
    let again = scan_dir(&b, input("Fake")).unwrap();
    let err = reg.adopt(again.clone(), &key, &kid, &anchors, false).unwrap_err();
    assert!(matches!(err, ModelError::NameConflict(_)), "{err}");
    reg.adopt(again, &key, &kid, &anchors, true).unwrap();
    assert_eq!(reg.entries().len(), 1);
}

#[test]
fn attached_manifest_is_hash_checked_on_first_use() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let src = new_reg();
    let (adopted, _, anchors) = adopt_fake(&src, &dir, "Fake");
    let other = new_reg();
    other.attach(&adopted.envelope, &dir, &anchors, false).unwrap();
    assert!(other.resolve("Fake").is_ok(), "hashes verify lazily on first resolve");
    let (_, _, bad_anchors) = {
        let k = SigningKey::from_bytes(&[1u8; 32]);
        let pk = k.verifying_key().to_bytes();
        let mut a = TrustAnchors::default();
        a.push_signer("other", &hex_encode(&pk), KeyOrigin::Operator).unwrap();
        (k, (), a)
    };
    assert!(ModelRegistry::in_memory().attach(&adopted.envelope, &dir, &bad_anchors, false).is_err());
    fs::write(dir.join("model-00001-of-00002.safetensors"), vec![8u8; 4096]).unwrap();
    bump_mtime(&dir.join("model-00001-of-00002.safetensors"));
    assert!(other.resolve("Fake").is_err());
}

#[test]
fn flipping_redistributable_breaks_the_signature() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let reg = new_reg();
    let (adopted, _, anchors) = adopt_fake(&reg, &dir, "Fake");
    let mut env = adopted.envelope.clone();
    env.body["redistributable"] = serde_json::json!(true);
    let err = verify_model(&env, &anchors).unwrap_err();
    assert!(
        matches!(err, ModelError::Verify(VerifyError::BadSignature { .. })),
        "{err}"
    );
}

#[test]
fn body_validation_rejects_bad_shapes() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let good = scan_dir(&dir, input("Fake")).unwrap().body;
    let mut b = good.clone();
    b.shards.reverse();
    assert!(b.validate().is_err(), "unsorted");
    let mut b = good.clone();
    b.shards[0].blake3 = "XYZ".into();
    assert!(b.validate().is_err(), "bad hash");
    let mut b = good.clone();
    b.shards[0].path = "../escape.safetensors".into();
    assert!(b.validate().is_err(), "traversal");
    let mut b = good.clone();
    b.source = ModelSource::default();
    assert!(b.validate().is_err(), "no source");
    let mut b = good.clone();
    b.shards.clear();
    assert!(b.validate().is_err(), "no shards");
    let mut b = good;
    b.name = "bad name".into();
    assert!(b.validate().is_err(), "name");
    let text = r#"{"name":"x","format":"mlx","shards":[],"source":{"ollama_tag":"x:1"},"surprise":1}"#;
    assert!(serde_json::from_str::<ModelPackageBody>(text).is_err(), "unknown field");
}

#[test]
fn scan_rejects_directories_without_weights() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("readme.txt"), b"x").unwrap();
    assert!(matches!(scan_dir(tmp.path(), input("Empty")), Err(ModelError::Scan(_))));
}

#[test]
fn scans_a_single_gguf() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("Tiny-Q4.gguf");
    fs::write(&f, vec![4u8; 512]).unwrap();
    let mut inp = input("Tiny-Q4");
    inp.format = ModelFormat::Gguf;
    let s = scan_file(&f, inp).unwrap();
    assert_eq!(s.body.shards.len(), 1);
    assert_eq!(s.body.shards[0].path, "Tiny-Q4.gguf");
    assert_eq!(s.root, tmp.path().canonicalize().unwrap());
}

fn fake_ollama(dir: &Path) -> (String, String) {
    let model = vec![6u8; 700];
    let tmpl = b"{{ .Prompt }}".to_vec();
    let digest = |b: &[u8]| format!("{:0>64}", hex_encode(&b[..b.len().min(32)]));
    let (md, td) = (digest(&model), digest(&tmpl));
    fs::create_dir_all(dir.join("blobs")).unwrap();
    fs::write(dir.join(format!("blobs/sha256-{md}")), &model).unwrap();
    fs::write(dir.join(format!("blobs/sha256-{td}")), &tmpl).unwrap();
    let mdir = dir.join("manifests/registry.ollama.ai/library/tiny");
    fs::create_dir_all(&mdir).unwrap();
    let manifest = serde_json::json!({"layers":[
        {"mediaType":"application/vnd.ollama.image.model","digest":format!("sha256:{md}"),"size":700},
        {"mediaType":"application/vnd.ollama.image.template","digest":format!("sha256:{td}"),"size":tmpl.len()},
        {"mediaType":"application/vnd.ollama.image.params","digest":format!("sha256:{td}"),"size":1},
    ]});
    fs::write(mdir.join("latest"), manifest.to_string()).unwrap();
    (md, td)
}

#[test]
fn adopts_ollama_blobs_through_their_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let (md, _) = fake_ollama(tmp.path());
    let mut inp = input("tiny");
    inp.source = ModelSource::default();
    let s = scan_ollama(tmp.path(), "tiny", "latest", inp).unwrap();
    assert_eq!(s.body.format, ModelFormat::Ollama);
    assert_eq!(s.body.source.ollama_tag.as_deref(), Some("tiny:latest"));
    assert_eq!(s.body.shards[0].path, format!("blobs/sha256-{md}"));
    assert!(s.body.template_blake3.is_some());
    // A size that disagrees with the Ollama manifest is refused.
    fs::write(tmp.path().join(format!("blobs/sha256-{md}")), vec![6u8; 701]).unwrap();
    let err = scan_ollama(tmp.path(), "tiny", "latest", input("tiny")).unwrap_err();
    assert!(matches!(err, ModelError::HashMismatch { .. }), "{err}");
    assert!(scan_ollama(tmp.path(), "../x", "latest", input("tiny")).is_err());
}

// ── advertising ──────────────────────────────────────────────────

fn external_setup() -> (tempfile::TempDir, PathBuf, PathBuf, TierResolver) {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let volumes = base.join("Volumes");
    let drive = volumes.join("Drive");
    let dir = drive.join("quant");
    fake_model(&dir);
    let tiers = TierResolver::with_external_roots(vec![volumes]);
    (tmp, drive, dir, tiers)
}

fn shard_list(c: &clawft_types::placement::Capability) -> Vec<String> {
    match c.attrs.get("shards") {
        Some(AttrValue::List(l)) => l
            .iter()
            .filter_map(|v| if let AttrValue::Str(s) = v { Some(s.clone()) } else { None })
            .collect(),
        _ => vec![],
    }
}

#[test]
fn model_present_is_available_when_complete() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let reg = new_reg();
    let (adopted, _, _) = adopt_fake(&reg, &dir, "Fake");
    let caps = model_capabilities(&reg, &TierResolver::with_external_roots(vec![]));
    assert_eq!(caps.len(), 1);
    let c = &caps[0];
    assert_eq!(c.id.as_str(), "model.present");
    assert_eq!(c.state, CapabilityState::Available);
    let list = shard_list(c);
    assert_eq!(list[0], format!("model:{}", adopted.package_id));
    assert_eq!(list.len(), 3, "marker plus two shard hashes");
    let json = serde_json::to_string(c).unwrap();
    assert!(!json.contains("quant") && !json.contains(&tmp.path().display().to_string()));
}

#[test]
fn partial_holding_is_degraded_and_unmarked() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let reg = new_reg();
    adopt_fake(&reg, &dir, "Fake");
    fs::remove_file(dir.join("model-00002-of-00002.safetensors")).unwrap();
    let caps = model_capabilities(&reg, &TierResolver::with_external_roots(vec![]));
    let c = &caps[0];
    assert_eq!(c.state, CapabilityState::Degraded);
    let list = shard_list(c);
    assert_eq!(list.len(), 1);
    assert!(!list[0].starts_with("model:"));
}

#[test]
fn unmounting_the_drive_changes_advertised_state() {
    let (tmp, drive, dir, tiers) = external_setup();
    let reg = new_reg();
    adopt_fake(&reg, &dir, "Fake");
    let mounted = model_capabilities(&reg, &tiers);
    assert_eq!(mounted.len(), 1, "no store marker while the drive is there");
    assert_eq!(mounted[0].state, CapabilityState::Available);
    // Eject: the drive directory disappears.
    fs::rename(&drive, tmp.path().join("ejected")).unwrap();
    let c = reg.check("Fake", CheckMode::Lazy).unwrap();
    assert!(matches!(c.state, ModelState::Degraded { detached: true, .. }), "{:?}", c.state);
    let err = reg.resolve("Fake").unwrap_err();
    assert!(err.to_string().contains("detached"), "{err}");
    let gone = model_capabilities(&reg, &tiers);
    let model = gone.iter().find(|c| c.id.as_str() == "model.present").unwrap();
    assert_eq!(model.state, CapabilityState::Degraded);
    let store = gone.iter().find(|c| c.id.as_str() == "store.tier.external").unwrap();
    assert_eq!(store.attrs.get("mounted"), Some(&AttrValue::Bool(false)));
    assert_eq!(store.state, CapabilityState::Degraded);
    // Back: Ready again, no refusal recorded for a drive that was merely away.
    fs::rename(tmp.path().join("ejected"), &drive).unwrap();
    assert!(reg.resolve("Fake").is_ok());
    assert_eq!(model_capabilities(&reg, &tiers).len(), 1);
}

#[test]
fn tier_resolver_classifies_by_mount_parent() {
    let t = TierResolver::with_external_roots(vec![PathBuf::from("/Volumes")]);
    let (tier, mount) = t.classify(Path::new("/Volumes/Big/models/x"));
    assert_eq!(tier, StoreTier::External);
    assert_eq!(mount, Some(PathBuf::from("/Volumes/Big")));
    assert_eq!(t.classify(Path::new("/Users/someone/models")).0, StoreTier::Internal);
    assert_eq!(t.classify(Path::new("/Volumes")).0, StoreTier::Internal);
}
