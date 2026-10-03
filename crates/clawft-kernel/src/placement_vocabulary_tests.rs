//! Tests for the governed vocabulary path (ADR-099 decision 6).

use std::path::PathBuf;
use std::sync::Mutex;

use ed25519_dalek::SigningKey;

use super::*;
use crate::workload_pkg::KeyOrigin;

fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn anchors_for(k: &SigningKey) -> TrustAnchors {
    let mut a = TrustAnchors::default();
    let id = key_id_for(k.verifying_key().as_bytes());
    a.push_signer(
        &id,
        &hex_encode(k.verifying_key().as_bytes()),
        KeyOrigin::Operator,
    )
    .unwrap();
    a
}

fn governor(gate: Arc<FixedGate>, chain: &Arc<ChainManager>) -> VocabularyGovernor {
    VocabularyGovernor::new(gate)
        .with_chain(chain.clone())
        .with_signer(signing_key())
}

/// A pin as the baseline: version 1, genesis event, no signature.
fn baseline_pin() -> String {
    let mut p = VocabularyPin::of(&Vocabulary::from_toml_str(V1).unwrap());
    p.event_hash = Some(GENESIS_EVENT.into());
    p.to_toml_string()
}

struct FixedGate {
    decision: GateDecision,
    seen: Mutex<Vec<(String, String, serde_json::Value)>>,
}

impl FixedGate {
    fn new(decision: GateDecision) -> Arc<Self> {
        Arc::new(Self {
            decision,
            seen: Mutex::new(Vec::new()),
        })
    }
}

impl GateBackend for FixedGate {
    fn check(&self, agent_id: &str, action: &str, context: &serde_json::Value) -> GateDecision {
        self.seen
            .lock()
            .unwrap()
            .push((agent_id.into(), action.into(), context.clone()));
        self.decision.clone()
    }
}

fn permit() -> GateDecision {
    GateDecision::Permit { token: None }
}

const V1: &str = r#"
[meta]
version = 1
title = "t"

[families.accel]
title = "Accelerators"
order = 1

[ids."accel.gpu.metal"]
summary = "Metal"
"#;

fn v2() -> String {
    V1.replace("version = 1", "version = 2") + "\n[ids.\"accel.gpu.cuda\"]\nsummary = \"CUDA\"\n"
}

fn repo_config() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config")
}

#[test]
fn committed_vocabulary_loads_through_its_pin() {
    let v = load_governed_dir(&repo_config()).expect("committed vocabulary matches its pin");
    assert!(v.lookup("mem.unified").is_some());
}

#[test]
fn committed_pin_is_the_compiled_in_baseline() {
    let v = load_governed_dir(&repo_config()).unwrap();
    assert_eq!(v.digest(), BASELINE_VOCABULARY_DIGEST);
}

#[test]
fn free_edits_do_not_load() {
    let a = TrustAnchors::default();
    // The baseline test fixture is not the compiled-in baseline digest, so a
    // genesis pin over it is refused: a self-made pin proves nothing.
    assert!(load_governed(V1, &baseline_pin(), &a, 0).is_err());
    assert!(load_governed(V1, "not a pin", &a, 0).is_err());
}

#[test]
fn editing_the_vocabulary_and_pin_together_does_not_forge_one() {
    // The old attack: change both files so the digest agrees.
    let a = TrustAnchors::weftos_default().unwrap();
    let repo = std::fs::read_to_string(repo_config().join(VOCABULARY_FILE)).unwrap();
    let edited = repo.replace("version = 1", "version = 2") + "\n# tampered\n";
    let ev = Vocabulary::from_toml_str(&edited).unwrap();
    // Unsigned pin for the edited file, with and without a genesis claim.
    let mut forged = VocabularyPin::of(&ev);
    assert!(load_governed(&edited, &forged.to_toml_string(), &a, 0).is_err());
    forged.event_hash = Some(GENESIS_EVENT.into());
    assert!(load_governed(&edited, &forged.to_toml_string(), &a, 0).is_err());
    // A signature from a key that is not pinned is refused.
    let rogue = signing_key();
    forged.key_id = Some(key_id_for(rogue.verifying_key().as_bytes()));
    forged.event_seq = Some(3);
    forged.event_hash = Some("ab".repeat(32));
    forged.signature = Some(hex_encode(
        &rogue.sign(&forged.signed_statement()).to_bytes(),
    ));
    let err = load_governed(&edited, &forged.to_toml_string(), &a, 0).unwrap_err();
    assert!(err.to_string().contains("not a trusted key"), "{err}");
}

