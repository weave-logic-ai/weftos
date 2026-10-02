//! Shared harness for the `shared.*` integration tests: a user-daemon
//! stand-in, a stub embedder and LLM that record what they receive, and
//! project/token helpers. No model is loaded and nothing touches the network.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use clawft_kernel::boot::Kernel;
use clawft_kernel::embedding::{EmbeddingError, EmbeddingProvider};
use clawft_platform::NativePlatform;
use clawft_service_llm::{
    ChatRequest, ChatResponse, LlmBackend, LlmClient, LlmConfig, LlmError, SharedLlmClient,
    share_llm_client,
};
use clawft_types::config::{ChainConfig, Config, KernelConfig};
use clawft_weave::parent_link::{Backoff, ParentLink};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{RwLock, Semaphore, watch};

pub type KernelRef = Arc<RwLock<Kernel<NativePlatform>>>;

/// `scope_gate`, the manifest dir and the stub installs are process-global:
/// one test at a time.
pub static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub const PROMPT: &str = "the-secret-prompt-text-xyzzy";

/// Embeds of a text starting `block` wait on this gate (add permits to free).
pub fn gate() -> &'static Semaphore {
    static G: OnceLock<Semaphore> = OnceLock::new();
    G.get_or_init(|| Semaphore::new(0))
}

pub struct StubEmbedder;

#[async_trait]
impl EmbeddingProvider for StubEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        if text.starts_with("block") {
            gate().acquire().await.unwrap().forget();
        }
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
pub struct StubLlm {
    /// Every message text the model was sent.
    pub seen: Mutex<Vec<String>>,
    /// The `model` of every request.
    pub models: Mutex<Vec<String>>,
}

#[async_trait]
impl LlmBackend for StubLlm {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let texts: Vec<String> = request.messages.iter().map(|m| m.content.as_text().into_owned()).collect();
        self.models.lock().unwrap().push(request.model.clone());
        if texts.iter().any(|t| t == "fail-now") {
            return Err(LlmError::ClientError { status: 418, body: "leaked-upstream-body-secret".into() });
        }
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

/// The installed stub LLM and the client the daemon serves it through.
pub fn stub_llm() -> &'static (Arc<StubLlm>, SharedLlmClient) {
    static S: OnceLock<(Arc<StubLlm>, SharedLlmClient)> = OnceLock::new();
    S.get_or_init(|| {
        let stub = Arc::new(StubLlm::default());
        let shared = share_llm_client(LlmClient::with_backend(LlmConfig::default(), stub.clone()).unwrap());
        clawft_weave::shared_state::install_llm(shared.clone());
        (stub, shared)
    })
}

pub struct Daemon {
    _tmp: tempfile::TempDir,
    pub sock: PathBuf,
    pub manifests: PathBuf,
    pub kernel: KernelRef,
    pub shutdown: watch::Sender<bool>,
}

pub async fn spawn() -> Daemon {
    stub_llm();
    clawft_weave::shared_state::install_embedder(Arc::new(StubEmbedder));
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

pub async fn rpc(sock: &Path, method: &str, params: Value, auth: Option<&str>, project: Option<&str>) -> Value {
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
pub async fn register(d: &Daemon, shared: &str) -> String {
    let root = tempfile::tempdir().unwrap().keep();
    let r = rpc(&d.sock, "project.register", json!({"root": root, "name": "p"}), Some("admin"), None).await;
    assert_eq!(r["ok"], true, "{r}");
    let id = r["result"]["project"]["id"].as_str().unwrap().to_owned();
    if !shared.is_empty() {
        edit_shared(d, &id, shared);
    }
    id
}

/// Replace/append the `[shared]` table of a registered project's manifest.
pub fn edit_shared(d: &Daemon, id: &str, shared: &str) {
    let path = d.manifests.join(format!("{id}.toml"));
    let text = std::fs::read_to_string(&path).unwrap();
    let base = text.split("\n[shared]").next().unwrap().to_owned();
    std::fs::write(path, format!("{base}\n[shared]\n{shared}\n")).unwrap();
}

/// A project-scoped token, as the supervisor will hand a child in spawn.json.
pub async fn token_for(d: &Daemon, project: &str) -> String {
    let r = rpc(&d.sock, "auth.token.issue", json!({"label": "child", "project": project}), Some("admin"), None).await;
    assert_eq!(r["ok"], true, "{r}");
    r["result"]["secret"].as_str().unwrap().to_owned()
}

pub fn child(d: &Daemon, project: &str, token: &str) -> Arc<ParentLink> {
    Arc::new(
        ParentLink::new(d.sock.clone(), project.into(), token.into())
            .with_backoff(Backoff { attempts: 1, initial: Duration::from_millis(1), max: Duration::from_millis(1) }),
    )
}

pub fn shared_use_events(d: &Daemon, rt: &tokio::runtime::Handle) -> Vec<Value> {
    let k = rt.block_on(d.kernel.read());
    k.chain_manager()
        .unwrap()
        .tail(500)
        .into_iter()
        .filter(|e| e.kind == "shared.use")
        .map(|e| e.payload.unwrap())
        .collect()
}
