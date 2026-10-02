//! Package F, user-daemon side: `shared.*` served to a stub child over the
//! real socket path (ADR-103 Phase 2 F).
//!
//! The "user daemon" is an in-process kernel behind `handle_connection`; the
//! "child" is a `ParentLink` with a project token, the same client the
//! project profile installs. Embedding and LLM are stubs that record what
//! they were sent: no model is loaded and nothing touches the network.
//! This process never enters the project profile (that is
//! `shared_services_child.rs`), so `shared.*` is served here.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use clawft_kernel::boot::Kernel;
use clawft_kernel::embedding::{EmbeddingError, EmbeddingProvider};
use clawft_platform::NativePlatform;
use clawft_service_llm::{
    ChatRequest, ChatResponse, LlmBackend, LlmClient, LlmConfig, LlmError, share_llm_client,
};
use clawft_types::config::{ChainConfig, Config, KernelConfig};
use clawft_weave::parent_link::{Backoff, ParentError, ParentLink, ParentLlmBackend, RemoteEmbedder};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{RwLock, watch};

type KernelRef = Arc<RwLock<Kernel<NativePlatform>>>;

/// `scope_gate`, the manifest dir and the stub installs are process-global:
/// one test at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const PROMPT: &str = "the-secret-prompt-text-xyzzy";

// ── stubs ────────────────────────────────────────────────────────────────

struct StubEmbedder;

#[async_trait]
impl EmbeddingProvider for StubEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        Ok(vec![text.len() as f32, 1.0, 2.0, 3.0])
    }
    fn dimensions(&self) -> usize {
        4
    }
    fn model_name(&self) -> &str {
        "stub-embed"
    }
}

#[derive(Debug, Default)]
struct StubLlm {
    seen: Mutex<Vec<String>>,
}

#[async_trait]
impl LlmBackend for StubLlm {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let texts: Vec<String> = request.messages.iter().map(|m| m.content.as_text().into_owned()).collect();
        self.seen.lock().unwrap().extend(texts);
        Ok(serde_json::from_value(json!({
            "choices": [{"message": {"role": "assistant", "content": "stub reply"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10},
            "model": "stub-model"
        }))
        .unwrap())
    }
    async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        Ok(vec!["stub-model".into()])
    }
    async fn health(&self) -> Result<bool, LlmError> {
        Ok(true)
    }
}

fn stub_llm() -> &'static Arc<StubLlm> {
    static S: OnceLock<Arc<StubLlm>> = OnceLock::new();
    S.get_or_init(|| {
        let stub = Arc::new(StubLlm::default());
        let client = LlmClient::with_backend(LlmConfig::default(), stub.clone()).unwrap();
        clawft_weave::shared_rpc::install_llm(share_llm_client(client));
        stub
    })
}

// ── user daemon stand-in ─────────────────────────────────────────────────

struct Daemon {
    _tmp: tempfile::TempDir,
    sock: PathBuf,
    manifests: PathBuf,
    kernel: KernelRef,
    shutdown: watch::Sender<bool>,
}

async fn spawn() -> Daemon {
    stub_llm();
    clawft_weave::shared_rpc::install_embedder(Arc::new(StubEmbedder));
    let tmp = tempfile::tempdir().unwrap();
    let manifests = tmp.path().join("projects");
    clawft_weave::project_rpc::init_manifests_dir(manifests.clone());
    clawft_weave::scope_gate::init(Some(manifests.clone()), false);
    let kcfg = KernelConfig {
        chain: Some(ChainConfig::isolated_in(&tempfile::tempdir().unwrap().keep())),
        ..KernelConfig::default()
    };
    let kernel = Kernel::boot(Config::default(), kcfg, Arc::new(NativePlatform::new()))
        .await
        .expect("boot");
    let kernel: KernelRef = Arc::new(RwLock::new(kernel));
    let sock = tmp.path().join("kernel.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let (tx, mut rx) = watch::channel(false);
    let (k, t) = (Arc::clone(&kernel), tx.clone());
    tokio::spawn(async move {
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((s, _)) => {
                        tokio::spawn(clawft_weave::daemon::handle_connection(s, Arc::clone(&k), t.clone()));
                    }
                    Err(_) => break,
                },
                _ = rx.changed() => if *rx.borrow() { break; },
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    Daemon { _tmp: tmp, sock, manifests, kernel, shutdown: tx }
}

async fn rpc(sock: &Path, method: &str, params: Value, auth: Option<&str>, project: Option<&str>) -> Value {
    let (r, mut w) = UnixStream::connect(sock).await.unwrap().into_split();
    let mut req = json!({"id": "t", "method": method, "params": params});
    if let Some(a) = auth {
        req["auth"] = json!(a);
    }
    if let Some(p) = project {
        req["project"] = json!(p);
    }
    w.write_all(format!("{req}\n").as_bytes()).await.unwrap();
    let mut line = String::new();
    BufReader::new(r).read_line(&mut line).await.unwrap();
    serde_json::from_str(line.trim()).unwrap()
}

/// Register a project root and return its id; `shared` is appended to its
/// manifest as the `[shared]` limits table.
async fn register(d: &Daemon, shared: &str) -> String {
    let root = tempfile::tempdir().unwrap().keep();
    let r = rpc(&d.sock, "project.register", json!({"root": root, "name": "p"}), Some("admin"), None).await;
    assert_eq!(r["ok"], true, "{r}");
    let id = r["result"]["project"]["id"].as_str().unwrap().to_owned();
    if !shared.is_empty() {
        let path = d.manifests.join(format!("{id}.toml"));
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(&format!("\n[shared]\n{shared}\n"));
        std::fs::write(path, text).unwrap();
    }
    id
}

/// A project-scoped token, as the supervisor will hand a child in spawn.json.
async fn token_for(d: &Daemon, project: &str) -> String {
    let r = rpc(&d.sock, "auth.token.issue", json!({"label": "child", "project": project}), Some("admin"), None).await;
    assert_eq!(r["ok"], true, "{r}");
    r["result"]["secret"].as_str().unwrap().to_owned()
}

fn child(d: &Daemon, project: &str, token: &str) -> Arc<ParentLink> {
    Arc::new(
        ParentLink::new(d.sock.clone(), project.into(), token.into())
            .with_backoff(Backoff { attempts: 1, initial: Duration::from_millis(1), max: Duration::from_millis(1) }),
    )
}

fn shared_use_events(d: &Daemon, rt: &tokio::runtime::Handle) -> Vec<Value> {
    let k = rt.block_on(d.kernel.read());
    k.chain_manager()
        .unwrap()
        .tail(200)
        .into_iter()
        .filter(|e| e.kind == "shared.use")
        .map(|e| e.payload.unwrap())
        .collect()
}

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
    let before = stub_llm().seen.lock().unwrap().len();
    cb.complete(vec![clawft_service_llm::ChatMessage::user("beta question")], None, None).await.unwrap();
    let seen = stub_llm().seen.lock().unwrap().clone();
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
