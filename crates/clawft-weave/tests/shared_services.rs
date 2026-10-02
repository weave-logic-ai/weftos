//! Package F, user-daemon side: `shared.*` served to a stub child over the
//! real socket path (ADR-103 Phase 2 F). Harness in `common/mod.rs`.
//!
//! This process never enters the project profile (that is
//! `shared_services_child.rs`), so `shared.*` is served here.

mod common;

use std::sync::Arc;
use std::time::Duration;

use clawft_service_llm::{LlmClient, LlmConfig};
use clawft_weave::parent_link::{ParentError, ParentLlmBackend, RemoteEmbedder};
use common::*;
use serde_json::{Value, json};

// ── tests ────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_child_embeds_and_chats_through_the_parent_and_the_chain_has_no_text() {
    let _serial = SERIAL.lock().await;
    let d = spawn().await;
    let p = register(&d, "").await;
    let link = child(&d, &p, &token_for(&d, &p).await);

    let emb = RemoteEmbedder::connect(link.clone()).await;
    assert_eq!(clawft_core::embeddings::Embedder::dimension(&*emb), 4, "width learned from the parent");
    let v = clawft_core::embeddings::Embedder::embed(&*emb, PROMPT).await.unwrap();
    assert_eq!(v[0], PROMPT.len() as f32);

    let client = LlmClient::with_backend(LlmConfig::default(), Arc::new(ParentLlmBackend::new(link.clone()))).unwrap();
    let resp = client
        .complete(vec![clawft_service_llm::ChatMessage::user(PROMPT)], None, None)
        .await
        .unwrap();
    assert_eq!(resp.choices[0].message.content.as_text(), "stub reply");
    assert_eq!(client.list_models().await.unwrap(), vec!["stub-model".to_owned()]);
    assert!(link.probe().await);
    let h = link.health();
    assert_eq!((h.embeddings.as_str(), h.llm.as_str(), h.voice.as_str()), ("parent", "parent", "parent"));

    // The chain records who, which service and how many tokens, never text.
    let rt = tokio::runtime::Handle::current();
    let events = tokio::task::spawn_blocking(move || shared_use_events(&d, &rt)).await.unwrap();
    let mine: Vec<&Value> = events.iter().filter(|e| e["project_id"] == p.as_str()).collect();
    assert!(mine.iter().any(|e| e["service"] == "embeddings"), "{events:?}");
    let llm = mine.iter().find(|e| e["service"] == "llm").expect("llm use recorded");
    assert_eq!(llm["tokens"], 10);
    for e in &mine {
        let mut keys: Vec<&String> = e.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["project_id", "service", "tokens"]);
    }
    let all = serde_json::to_string(&events).unwrap();
    assert!(!all.contains("xyzzy") && !all.contains("stub reply"), "payload text on the chain: {all}");
}

#[tokio::test(flavor = "multi_thread")]
async fn stopped_parent_gives_parent_unavailable_never_a_local_result() {
    let _serial = SERIAL.lock().await;
    let d = spawn().await;
    let p = register(&d, "").await;
    let link = child(&d, &p, &token_for(&d, &p).await);
    assert!(link.probe().await);

    d.shutdown.send(true).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let err = link.call(clawft_weave::parent_link::Service::Embeddings, "shared.embed", json!({"texts": ["x"]})).await.unwrap_err();
    assert!(matches!(err, ParentError::Unavailable(_)), "{err}");
    assert_eq!(err.kind(), "parent_unavailable");
    let e = RemoteEmbedder::new(link.clone());
    let msg = clawft_core::embeddings::Embedder::embed(&e, "x").await.unwrap_err().to_string();
    assert!(msg.contains("parent_unavailable"), "{msg}");
    assert_eq!(link.health().embeddings, "down");
}

