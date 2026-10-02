//! Package F, project-profile side (ADR-103 Phase 2 F).
//!
//! A kernel booted from a `profile = "project"` config, with provider keys in
//! the environment and the config and no reachable user daemon. Its own
//! process, because the profile installs a process-wide parent link.
//!
//! What this proves: the services snapshot has no embedding, voice,
//! talk_loop or mesh service; no provider key is readable from the
//! environment, the config dump or `kernel.status`; the LLM endpoint carries
//! no key and no URL; `shared_services` reports `down` and the call fails
//! closed. What it does not run: `daemon::run` end to end (the embedder and
//! `DAEMON_LLM` construction sites are exercised by their pure helpers and by
//! `parent_link` unit tests).

use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use clawft_types::config::{
    AgentAnchorConfig, ChainConfig, Config, KernelConfig, KernelProfile, MeshConfig,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{RwLock, watch};

const FAKE_KEYS: &[(&str, &str)] = &[
    ("OPENAI_API_KEY", "sk-fake-openai-0001"),
    ("ANTHROPIC_API_KEY", "sk-ant-fake-0002"),
    ("OPENROUTER_API_KEY", "sk-or-fake-0003"),
    ("GROQ_API_KEY", "gsk-fake-0004"),
    ("GITHUB_TOKEN", "ghp-fake-0005"),
    ("my_custom_secret", "fake-0006"),
    ("AWS_SESSION_THING", "fake-0007"),
];

#[tokio::test(flavor = "multi_thread")]
async fn project_profile_boots_with_no_heavy_services_and_no_provider_keys() {
    // The parent's environment as it would be inherited.
    let run = tempfile::tempdir().unwrap();
    // SAFETY: first statement of the only test in this binary; nothing else
    // reads the environment concurrently.
    unsafe {
        for (k, v) in FAKE_KEYS {
            std::env::set_var(k, v);
        }
        // No spawn.json: the link is unconfigured and every shared call fails closed.
        std::env::set_var("WEFTOS_RUNTIME_DIR", run.path());
    }

    // Decoy endpoints for everything a local fallback could dial: the LLM
    // service, local STT and TTS. Any connection to them fails the test.
    let decoys: Vec<tokio::net::TcpListener> = {
        let mut v = Vec::new();
        for var in ["LLM_SERVICE_URL", "WEFT_WHISPER_URL", "WEFT_TTS_URL", "WHISPER_SERVICE_URL"] {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            // SAFETY: as above, before anything else runs.
            unsafe { std::env::set_var(var, format!("http://{}", l.local_addr().unwrap())) };
            v.push(l);
        }
        v
    };

    let mut config = Config::default();
    config.providers.openai.api_key = serde_json::from_str("\"sk-fake-openai-0001\"").unwrap();
    config.providers.anthropic.api_key = serde_json::from_str("\"sk-ant-fake-0002\"").unwrap();
    config.voice.enabled = true;
    config.voice.consumer.enabled = true;
    let mut kc = KernelConfig {
        profile: Some(KernelProfile::Project),
        chain: Some(ChainConfig::isolated_in(&tempfile::tempdir().unwrap().keep())),
        mesh: Some(MeshConfig { enabled: true, ..MeshConfig::default() }),
        agent: Some(AgentAnchorConfig { talk_loop: true, voice_loop: true, ..AgentAnchorConfig::default() }),
        vector: Some(Default::default()),
        ..KernelConfig::default()
    };
    config.kernel = kc.clone();

    clawft_weave::project_hooks::adjust_services(&mut config, &mut kc);
    assert!(clawft_weave::project_profile::is_project_profile());

    // Environment: every fake key is gone.
    for (k, _) in FAKE_KEYS {
        assert!(std::env::var(k).is_err(), "{k} still readable in the child's environment");
    }
    let env_dump = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\n");
    // Config: nothing carries a key; the vector store is kept.
    let cfg_dump = serde_json::to_string(&config).unwrap() + &serde_json::to_string(&kc).unwrap();
    for (_, v) in FAKE_KEYS {
        assert!(!env_dump.contains(v), "{v} in the environment");
        assert!(!cfg_dump.contains(v), "{v} in the config dump");
    }
    assert!(kc.vector.is_some());

    // The LLM endpoint is the parent: no URL, no key, no OpenRouter takeover
    // (the fake OPENROUTER_API_KEY was set when this process started).
    let ep = clawft_weave::llm_service::resolve_llm_endpoint(None);
    assert_eq!(ep.config.base_url, "parent://shared.llm");
    assert!(ep.config.api_key.is_none() && !ep.using_openrouter);
    let (client, _) = clawft_weave::llm_service::build_llm_client(None).unwrap();
    let err = client
        .complete(vec![clawft_service_llm::ChatMessage::user("hi")], None, None)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("parent_unavailable"), "no local fallback: {err}");

    // A project profile only boots on a real child root (package E checks
    // its certificate, signed parent policy and pins). Build one with the
    // kernel's test builder; nothing in the overlay's checks is bypassed.
    clawft_kernel::overlay_runtime::test_support::install_child_fixture(run.path());

    // Boot the kernel from the adjusted kernel config and look at it over RPC.
    // The fixture's project key (seed [3; 32]) is the node key, as `pre_boot` hands it over.
    let kernel = Kernel::boot_with_node_key(config, kc, Arc::new(NativePlatform::new()), Some([3u8; 32]))
        .await
        .expect("boot");
    let kernel = Arc::new(RwLock::new(kernel));
    let sock = run.path().join("kernel.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let (tx, _rx) = watch::channel(false);
    let (k, t) = (Arc::clone(&kernel), tx.clone());
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            tokio::spawn(clawft_weave::daemon::handle_connection(s, Arc::clone(&k), t.clone()));
        }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;

    let services = rpc(&sock, "kernel.services", Value::Null, "admin").await;
    assert_eq!(services["ok"], true, "{services}");
    let listing = services["result"].to_string().to_lowercase();
    for banned in ["embedding", "voice", "talk_loop", "talkmode", "mesh"] {
        assert!(!listing.contains(banned), "{banned} service listed in a project kernel: {listing}");
    }

    let status = rpc(&sock, "kernel.status", Value::Null, "admin").await;
    assert_eq!(status["ok"], true, "{status}");
    assert_eq!(
        status["result"]["shared_services"],
        json!({"embeddings": "down", "llm": "down", "voice": "down"}),
        "no parent reachable: every shared service reports down"
    );
    let status_dump = status.to_string();
    for (_, v) in FAKE_KEYS {
        assert!(!status_dump.contains(v), "{v} in kernel.status");
    }

    // No voice tools: their default STT/TTS endpoints belong to the parent.
    let reg = clawft_weave::project_profile::strip_voice_tools(tools(&[
        "audio_transcribe", "audio_synthesize", "voice_listen", "voice_speak", "read_file",
    ]));
    assert_eq!(reg.list(), ["read_file"]);

    // Parent down, and the shared embedder has no width: not ready, no guess.
    let emb = clawft_weave::project_profile::project_embedder().await.expect("project embedder");
    assert!(!emb.is_ready());
    assert_eq!(clawft_core::embeddings::Embedder::dimension(&*emb), 0);
    assert!(clawft_core::embeddings::Embedder::embed(&*emb, "x").await.is_err());

    // Through all of the above, nothing dialled a local endpoint.
    for l in &decoys {
        let hit = tokio::time::timeout(Duration::from_millis(150), l.accept()).await;
        assert!(hit.is_err(), "a project kernel with its parent down made an outbound connection");
    }

    // A project kernel does not serve shared.* to a sibling.
    let r = rpc(&sock, "shared.embed", json!({"texts": [], "project_id": "01J0000000000000000000000A"}), "admin").await;
    assert_eq!(r["error_kind"], "not_a_parent", "{r}");
}

async fn rpc(sock: &std::path::Path, method: &str, params: Value, auth: &str) -> Value {
    let (r, mut w) = UnixStream::connect(sock).await.unwrap().into_split();
    let req = json!({"id": "t", "proto": 1, "method": method, "params": params, "auth": auth});
    w.write_all(format!("{req}\n").as_bytes()).await.unwrap();
    let mut line = String::new();
    BufReader::new(r).read_line(&mut line).await.unwrap();
    serde_json::from_str(line.trim()).unwrap()
}

struct Named(&'static str);

#[async_trait::async_trait]
impl clawft_core::tools::registry::Tool for Named {
    fn name(&self) -> &str {
        self.0
    }
    fn description(&self) -> &str {
        "test"
    }
    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }
    async fn execute(&self, _args: Value) -> Result<Value, clawft_core::tools::registry::ToolError> {
        Ok(Value::Null)
    }
}

fn tools(names: &[&'static str]) -> clawft_core::tools::registry::ToolRegistry {
    let mut r = clawft_core::tools::registry::ToolRegistry::new();
    for n in names {
        r.register(Arc::new(Named(n)));
    }
    r
}
