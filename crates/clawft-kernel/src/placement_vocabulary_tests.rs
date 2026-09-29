//! Tests for the governed vocabulary path (ADR-099 decision 6).

use std::path::PathBuf;
use std::sync::Mutex;

use super::*;

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

fn pin_text(v: &Vocabulary) -> String {
    VocabularyPin::of(v).to_toml_string()
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
fn free_edits_do_not_load() {
    let cur = Vocabulary::from_toml_str(V1).unwrap();
    let pin = pin_text(&cur);
    assert!(load_governed(V1, &pin).is_ok());
    // Edited content, with or without a version bump, is refused.
    for edited in [V1.replace("\"Metal\"", "\"Metal!\""), v2()] {
        let err = load_governed(&edited, &pin).unwrap_err();
        assert!(
            matches!(err, KernelError::Config(ref m) if m.contains("digest")),
            "{err}"
        );
    }
    assert!(load_governed(V1, "not a pin").is_err());
}

#[test]
fn load_governed_dir_refuses_a_tampered_copy() {
    let dir = std::env::temp_dir().join(format!("weftos-vocab-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cur = Vocabulary::from_toml_str(V1).unwrap();
    std::fs::write(dir.join(VOCABULARY_PIN_FILE), pin_text(&cur)).unwrap();
    std::fs::write(dir.join(VOCABULARY_FILE), V1).unwrap();
    assert!(load_governed_dir(&dir).is_ok());
    std::fs::write(dir.join(VOCABULARY_FILE), v2()).unwrap();
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
    let gov = VocabularyGovernor::new(gate.clone()).with_chain(chain.clone());
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

    // The issued pin loads the approved text and nothing else.
    let pin = ok.pin.to_toml_string();
    assert!(load_governed(&next, &pin).is_ok());
    assert!(load_governed(V1, &pin).is_err());
    assert_eq!(ok.change.from_version, 1);
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
        let gov = VocabularyGovernor::new(FixedGate::new(decision)).with_chain(chain.clone());
        let err = gov.propose("agent-7", &cur, &v2()).unwrap_err();
        assert!(matches!(err, KernelError::GovernanceDenied(_)), "{err}");
        assert_eq!(chain.len(), before, "nothing chained");
    }
}

#[test]
fn malformed_or_unbumped_proposals_never_reach_the_gate() {
    let gate = FixedGate::new(permit());
    let gov = VocabularyGovernor::new(gate.clone());
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