#[tokio::test(flavor = "multi_thread")]
async fn identity_rules_token_scope_operator_and_anonymous() {
    let _serial = SERIAL.lock().await;
    let d = spawn().await;
    let (a, b) = (register(&d, "").await, register(&d, "").await);
    let tok_a = token_for(&d, &a).await;

    // Token scoped to A, request claims B: refused, kind project_scope_mismatch.
    let r = rpc(&d.sock, "shared.embed", json!({"texts": ["x"]}), Some(&tok_a), Some(&b)).await;
    assert_eq!(r["error_kind"], "project_scope_mismatch", "{r}");
    // ... and naming B in params is the same refusal.
    let r = rpc(&d.sock, "shared.embed", json!({"texts": ["x"], "project_id": b}), Some(&tok_a), None).await;
    assert_eq!(r["error_kind"], "project_scope_mismatch", "{r}");
    // Scoped to A, no claim: served as A.
    let r = rpc(&d.sock, "shared.embed", json!({"texts": ["x"]}), Some(&tok_a), None).await;
    assert_eq!(r["ok"], true, "{r}");

    // Anonymous and read-only callers do not reach the handler.
    for auth in [None, Some("read"), Some("chat")] {
        let r = rpc(&d.sock, "shared.embed", json!({"texts": []}), auth, Some(&a)).await;
        assert_eq!(r["ok"], false, "{r}");
        assert!(r["error"].as_str().unwrap().contains("permission denied"), "{r}");
    }
    // A forged token is not a project.
    let r = rpc(&d.sock, "shared.embed", json!({"texts": []}), Some("wft_forged"), Some(&a)).await;
    assert_eq!(r["ok"], false, "{r}");

    // An operator must name the project; naming an unregistered one fails.
    let r = rpc(&d.sock, "shared.embed", json!({"texts": []}), Some("admin"), None).await;
    assert_eq!(r["error_kind"], "bad_params", "{r}");
    let r = rpc(&d.sock, "shared.embed", json!({"texts": [], "project_id": "01J0000000000000000000000A"}), Some("admin"), None).await;
    assert_eq!(r["error_kind"], "project_unknown", "{r}");
    let r = rpc(&d.sock, "shared.embed", json!({"texts": [], "project_id": a}), Some("admin"), None).await;
    assert_eq!(r["ok"], true, "{r}");
    // A token scoped to an unregistered project is refused too.
    let ghost = "01J0000000000000000000000B";
    let tok = token_for(&d, ghost).await;
    let r = rpc(&d.sock, "shared.embed", json!({"texts": []}), Some(&tok), None).await;
    assert_eq!(r["error_kind"], "project_unknown", "{r}");
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limit_and_token_budget_refuse_including_many_small_calls() {
    let _serial = SERIAL.lock().await;
    let d = spawn().await;

    // Rate: 3 per minute.
    let p = register(&d, "max_calls_per_min = 3").await;
    let tok = token_for(&d, &p).await;
    for _ in 0..3 {
        let r = rpc(&d.sock, "shared.embed", json!({"texts": ["x"]}), Some(&tok), None).await;
        assert_eq!(r["ok"], true, "{r}");
    }
    let r = rpc(&d.sock, "shared.embed", json!({"texts": ["x"]}), Some(&tok), None).await;
    assert_eq!(r["error_kind"], "rate_limited", "{r}");

    // Budget: 40 tokens; each one-token call is charged the 8-token minimum,
    // so five calls fit and the sixth is refused with the rate limit unused.
    let q = register(&d, "token_budget = 40\nmax_calls_per_min = 1000").await;
    let tq = token_for(&d, &q).await;
    let mut ok = 0;
    let kind = loop {
        let r = rpc(&d.sock, "shared.embed", json!({"texts": ["x"]}), Some(&tq), None).await;
        if r["ok"] == true {
            ok += 1;
            assert!(ok <= 5, "budget never bound");
        } else {
            break r["error_kind"].as_str().unwrap().to_owned();
        }
    };
    assert_eq!((ok, kind.as_str()), (5, "budget_exceeded"));

    // A single large request is refused up front, and over the same budget.
    let big = "y".repeat(4 * 1000);
    let r = rpc(&d.sock, "shared.embed", json!({"texts": [big]}), Some(&tq), None).await;
    assert_eq!(r["ok"], false, "{r}");

    // One project's exhaustion does not touch another's.
    let r = rpc(&d.sock, "shared.embed", json!({"texts": ["x"]}), Some(&token_for(&d, &register(&d, "").await).await), None).await;
    assert_eq!(r["ok"], true, "{r}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_only_gets_context_it_sent() {
    let _serial = SERIAL.lock().await;
    let d = spawn().await;
    let (a, b) = (register(&d, "").await, register(&d, "").await);
    let (ta, tb) = (token_for(&d, &a).await, token_for(&d, &b).await);
    let la = child(&d, &a, &ta);
    let lb = child(&d, &b, &tb);
    let ca = LlmClient::with_backend(LlmConfig::default(), Arc::new(ParentLlmBackend::new(la))).unwrap();
    let cb = LlmClient::with_backend(LlmConfig::default(), Arc::new(ParentLlmBackend::new(lb))).unwrap();
    ca.complete(vec![clawft_service_llm::ChatMessage::user("alpha-only-context")], None, None).await.unwrap();

    // B's request reaches the model with exactly B's messages.
    let before = stub_llm().0.seen.lock().unwrap().len();
    cb.complete(vec![clawft_service_llm::ChatMessage::user("beta question")], None, None).await.unwrap();
    let seen = stub_llm().0.seen.lock().unwrap().clone();
    assert_eq!(&seen[before..], ["beta question"], "B must see only its own messages");

    // There is no way to ask for stored context: conversation, session and
    // memory params are refused, not ignored.
    for key in ["conv_id", "session", "context_of", "memory"] {
        let mut params = json!({"messages": [{"role": "user", "content": "hi"}]});
        params[key] = json!(a);
        let r = rpc(&d.sock, "shared.llm.chat", params, Some(&tb), None).await;
        assert_eq!(r["error_kind"], "bad_params", "{key}: {r}");
    }
    // Streaming is not offered.
    let r = rpc(&d.sock, "shared.llm.chat", json!({"messages": [{"role": "user", "content": "hi"}], "stream": true}), Some(&tb), None).await;
    assert_eq!(r["error_kind"], "bad_params", "{r}");
}

/// Review S10c / card 3503efb6: limits are cached, but an archived or
/// unregistered project is no longer served, with no `shared.reload`.
#[tokio::test(flavor = "multi_thread")]
async fn an_archived_or_unregistered_project_stops_being_served_without_a_reload() {
    use clawft_types::project::{ProjectState, read_manifest, write_manifest};
    let _serial = SERIAL.lock().await;
    let d = spawn().await;
    let (p, q) = (register(&d, "").await, register(&d, "").await);
    let (tp, tq) = (token_for(&d, &p).await, token_for(&d, &q).await);
    for tok in [&tp, &tq] {
        let r = rpc(&d.sock, "shared.embed", json!({"texts": ["x"]}), Some(tok), None).await;
        assert_eq!(r["ok"], true, "{r}");
    }
    // Archive p (what `weft project init --fork --force` leaves behind) and
    // unregister q (the manifest file is gone).
    let mut m = read_manifest(&d.manifests, &p).unwrap().unwrap();
    m.state = ProjectState::Archived;
    write_manifest(&d.manifests, &m).unwrap();
    std::fs::remove_file(d.manifests.join(format!("{q}.toml"))).unwrap();
    for tok in [&tp, &tq] {
        let r = rpc(&d.sock, "shared.embed", json!({"texts": ["x"]}), Some(tok), None).await;
        assert_eq!(r["error_kind"], "project_unknown", "{r}");
    }
    // Registering the project again (it is active) serves it once more.
    m.state = ProjectState::Active;
    write_manifest(&d.manifests, &m).unwrap();
    let r = rpc(&d.sock, "shared.embed", json!({"texts": ["x"]}), Some(&tp), None).await;
    assert_eq!(r["ok"], true, "{r}");
}