#[test]
fn load_governed_dir_refuses_a_tampered_copy() {
    let dir = std::env::temp_dir().join(format!("weftos-vocab-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let repo = repo_config();
    let vocab = std::fs::read_to_string(repo.join(VOCABULARY_FILE)).unwrap();
    std::fs::write(
        dir.join(VOCABULARY_PIN_FILE),
        std::fs::read(repo.join(VOCABULARY_PIN_FILE)).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join(VOCABULARY_FILE), &vocab).unwrap();
    assert!(load_governed_dir(&dir).is_ok());
    std::fs::write(dir.join(VOCABULARY_FILE), vocab + "\n# edit\n").unwrap();
    assert!(load_governed_dir(&dir).is_err());
    std::fs::remove_file(dir.join(VOCABULARY_PIN_FILE)).unwrap();
    assert!(load_governed_dir(&dir).is_err(), "no pin, no load");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn permitted_change_is_gated_chained_and_pinned() {
    let gate = FixedGate::new(permit());
    let chain = Arc::new(ChainManager::new(0, 1000));
    let before = chain.len();
    let gov = governor(gate.clone(), &chain);
    let cur = Vocabulary::from_toml_str(V1).unwrap();
    let next = v2();

    let ok = gov.propose("operator", &cur, &next).unwrap();

    let seen = gate.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let (who, action, ctx) = &seen[0];
    assert_eq!((who.as_str(), action.as_str()), ("operator", "config.set"));
    assert_eq!(ctx["namespace"], "placement");
    assert_eq!(ctx["key"], "capabilities.toml");
    assert_eq!(ctx["from_digest"], cur.digest());
    assert_eq!(ctx["to_version"], 2);
    assert!(ctx.get("effect").is_some(), "effect vector attached");

    assert_eq!(chain.len(), before + 1);
    let ev = chain.tail(1).pop().unwrap();
    assert_eq!(
        (ev.source.as_str(), ev.kind.as_str()),
        ("placement", "config.set")
    );
    let payload = ev.payload.unwrap();
    assert_eq!(payload["change"]["to_digest"], ok.pin.digest);
    assert_eq!(payload["pin"]["version"], 2);

    // The issued pin is signed, bound to that very event, and loads the
    // approved text and nothing else.
    let pin = ok.pin.to_toml_string();
    let anchors = anchors_for(&signing_key());
    assert!(load_governed(&next, &pin, &anchors, 0).is_ok());
    assert!(load_governed(V1, &pin, &anchors, 0).is_err());
    assert_eq!(ok.change.from_version, 1);
    assert_eq!(ok.pin.event_seq, Some(ev.sequence));
    assert_eq!(
        ok.pin.event_hash.as_deref(),
        Some(hex_encode(&ev.hash).as_str())
    );
    verify_pin_event(&ok.pin, &chain).unwrap();
}

#[test]
fn forged_pins_are_rejected() {
    let chain = Arc::new(ChainManager::new(0, 1000));
    let gov = governor(FixedGate::new(permit()), &chain);
    let cur = Vocabulary::from_toml_str(V1).unwrap();
    let ok = gov.propose("operator", &cur, &v2()).unwrap();
    let anchors = anchors_for(&signing_key());
    let next = v2();

    // Change the signed content: version or digest edited after signing.
    let mut bumped = ok.pin.clone();
    bumped.version = 9;
    assert!(load_governed(&next, &bumped.to_toml_string(), &anchors, 0).is_err());
    let mut other = ok.pin.clone();
    other.digest = "00".repeat(32);
    assert!(load_governed(&next, &other.to_toml_string(), &anchors, 0).is_err());
    // Rebind to another event: the signature covers the event.
    let mut moved = ok.pin.clone();
    moved.event_hash = Some("cd".repeat(32));
    let err = load_governed(&next, &moved.to_toml_string(), &anchors, 0).unwrap_err();
    assert!(err.to_string().contains("does not verify"), "{err}");
    // Stripped signature or event binding.
    let mut unsigned = ok.pin.clone();
    unsigned.signature = None;
    assert!(load_governed(&next, &unsigned.to_toml_string(), &anchors, 0).is_err());
    let mut unbound = ok.pin.clone();
    unbound.event_hash = None;
    assert!(load_governed(&next, &unbound.to_toml_string(), &anchors, 0).is_err());
    // Untrusted signer.
    let none = TrustAnchors::default();
    assert!(load_governed(&next, &ok.pin.to_toml_string(), &none, 0).is_err());
    // Pin claims an event the chain does not hold (or holds differently).
    let other_chain = ChainManager::new(0, 1000);
    assert!(verify_pin_event(&ok.pin, &other_chain).is_err());
    let mut wrong = ok.pin.clone();
    wrong.event_hash = Some("ef".repeat(32));
    assert!(verify_pin_event(&wrong, &chain).is_err());
}

#[test]
fn governor_without_a_chain_or_signer_refuses_to_issue_a_pin() {
    let cur = Vocabulary::from_toml_str(V1).unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let gate = FixedGate::new(permit());
    let no_chain = VocabularyGovernor::new(gate.clone()).with_signer(signing_key());
    let no_key = VocabularyGovernor::new(gate.clone()).with_chain(chain.clone());
    let bare = VocabularyGovernor::new(gate.clone());
    for gov in [no_chain, no_key, bare] {
        let err = gov.propose("operator", &cur, &v2()).unwrap_err();
        assert!(err.to_string().contains("refusing to issue a pin"), "{err}");
    }
    assert!(gate.seen.lock().unwrap().is_empty(), "gate not consulted");
}

#[test]
fn denied_or_deferred_change_issues_nothing() {
    let cur = Vocabulary::from_toml_str(V1).unwrap();
    for decision in [
        GateDecision::Deny {
            reason: "no".into(),
            receipt: None,
        },
        GateDecision::Defer {
            reason: "needs a human".into(),
        },
    ] {
        let chain = Arc::new(ChainManager::new(0, 1000));
        let before = chain.len();
        let gov = governor(FixedGate::new(decision), &chain);
        let err = gov.propose("agent-7", &cur, &v2()).unwrap_err();
        assert!(matches!(err, KernelError::GovernanceDenied(_)), "{err}");
        assert_eq!(chain.len(), before, "nothing chained");
    }
}

#[test]
fn malformed_or_unbumped_proposals_never_reach_the_gate() {
    let gate = FixedGate::new(permit());
    let chain = Arc::new(ChainManager::new(0, 1000));
    let gov = governor(gate.clone(), &chain);
    let cur = Vocabulary::from_toml_str(V1).unwrap();
    let same_version = V1.replace("\"Metal\"", "\"Metal GPU\"");
    for bad in ["not toml [", same_version.as_str(), V1] {
        assert!(matches!(
            gov.propose("operator", &cur, bad),
            Err(KernelError::Config(_))
        ));
    }
    assert!(gate.seen.lock().unwrap().is_empty());
}

fn write_config(dir: &Path, vocab: &str, pin: &str) {
    std::fs::write(dir.join(VOCABULARY_FILE), vocab).unwrap();
    std::fs::write(dir.join(VOCABULARY_PIN_FILE), pin).unwrap();
}

/// The committed baseline, a signed v2 from the governor, and its chain.
struct Rollback {
    base_vocab: String,
    base_pin: String,
    v2_vocab: String,
    v2_pin: String,
    chain: Arc<ChainManager>,
}

fn rollback_rig() -> Rollback {
    let repo = repo_config();
    let base_vocab = std::fs::read_to_string(repo.join(VOCABULARY_FILE)).unwrap();
    let base_pin = std::fs::read_to_string(repo.join(VOCABULARY_PIN_FILE)).unwrap();
    let cur = Vocabulary::from_toml_str(&base_vocab).unwrap();
    let v2_vocab = base_vocab.replace("version = 1", "version = 2") + "\n# v2\n";
    let chain = Arc::new(ChainManager::new(0, 1000));
    let ok = governor(FixedGate::new(permit()), &chain)
        .propose("operator", &cur, &v2_vocab)
        .unwrap();
    Rollback {
        base_vocab,
        base_pin,
        v2_vocab,
        v2_pin: ok.pin.to_toml_string(),
        chain,
    }
}

#[test]
fn rollback_to_the_baseline_after_a_signed_pin_is_refused() {
    let r = rollback_rig();
    let anchors = anchors_for(&signing_key());
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), &r.base_vocab, &r.base_pin);
    load_governed_dir_with(dir.path(), &anchors).expect("baseline loads before any signed pin");
    write_config(dir.path(), &r.v2_vocab, &r.v2_pin);
    load_governed_dir_with(dir.path(), &anchors).expect("signed v2 loads");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(VOCABULARY_FLOOR_FILE))
            .unwrap()
            .trim(),
        "2"
    );
    // The baseline pin and text, both internally consistent, are now refused.
    write_config(dir.path(), &r.base_vocab, &r.base_pin);
    let err = load_governed_dir_with(dir.path(), &anchors).unwrap_err();
    assert!(err.to_string().contains("rollback"), "{err}");
    // So is the pure check with the floor.
    assert!(load_governed(&r.base_vocab, &r.base_pin, &anchors, 2).is_err());
    assert!(load_governed(&r.v2_vocab, &r.v2_pin, &anchors, 2).is_ok());
}

