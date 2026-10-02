//! What `kernel.profile = "project"` changes (ADR-103 Phase 2 F).
//!
//! [`adjust_services`] is the body of `project_hooks::adjust_services`. For a
//! `project`-profile kernel it:
//!
//! * turns the mesh listener off (a project `weave.toml` mesh section is
//!   ignored with a warning; the machine mesh service speaks for the
//!   project, Phase 3);
//! * turns the voice consumer, `talk_loop` and `voice_loop` off (voice runs
//!   in the user daemon);
//! * clears every provider API key from the config and, best effort,
//!   scrubs credential-looking variables from this process's environment
//!   ([`scrub_provider_secrets_from_env`]). This is NOT a guarantee:
//!   `remove_var` does not change the initial environment block that
//!   `/proc/<pid>/environ` and `ps -E` read, and the project kernel has a
//!   shell tool. The real guard is the supervisor (package G) spawning the
//!   child with a cleared environment, so the keys are never in it; use
//!   [`crate::env_probe`] to test that. The scrub also narrows what in-process
//!   code can `getenv`;
//! * installs the process-wide [`ParentLink`], read from `spawn.json`, which
//!   the embedder and LLM construction sites in `daemon.rs` and
//!   `llm_service.rs` consult to build the remote variants instead of loading
//!   a model or reading a key.
//!
//! The `ecc` vector store is untouched: it is the project's memory. Only the
//! embedding *model* is shared.

use std::sync::{Arc, OnceLock};

use clawft_service_llm::{LlmBackend, LlmConfig};
use clawft_types::config::{
    Config, KernelConfig, KernelProfile, LlmEndpointConfig, ProviderConfig, ProvidersConfig,
};
use clawft_types::runtime_paths::RuntimePaths;
use tracing::{info, warn};

use crate::llm_service::ResolvedLlmEndpoint;
use crate::parent_link::{ParentLink, ParentLlmBackend, RemoteEmbedder};
use crate::protocol::SharedServicesHealth;

/// Set once by [`adjust_services`] for a project-profile process. Its
/// presence IS "this process is a project kernel": the construction sites
/// ask [`parent_link`] and never look at config again.
static LINK: OnceLock<Arc<ParentLink>> = OnceLock::new();

static EMBEDDER: tokio::sync::OnceCell<Arc<RemoteEmbedder>> = tokio::sync::OnceCell::const_new();

/// The parent link, when this process is a project kernel.
pub fn parent_link() -> Option<Arc<ParentLink>> {
    LINK.get().cloned()
}

/// True in a project-profile process.
pub fn is_project_profile() -> bool {
    LINK.get().is_some()
}

/// `shared_services` for `kernel.status`; `None` outside the project profile.
pub fn shared_services_health() -> Option<SharedServicesHealth> {
    LINK.get().map(|l| l.health())
}

/// Environment variables that carry provider credentials: these known names
/// plus the shapes [`is_provider_secret_env`] matches. The supervisor
/// (package G) must keep all of these out of a child's environment (clear the
/// env and pass an allow-list); the child's own scrub is a second layer only.
pub const PROVIDER_SECRET_ENV: &[&str] = &[
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENROUTER_API_KEY",
    "GROQ_API_KEY",
    "DEEPSEEK_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "XAI_API_KEY",
    "ZHIPU_API_KEY",
    "DASHSCOPE_API_KEY",
    "MOONSHOT_API_KEY",
    "MINIMAX_API_KEY",
    "AIHUBMIX_API_KEY",
    "ELEVENLABS_API_KEY",
    "BRAVE_API_KEY",
    "HF_TOKEN",
    "HUGGING_FACE_HUB_TOKEN",
];

/// Whether `name` looks like a credential variable: a known name, or (case
/// insensitively) ending `_KEY`, `_TOKEN`, `_SECRET`, `_PASSWORD`, or starting
/// `AWS_`.
pub fn is_provider_secret_env(name: &str) -> bool {
    let up = name.to_ascii_uppercase();
    PROVIDER_SECRET_ENV.contains(&up.as_str())
        || ["_KEY", "_TOKEN", "_SECRET", "_PASSWORD"].iter().any(|s| up.ends_with(s))
        || up.starts_with("AWS_")
}

/// Remove credential variables from this process's environment: everything
/// [`is_provider_secret_env`] matches plus each name in `extra` (the
/// `api_key_env` names the config points at). Returns the names removed,
/// never the values.
///
/// Best effort only; see the module docs. It runs after the tokio runtime and
/// its worker threads exist (`main` is `#[tokio::main]`, and the profile is
/// known only once the config is loaded), so another thread could be reading
/// the environment while it runs, which is why `remove_var` is `unsafe`.
/// That race is accepted because this is not the guard that matters.
pub fn scrub_provider_secrets_from_env(extra: &[String]) -> Vec<String> {
    let names: Vec<String> = std::env::vars_os()
        .filter_map(|(k, _)| k.into_string().ok())
        .filter(|k| is_provider_secret_env(k) || extra.iter().any(|e| e.eq_ignore_ascii_case(k)))
        .collect();
    for name in &names {
        // SAFETY: see the function docs: worker threads exist, and the race
        // with a concurrent `getenv` is accepted for a best-effort scrub. The
        // values are discarded, not read.
        unsafe { std::env::remove_var(name) };
    }
    names
}

