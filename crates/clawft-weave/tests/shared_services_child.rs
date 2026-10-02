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

    // Boot the kernel from the adjusted kernel config and look at it over RPC.
    let kernel = Kernel::boot(config, kc, Arc::new(NativePlatform::new())).await.expect("boot");
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

    // A project kernel does not serve shared.* to a sibling.
    let r = rpc(&sock, "shared.embed", json!({"texts": [], "project_id": "01J0000000000000000000000A"}), "admin").await;
    assert_eq!(r["error_kind"], "not_a_parent", "{r}");
}

async fn rpc(sock: &std::path::Path, method: &str, params: Value, auth: &str) -> Value {
    let (r, mut w) = UnixStream::connect(sock).await.unwrap().into_split();
    let req = json!({"id": "t", "method": method, "params": params, "auth": auth});
    w.write_all(format!("{req}\n").as_bytes()).await.unwrap();
    let mut line = String::new();
    BufReader::new(r).read_line(&mut line).await.unwrap();
    serde_json::from_str(line.trim()).unwrap()
}