#[test]
fn the_chain_floor_holds_when_the_floor_file_is_deleted() {
    let r = rollback_rig();
    let anchors = anchors_for(&signing_key());
    assert_eq!(chain_version_floor(&r.chain), 2);
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), &r.base_vocab, &r.base_pin);
    let err = load_governed_dir_on_chain(dir.path(), &anchors, &r.chain).unwrap_err();
    assert!(err.to_string().contains("rollback"), "{err}");
    // The signed v2 loads with its event verified on the chain.
    write_config(dir.path(), &r.v2_vocab, &r.v2_pin);
    load_governed_dir_on_chain(dir.path(), &anchors, &r.chain).unwrap();
    // A chain that does not hold the pin's event refuses it.
    let other = ChainManager::new(0, 1000);
    assert!(load_governed_dir_on_chain(dir.path(), &anchors, &other).is_err());
    // A garbled floor file fails closed.
    std::fs::write(dir.path().join(VOCABULARY_FLOOR_FILE), "banana").unwrap();
    assert!(load_governed_dir_with(dir.path(), &anchors).is_err());
}

#[test]
fn pin_event_payload_version_must_match_the_pin() {
    let chain = Arc::new(ChainManager::new(0, 1000));
    let cur = Vocabulary::from_toml_str(V1).unwrap();
    let ok = governor(FixedGate::new(permit()), &chain)
        .propose("operator", &cur, &v2())
        .unwrap();
    // An event with the right digest but another version, bound by a pin
    // that claims version 2.
    let ev = chain.append(
        "placement",
        EVENT_KIND_CONFIG_SET,
        Some(serde_json::json!({"pin": {"digest": ok.pin.digest, "version": 9}})),
    );
    let mut pin = ok.pin.clone();
    pin.event_seq = Some(ev.sequence);
    pin.event_hash = Some(hex_encode(&ev.hash));
    assert!(verify_pin_event(&pin, &chain).is_err());
    assert!(verify_pin_event(&ok.pin, &chain).is_ok());
}