/// The project registry's tool set minus the voice tools: `audio_transcribe`
/// / `audio_synthesize` / `voice_listen` / `voice_speak` default to local STT
/// and TTS endpoints (and cloud fallbacks); voice runs in the user daemon.
/// Outside the project profile the registry is returned unchanged.
pub fn strip_voice_tools(
    registry: clawft_core::tools::registry::ToolRegistry,
) -> clawft_core::tools::registry::ToolRegistry {
    if !is_project_profile() {
        return registry;
    }
    let deny: Vec<String> = ["audio_transcribe", "audio_synthesize", "voice_listen", "voice_speak"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    registry.filtered_tools(&[], &deny)
}

fn clear_keys(p: &mut ProvidersConfig) {
    for pc in [
        &mut p.custom,
        &mut p.anthropic,
        &mut p.openai,
        &mut p.openrouter,
        &mut p.deepseek,
        &mut p.groq,
        &mut p.zhipu,
        &mut p.dashscope,
        &mut p.vllm,
        &mut p.gemini,
        &mut p.moonshot,
        &mut p.minimax,
        &mut p.aihubmix,
        &mut p.openai_codex,
        &mut p.xai,
        &mut p.elevenlabs,
        &mut p.local,
        &mut p.ollama,
    ] {
        pc.api_key = ProviderConfig::default().api_key;
    }
}

fn is_project(config: &Config, kernel_config: &KernelConfig) -> bool {
    kernel_config.profile == Some(KernelProfile::Project)
        || config.kernel.profile == Some(KernelProfile::Project)
}

/// The config half of [`adjust_services`], pure so it can be tested. Returns
/// `false` and changes nothing outside the project profile; otherwise returns
/// `true`, with a warning logged for each ignored setting.
pub fn apply_to_config(config: &mut Config, kernel_config: &mut KernelConfig) -> bool {
    if !is_project(config, kernel_config) {
        return false;
    }
    for kc in [&mut *kernel_config, &mut config.kernel] {
        kc.profile = Some(KernelProfile::Project);
        if let Some(mesh) = kc.mesh.as_mut() {
            if mesh.enabled {
                warn!(
                    "project profile: the [kernel.mesh] section is ignored; a project \
                     kernel opens no mesh listener"
                );
            }
            mesh.enabled = false;
        }
        if let Some(agent) = kc.agent.as_mut() {
            if agent.talk_loop || agent.voice_loop {
                warn!("project profile: talk_loop and voice_loop run in the user daemon; turned off");
            }
            agent.talk_loop = false;
            agent.voice_loop = false;
        }
    }
    config.voice.enabled = false;
    config.voice.consumer.enabled = false;
    clear_keys(&mut config.providers);
    config.tools.web.search.api_key = Default::default();
    true
}

/// Body of `project_hooks::adjust_services`. A no-op unless the profile is
/// `project`.
pub fn adjust_services(config: &mut Config, kernel_config: &mut KernelConfig) {
    if !apply_to_config(config, kernel_config) {
        return;
    }
    let extra = vec![config.voice.xai.api_key_env_name().to_owned()];
    let scrubbed = scrub_provider_secrets_from_env(&extra);
    if !scrubbed.is_empty() {
        info!(count = scrubbed.len(), "project profile: credential variables removed from this process's environment (best effort; the supervisor must spawn the child with a cleared env)");
    }
    let spawn_json = RuntimePaths::resolve().spawn_json();
    let link = Arc::new(ParentLink::from_spawn_json(&spawn_json));
    if let Some(why) = link_problem(&link) {
        warn!(
            problem = %why,
            "project profile: no usable link to the user daemon; shared embeddings and LLM \
             fail closed (parent_unavailable) until spawn.json is fixed and the kernel restarted"
        );
    }
    link.spawn_monitor();
    if LINK.set(link).is_err() {
        warn!("project profile: parent link already installed; keeping the first");
    }
}

fn link_problem(link: &ParentLink) -> Option<String> {
    link.project_id().is_none().then(|| "spawn.json missing or incomplete".to_owned())
}

/// The embedder for the context router (`daemon.rs`): the remote embedder in
/// a project kernel, `None` elsewhere (the caller keeps its own choice).
pub async fn project_embedder() -> Option<Arc<RemoteEmbedder>> {
    let link = parent_link()?;
    Some(
        EMBEDDER
            .get_or_init(|| RemoteEmbedder::connect(link))
            .await
            .clone(),
    )
}

/// The embedding provider for the L2 session tier (`daemon.rs`): remote in a
/// project kernel, `None` elsewhere. The local Qwen3/e5/ONNX/Mock selection is
/// never run in a project kernel.
pub async fn project_embedding_provider()
-> Option<Arc<dyn clawft_kernel::embedding::EmbeddingProvider>> {
    project_embedder()
        .await
        .map(|e| e as Arc<dyn clawft_kernel::embedding::EmbeddingProvider>)
}

/// LLM endpoint for a project kernel: no URL, no key, no OpenRouter
/// takeover; only the model name from `[kernel.llm]` is honoured.
pub fn parent_llm_endpoint(cfg_llm: Option<&LlmEndpointConfig>) -> ResolvedLlmEndpoint {
    let model = cfg_llm
        .and_then(|c| c.model.clone())
        .filter(|m| !m.is_empty());
    ResolvedLlmEndpoint {
        config: LlmConfig {
            base_url: "parent://shared.llm".to_owned(),
            model: model
                .clone()
                .unwrap_or_else(|| clawft_service_llm::DEFAULT_LLM_MODEL.to_owned()),
            api_key: None,
            referer: None,
            app_title: None,
            ..LlmConfig::default()
        },
        using_openrouter: false,
        url_source: "parent",
        model_source: if model.is_some() { "config:[kernel.llm].model" } else { "default" },
    }
}

/// The backend `llm_service::new_client` attaches in a project kernel.
pub fn parent_llm_backend() -> Option<Arc<dyn LlmBackend>> {
    parent_link().map(|l| Arc::new(ParentLlmBackend::new(l)) as Arc<dyn LlmBackend>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_types::config::{AgentAnchorConfig, MeshConfig};

    fn project_cfg() -> (Config, KernelConfig) {
        let mut config = Config::default();
        let mut kc = KernelConfig::default();
        kc.profile = Some(KernelProfile::Project);
        kc.mesh = Some(MeshConfig {
            enabled: true,
            ..MeshConfig::default()
        });
        kc.agent = Some(AgentAnchorConfig {
            talk_loop: true,
            voice_loop: true,
            ..AgentAnchorConfig::default()
        });
        config.kernel = kc.clone();
        config.voice.enabled = true;
        config.voice.consumer.enabled = true;
        (config, kc)
    }

    #[test]
    fn project_profile_turns_off_mesh_voice_and_keys() {
        let (mut config, mut kc) = project_cfg();
        config.providers.openai.api_key = serde_json::from_str("\"sk-fake-openai\"").unwrap();
        config.tools.web.search.api_key = serde_json::from_str("\"brave-fake\"").unwrap();
        kc.vector = Some(Default::default());
        assert!(apply_to_config(&mut config, &mut kc));

        for k in [&kc, &config.kernel] {
            assert!(!k.mesh.as_ref().unwrap().enabled);
            let a = k.agent.as_ref().unwrap();
            assert!(!a.talk_loop && !a.voice_loop);
        }
        assert!(!config.voice.enabled && !config.voice.consumer.enabled);
        let dump = serde_json::to_string(&config).unwrap();
        assert!(!dump.contains("sk-fake-openai"), "provider key survived: {dump}");
        assert!(!dump.contains("brave-fake"));
        assert!(kc.vector.is_some(), "the ecc vector store is the project's memory");
    }

    #[test]
    fn other_profiles_are_untouched() {
        let mut config = Config::default();
        let mut kc = KernelConfig::default();
        kc.mesh = Some(MeshConfig {
            enabled: true,
            ..MeshConfig::default()
        });
        config.voice.consumer.enabled = true;
        let before = serde_json::to_value(&config).unwrap();
        assert!(!apply_to_config(&mut config, &mut kc));
        assert_eq!(serde_json::to_value(&config).unwrap(), before);
        assert!(kc.mesh.as_ref().unwrap().enabled);
    }

    #[test]
    fn provider_secret_names() {
        for n in [
            "OPENAI_API_KEY", "ANTHROPIC_API_KEY", "SOMETHING_NEW_API_KEY", "HF_TOKEN",
            "openai_api_key", "GITHUB_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "MY_SERVICE_SECRET",
            "AWS_ACCESS_KEY_ID", "AWS_PROFILE", "DB_PASSWORD", "STRIPE_KEY",
        ] {
            assert!(is_provider_secret_env(n), "{n}");
        }
        for n in ["PATH", "HOME", "WEFTOS_RUNTIME_DIR", "LLM_MODEL", "KEYBOARD", "TOKENIZERS_PARALLELISM"] {
            assert!(!is_provider_secret_env(n), "{n}");
        }
    }

    #[test]
    fn parent_endpoint_has_no_url_and_no_key() {
        let r = parent_llm_endpoint(Some(&LlmEndpointConfig {
            service_url: Some("http://elsewhere:1".into()),
            model: Some("m".into()),
        }));
        assert_eq!(r.config.base_url, "parent://shared.llm");
        assert_eq!(r.config.model, "m");
        assert!(r.config.api_key.is_none() && !r.using_openrouter);
    }
}
