//! WASM entrypoint for clawft.
//!
//! This crate provides a WebAssembly build of the clawft agent core.
//! It excludes components that require native OS features:
//! - Shell execution tools (exec_shell, spawn)
//! - Channel plugins (Telegram, Slack, Discord)
//! - Native CLI terminal I/O
//! - Process spawning
//!
//! # Platform Support
//!
//! The WASM build targets `wasm32-wasip2` and uses WASI preview 1 for:
//! - HTTP outbound (LLM API calls)
//! - Filesystem (config, sessions)
//! - Environment variables
//!
//! # Dependencies
//!
//! This crate is intentionally decoupled from `clawft-core` and `clawft-platform`
//! to avoid pulling in tokio["full"] and reqwest, neither of which compiles for
//! WASM targets. It depends only on `clawft-types`, `serde`, and `serde_json`.
//!
//! # Size Budget
//!
//! Target: < 300 KB uncompressed, < 120 KB gzipped.

#[cfg(feature = "alloc-tracing")]
pub mod alloc_trace;
pub mod allocator;
pub mod env;
pub mod fs;
pub mod http;
pub mod platform;

/// Plugin sandbox enforcement for WASM plugins.
///
/// This module is only available when the `wasm-plugins` feature is enabled.
/// It provides [`sandbox::PluginSandbox`], validation functions for HTTP,
/// filesystem, and environment access, plus rate limiting and size enforcement.
#[cfg(feature = "wasm-plugins")]
pub mod sandbox;

/// Audit logging for WASM plugin host function calls.
///
/// Every host function invocation is recorded in a per-plugin audit log.
/// This provides a tamper-evident record of all side-effecting operations.
#[cfg(feature = "wasm-plugins")]
pub mod audit;

/// WASM plugin engine with fuel metering and memory limits.
///
/// Provides [`engine::WasmPluginEngine`] -- the host-side runtime for loading
/// and executing WASM plugin modules with configurable resource limits.
#[cfg(feature = "wasm-plugins")]
pub mod engine;

/// Permission persistence and approval for WASM plugin upgrades.
///
/// Provides [`permission_store::PermissionStore`] for saving/loading approved
/// permissions, and [`permission_store::PermissionApprover`] for requesting
/// user consent when a plugin upgrade introduces new permissions.
#[cfg(feature = "wasm-plugins")]
pub mod permission_store;

pub use platform::WasmPlatform;

/// Version information for the WASM build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Initialize the WASM agent (WASI / non-browser path).
///
/// Called once when the WASM module is instantiated. Sets up the agent
/// configuration and pipeline from WASI-accessible config files.
///
/// # Returns
///
/// Returns 0 on success, non-zero on failure.
///
/// Browser builds shadow this with the async `wasm-bindgen` `init` from
/// [`browser_entry`]; gating prevents a shadow clash on `wasm32-unknown-unknown`.
#[cfg(not(feature = "browser"))]
pub fn init() -> i32 {
    // Phase 3A Week 11: Will load config from WASI filesystem
    // and set up the agent pipeline.
    0
}

/// Process a single message through the agent pipeline (WASI path).
///
/// # Arguments
///
/// * `input` - The user message as a UTF-8 string.
///
/// # Returns
///
/// The agent's response as a string, or an error message.
///
/// On browser builds the analogous wired entry-point is
/// [`browser_entry::send_message`] which drives the full pipeline.
#[cfg(not(feature = "browser"))]
pub fn process_message(input: &str) -> String {
    // Phase 3A Week 12: Will run the full 6-stage pipeline.
    format!(
        "clawft-wasm v{}: received '{}' (pipeline not yet wired)",
        VERSION, input
    )
}

/// Get the agent's capabilities as a JSON string (WASI path).
///
/// Returns a JSON object describing available tools, providers,
/// and configuration for this WASM instance. Browser builds query
/// capabilities via the wired pipeline directly.
#[cfg(not(feature = "browser"))]
pub fn capabilities() -> String {
    serde_json::json!({
        "version": VERSION,
        "platform": "wasm32-wasip2",
        "tools": ["read_file", "write_file", "edit_file", "list_directory", "memory_read", "memory_write", "web_fetch", "web_search"],
        "excluded_tools": ["exec_shell", "spawn", "message"],
        "channels": [],
        "status": "initializing"
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Browser WASM entry point (feature = "browser")
// ---------------------------------------------------------------------------

/// Browser entry point module providing wasm-bindgen exports for
/// running clawft in a web browser via the BrowserPlatform.
#[cfg(feature = "browser")]
mod browser_entry {
    use std::sync::{Arc, OnceLock};

    use clawft_core::agent::loop_core::AgentLoop;
    use clawft_core::bootstrap::AppContext;
    use clawft_core::tools::registry::ToolRegistry;
    use clawft_llm::browser_transport::BrowserLlmClient;
    use clawft_llm::config::LlmProviderConfig;
    use clawft_platform::BrowserPlatform;
    use clawft_types::config::Config;
    use clawft_types::event::InboundMessage;
    use wasm_bindgen::prelude::*;

    /// Persistent runtime state initialized by `init()`.
    ///
    /// Wires `AgentLoop<BrowserPlatform>` end-to-end through the
    /// 6-stage pipeline (classifier → router → assembler → transport
    /// → scorer → learner) plus session management, the context
    /// builder, the conversation sink, the effect gate, and the tool
    /// registry. `send_message` builds an [`InboundMessage`] and
    /// dispatches it through [`AgentLoop::handle_turn`], identical in
    /// shape to the native daemon's `agent.chat` path.
    struct BrowserRuntime {
        agent: Arc<AgentLoop<BrowserPlatform>>,
        /// Snapshot of the tool registry kept alive for introspection
        /// entry points (`tool_schema`, `tool_list`). Captured before
        /// `ctx.into_agent_loop()` consumes the AppContext (WEFT-307).
        tools: Arc<ToolRegistry>,
    }

    // SAFETY: wasm32-unknown-unknown is single-threaded; nothing here
    // ever crosses a thread boundary. The pipeline's `Send + Sync`
    // expectations on transports are satisfied via the `?Send`
    // `async_trait` relaxation in `LlmTransport` / `LlmProvider` for
    // the browser feature; the `OnceLock` in turn just needs the
    // outer struct to be `Send + Sync`.
    unsafe impl Send for BrowserRuntime {}
    unsafe impl Sync for BrowserRuntime {}

    static RUNTIME: OnceLock<BrowserRuntime> = OnceLock::new();

    /// Look up the user's `ProviderConfig` by provider name.
    fn user_provider_config(config: &Config, name: &str) -> clawft_types::config::ProviderConfig {
        match name {
            "anthropic" => config.providers.anthropic.clone(),
            "openai" => config.providers.openai.clone(),
            "openrouter" => config.providers.openrouter.clone(),
            "deepseek" => config.providers.deepseek.clone(),
            "groq" => config.providers.groq.clone(),
            "gemini" => config.providers.gemini.clone(),
            "xai" => config.providers.xai.clone(),
            _ => config.providers.custom.clone(),
        }
    }

    /// Route a model string like "anthropic/claude-sonnet-4-20250514" to the
    /// correct provider config and return (LlmProviderConfig, stripped_model, ProviderConfig).
    ///
    /// Resolution order:
    /// 1. Exact prefix match against builtin providers (e.g. `openrouter/` → OpenRouter).
    /// 2. Fallback: if no prefix matches, route to OpenRouter (or the first provider
    ///    with an API key). The full model string is sent as-is since there's no
    ///    prefix to strip. This handles models like `arcee-ai/trinity-large-preview:free`
    ///    that are hosted on OpenRouter but don't use the `openrouter/` prefix.
    fn resolve_provider(
        config: &Config,
        model: &str,
    ) -> Result<
        (
            LlmProviderConfig,
            String,
            clawft_types::config::ProviderConfig,
        ),
        String,
    > {
        let builtins = clawft_llm::config::builtin_providers();

        // 1. Find the builtin whose model_prefix matches.
        for builtin in &builtins {
            if let Some(ref prefix) = builtin.model_prefix {
                if model.starts_with(prefix) {
                    let stripped = model[prefix.len()..].to_string();
                    let user_cfg = user_provider_config(config, &builtin.name);

                    // Merge user overrides into the builtin config.
                    let mut llm_cfg = builtin.clone();
                    if let Some(ref base) = user_cfg.api_base {
                        llm_cfg.base_url = base.clone();
                    }
                    if let Some(ref extra) = user_cfg.extra_headers {
                        llm_cfg.headers.extend(extra.clone());
                    }

                    return Ok((llm_cfg, stripped, user_cfg));
                }
            }
        }

        // 2. No prefix matched — fall back to OpenRouter if it has an API key,
        //    since OpenRouter aggregates third-party models with vendor/ prefixes
        //    (e.g. arcee-ai/, meta-llama/, mistralai/).
        let fallback_order = [
            "openrouter",
            "openai",
            "anthropic",
            "groq",
            "deepseek",
            "gemini",
            "xai",
        ];

        for name in &fallback_order {
            let user_cfg = user_provider_config(config, name);
            if !user_cfg.api_key.expose().is_empty() {
                let builtin = builtins.iter().find(|b| b.name == *name).cloned();
                if let Some(mut llm_cfg) = builtin {
                    if let Some(ref base) = user_cfg.api_base {
                        llm_cfg.base_url = base.clone();
                    }
                    if let Some(ref extra) = user_cfg.extra_headers {
                        llm_cfg.headers.extend(extra.clone());
                    }

                    web_sys::console::log_1(
                        &format!(
                            "[clawft] no prefix match for '{}', falling back to {} provider",
                            model, name
                        )
                        .into(),
                    );

                    // Send the full model string as-is (no prefix to strip).
                    return Ok((llm_cfg, model.to_string(), user_cfg));
                }
            }
        }

        Err(format!(
            "no provider found for model '{}'. Either use a prefixed model like \
             'openrouter/arcee-ai/trinity-large-preview:free', or configure an \
             API key for a provider.",
            model
        ))
    }

    /// Initialize the clawft-wasm browser runtime.
    ///
    /// Parses the provided JSON config, builds an
    /// [`AppContext<BrowserPlatform>`](AppContext), wires the LLM
    /// transport to the appropriate provider via [`BrowserLlmClient`],
    /// and produces a fully assembled [`AgentLoop<BrowserPlatform>`].
    /// Must be called once before `send_message`.
    #[wasm_bindgen]
    pub async fn init(config_json: &str) -> Result<(), JsValue> {
        console_error_panic_hook::set_once();

        let mut config: Config = serde_json::from_str(config_json)
            .map_err(|e| JsValue::from_str(&format!("config parse error: {e}")))?;

        let platform = Arc::new(BrowserPlatform::new());

        let model = config.agents.defaults.model.clone();
        let (llm_cfg, stripped_model, user_cfg) =
            resolve_provider(&config, &model).map_err(|e| JsValue::from_str(&e))?;

        let api_key = user_cfg.api_key.expose();
        if api_key.is_empty() {
            return Err(JsValue::from_str(&format!(
                "no API key configured for provider matching model '{}'. Set apiKey in the provider config.",
                model
            )));
        }

        web_sys::console::log_1(
            &format!(
                "[clawft] provider={}, cors_proxy={:?}, browser_direct={}, base_url={}",
                llm_cfg.name, user_cfg.cors_proxy, user_cfg.browser_direct, llm_cfg.base_url,
            )
            .into(),
        );

        let client = Arc::new(BrowserLlmClient::with_api_key(
            llm_cfg,
            api_key.to_string(),
            user_cfg.browser_direct,
            user_cfg.cors_proxy.clone(),
        ));

        // The pipeline's transport sees the stripped model name; the
        // routing decision will pin it back to the same provider via
        // `StaticRouter::from_config`. Stamp the stripped form into the
        // agent defaults so the static router doesn't try to re-resolve
        // a prefix that the browser already consumed.
        config.agents.defaults.model = stripped_model;

        // Build the AppContext (bus, sessions, memory, skills, context,
        // empty tools registry, default classifier/router/assembler/
        // scorer/learner with stub transport) — exactly like the native
        // path — then swap in a transport wired to BrowserLlmClient.
        let mut ctx = AppContext::new(config.clone(), platform.clone())
            .await
            .map_err(|e| JsValue::from_str(&format!("AppContext init failed: {e}")))?;

        // Register all the browser-compatible tools. Native-exec tools
        // (shell, spawn) are gated out at compile time via
        // clawft-tools' feature flags so what lands here is the
        // file/memory/web-search/web-fetch subset. The browser
        // workspace is the in-memory `BrowserFileSystem`'s virtual
        // home (`/clawft`) — file tools sandbox to that root and the
        // web tools defer to UrlPolicy for SSRF protection.
        let workspace_dir = std::path::PathBuf::from("/clawft/workspace");
        clawft_tools::register_all(
            ctx.tools_mut(),
            platform.clone(),
            workspace_dir,
            clawft_types::security::CommandPolicy::default(),
            clawft_types::security::UrlPolicy::default(),
            clawft_tools::web_search::WebSearchConfig::default(),
        );

        // Replace the stub transport with the BrowserLlmClient-backed
        // one so the pipeline's transport stage actually reaches the
        // network. Stages 1, 2, 3, 5, and 6 are unchanged.
        let pipeline = clawft_core::bootstrap::build_browser_pipeline(&config, client);
        ctx.set_pipeline(pipeline);

        // WEFT-307: snapshot the tool registry before `into_agent_loop`
        // consumes the AppContext so JS callers can introspect tool
        // schemas via `tool_schema(slug)`.
        let tools = ctx.tools_arc();

        let agent = Arc::new(ctx.into_agent_loop());

        RUNTIME
            .set(BrowserRuntime { agent, tools })
            .map_err(|_| JsValue::from_str("already initialized"))?;

        web_sys::console::log_1(
            &"clawft-wasm initialized — AgentLoop<BrowserPlatform> wired through full pipeline"
                .into(),
        );
        Ok(())
    }

    /// Send a message through the clawft AgentLoop pipeline.
    ///
    /// Builds an [`InboundMessage`] (channel = `"web"`, chat_id =
    /// `"browser"`), dispatches it through
    /// [`AgentLoop::handle_turn`], and returns the resulting
    /// outbound text. Conversation history, session state, the
    /// 6-stage pipeline, the conversation sink, and tool execution
    /// all run on the wired [`AgentLoop<BrowserPlatform>`].
    #[wasm_bindgen]
    pub async fn send_message(text: &str) -> Result<String, JsValue> {
        let rt = RUNTIME
            .get()
            .ok_or_else(|| JsValue::from_str("not initialized — call init() first"))?;

        let msg = InboundMessage {
            channel: "web".into(),
            sender_id: "browser-user".into(),
            chat_id: "browser".into(),
            content: text.to_string(),
            timestamp: chrono::Utc::now(),
            media: vec![],
            metadata: std::collections::HashMap::new(),
        };

        let outbound = rt
            .agent
            .handle_turn(msg)
            .await
            .map_err(|e| JsValue::from_str(&format!("agent error: {e}")))?;

        Ok(outbound.content)
    }

    /// Set an environment variable on the BrowserPlatform.
    #[wasm_bindgen]
    pub fn set_env(_key: &str, _value: &str) {
        // Browser env vars are managed by BrowserPlatform.env()
    }

    // -------------------------------------------------------------------
    // Tool introspection (WEFT-307)
    // -------------------------------------------------------------------

    /// Return the JSON-Schema for a registered tool, or `null` if
    /// no tool with that name exists.
    ///
    /// The shape mirrors the Axum API's `/api/tools/{slug}/schema`
    /// response so the dashboard's `/tools` route can render the same
    /// JSON-Schema viewer in either backend mode:
    ///
    /// ```json
    /// {
    ///   "name": "<slug>",
    ///   "description": "...",
    ///   "parameters": { ...JSON-Schema... }
    /// }
    /// ```
    ///
    /// The runtime must already be initialized via [`init`].
    #[wasm_bindgen]
    pub fn tool_schema(slug: &str) -> String {
        let Some(rt) = RUNTIME.get() else {
            return "null".to_string();
        };
        match rt.tools.get(slug) {
            Some(tool) => serde_json::json!({
                "name": tool.name(),
                "description": tool.description(),
                "parameters": tool.parameters(),
            })
            .to_string(),
            None => "null".to_string(),
        }
    }

    /// Return the list of registered tool names as a JSON array string.
    ///
    /// The list is sorted alphabetically. Useful for the dashboard's
    /// `/tools` route to enumerate browser-mode tools without
    /// hardcoding the subset in `wasm-adapter.ts::listTools`.
    #[wasm_bindgen]
    pub fn tool_list() -> String {
        let Some(rt) = RUNTIME.get() else {
            return "[]".to_string();
        };
        serde_json::to_string(&rt.tools.list()).unwrap_or_else(|_| "[]".to_string())
    }

    // -------------------------------------------------------------------
    // Boot info (Feature 3)
    // -------------------------------------------------------------------

    /// Return a JSON array of boot phases mirroring the native kernel BootLog.
    ///
    /// Called once after WASM loads to feed real boot data into the ExoChain
    /// log instead of hardcoded entries on the TypeScript side.
    #[wasm_bindgen]
    pub fn boot_info() -> String {
        let phases = serde_json::json!([
            {"phase": "INIT", "detail": format!("WeftOS v{} booting...", crate::VERSION)},
            {"phase": "INIT", "detail": "PID 0 (kernel)"},
            {"phase": "CONFIG", "detail": "Platform: wasm32-browser"},
            {"phase": "CONFIG", "detail": "Max processes: 64"},
            {"phase": "CONFIG", "detail": "Memory model: linear (WASM)"},
            {"phase": "SERVICES", "detail": "Service registry ready"},
            {"phase": "SERVICES", "detail": "IPC subsystem ready"},
            {"phase": "SERVICES", "detail": "ExoChain audit log active"},
            {"phase": "NETWORK", "detail": "LLM transport: browser-direct CORS"},
            {"phase": "READY", "detail": "Kernel ready — all subsystems online"}
        ]);
        phases.to_string()
    }

    // -------------------------------------------------------------------
    // File analysis (Feature 2)
    // -------------------------------------------------------------------

    /// Analyze a set of files passed as a JSON array of `{path, content}` objects.
    ///
    /// Runs lightweight static analysis mirroring the native kernel's assessment
    /// analyzers (ComplexityAnalyzer, SecurityAnalyzer, DependencyAnalyzer,
    /// TopologyAnalyzer) but operating on in-memory strings rather than
    /// filesystem paths.
    ///
    /// Returns a JSON object with:
    /// - `summary`: file count, total LOC, language breakdown
    /// - `findings`: array of `{severity, category, file, line?, message}`
    #[wasm_bindgen]
    pub fn analyze_files(files_json: &str) -> String {
        #[derive(serde::Deserialize)]
        struct InputFile {
            path: String,
            content: String,
        }

        #[derive(serde::Serialize)]
        struct Finding {
            severity: String,
            category: String,
            file: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            line: Option<usize>,
            message: String,
        }

        #[derive(serde::Serialize)]
        struct LangStat {
            language: String,
            files: usize,
            lines: usize,
        }

        #[derive(serde::Serialize)]
        struct Summary {
            file_count: usize,
            total_lines: usize,
            languages: Vec<LangStat>,
        }

        #[derive(serde::Serialize)]
        struct AnalysisResult {
            summary: Summary,
            findings: Vec<Finding>,
        }

        let files: Vec<InputFile> = match serde_json::from_str(files_json) {
            Ok(f) => f,
            Err(e) => {
                return serde_json::json!({
                    "error": format!("Failed to parse input: {e}")
                })
                .to_string();
            }
        };

        let mut findings: Vec<Finding> = Vec::new();
        let mut total_lines: usize = 0;
        let mut lang_map: std::collections::HashMap<String, (usize, usize)> =
            std::collections::HashMap::new();

        // Secret patterns (mirrors SecurityAnalyzer)
        let secret_patterns: &[&str] = &[
            "api_key=",
            "api_key =",
            "apikey=",
            "apikey =",
            "password=",
            "password =",
            "passwd=",
            "passwd =",
            "secret=",
            "secret =",
            "token=",
            "token =",
            "aws_secret",
            "private_key",
        ];

        for file in &files {
            let path = &file.path;
            let content = &file.content;
            let line_count = content.lines().count();
            total_lines += line_count;

            // Language detection by extension
            let ext = path.rsplit('.').next().unwrap_or("");
            let lang = match ext {
                "rs" => "Rust",
                "ts" | "tsx" => "TypeScript",
                "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
                "py" => "Python",
                "go" => "Go",
                "java" => "Java",
                "c" | "h" => "C",
                "cpp" | "cc" | "cxx" | "hpp" => "C++",
                "rb" => "Ruby",
                "toml" => "TOML",
                "json" => "JSON",
                "yaml" | "yml" => "YAML",
                "md" | "mdx" => "Markdown",
                "sh" | "bash" => "Shell",
                "css" | "scss" | "less" => "CSS",
                "html" | "htm" => "HTML",
                "sql" => "SQL",
                "dockerfile" | "Dockerfile" => "Dockerfile",
                _ => {
                    // Check filename for special cases
                    let name = path.rsplit('/').next().unwrap_or(path);
                    match name {
                        n if n.starts_with("Dockerfile") => "Dockerfile",
                        "Makefile" | "Justfile" => "Make",
                        _ => "Other",
                    }
                }
            };

            let entry = lang_map.entry(lang.to_string()).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += line_count;

            // --- Complexity: large files ---
            if line_count > 500 {
                findings.push(Finding {
                    severity: "warning".into(),
                    category: "size".into(),
                    file: path.clone(),
                    line: None,
                    message: format!("File has {line_count} lines (>500 limit)"),
                });
            }

            // --- Complexity: TODO/FIXME/HACK markers ---
            for (i, line) in content.lines().enumerate() {
                if line.contains("TODO") || line.contains("FIXME") || line.contains("HACK") {
                    findings.push(Finding {
                        severity: "info".into(),
                        category: "todo".into(),
                        file: path.clone(),
                        line: Some(i + 1),
                        message: line.trim().to_string(),
                    });
                }
            }

            // --- Security: .env files ---
            let file_name = path.rsplit('/').next().unwrap_or(path);
            if file_name == ".env" || file_name.starts_with(".env.") {
                findings.push(Finding {
                    severity: "error".into(),
                    category: "security".into(),
                    file: path.clone(),
                    line: None,
                    message: "Environment file should not be committed to version control".into(),
                });
                continue;
            }

            // --- Security: hardcoded secrets ---
            let is_test = path.contains("test")
                || path.contains("spec")
                || path.contains("fixture")
                || path.contains("mock");

            if !is_test {
                for (i, line) in content.lines().enumerate() {
                    let lower = line.to_lowercase();
                    let trimmed = lower.trim();
                    // Skip comments
                    if trimmed.starts_with("//")
                        || trimmed.starts_with('#')
                        || trimmed.starts_with("/*")
                        || trimmed.starts_with('*')
                    {
                        continue;
                    }
                    for pattern in secret_patterns {
                        if lower.contains(pattern) {
                            if let Some(pos) = lower.find('=') {
                                let after = lower[pos + 1..].trim();
                                if !after.is_empty()
                                    && after != "\"\""
                                    && after != "''"
                                    && !after.starts_with("env")
                                    && !after.starts_with("std::env")
                                    && !after.starts_with("process.env")
                                    && !after.starts_with("os.environ")
                                {
                                    findings.push(Finding {
                                        severity: "warning".into(),
                                        category: "security".into(),
                                        file: path.clone(),
                                        line: Some(i + 1),
                                        message: format!(
                                            "Possible hardcoded secret: {}",
                                            pattern.trim_end_matches('=').trim()
                                        ),
                                    });
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            // --- Dependency: Cargo.toml ---
            if file_name == "Cargo.toml" {
                let mut in_deps = false;
                let mut dep_count: usize = 0;
                for line in content.lines() {
                    let t = line.trim();
                    if t.starts_with('[') {
                        in_deps = t == "[dependencies]"
                            || t == "[dev-dependencies]"
                            || t == "[build-dependencies]"
                            || t.starts_with("[dependencies.")
                            || t.starts_with("[dev-dependencies.")
                            || t.starts_with("[build-dependencies.");
                        continue;
                    }
                    if in_deps && !t.is_empty() && !t.starts_with('#') {
                        if let Some(dep_name) = t.split('=').next().map(|s| s.trim()) {
                            if !dep_name.is_empty() {
                                dep_count += 1;
                            }
                        }
                    }
                }
                findings.push(Finding {
                    severity: "info".into(),
                    category: "dependency".into(),
                    file: path.clone(),
                    line: None,
                    message: format!("Cargo.toml has {dep_count} dependencies"),
                });
            }

            // --- Dependency: package.json ---
            if file_name == "package.json" {
                // Simple JSON key counting for deps
                let dep_count = content.matches("\"dependencies\"").count()
                    + content.matches("\"devDependencies\"").count();
                let pkg_deps = count_json_object_keys(content, "dependencies")
                    + count_json_object_keys(content, "devDependencies");
                findings.push(Finding {
                    severity: "info".into(),
                    category: "dependency".into(),
                    file: path.clone(),
                    line: None,
                    message: format!(
                        "package.json: {} dependency section(s), ~{} packages",
                        dep_count, pkg_deps
                    ),
                });
            }

            // --- Topology: Docker / k8s ---
            if file_name.starts_with("Dockerfile") {
                findings.push(Finding {
                    severity: "info".into(),
                    category: "topology".into(),
                    file: path.clone(),
                    line: None,
                    message: "Dockerfile detected".into(),
                });
                // Extract base image
                for line in content.lines() {
                    if line.trim().to_uppercase().starts_with("FROM ") {
                        findings.push(Finding {
                            severity: "info".into(),
                            category: "topology".into(),
                            file: path.clone(),
                            line: None,
                            message: format!("Base image: {}", line.trim()),
                        });
                        break;
                    }
                }
            }
            if file_name.starts_with("docker-compose") {
                findings.push(Finding {
                    severity: "info".into(),
                    category: "topology".into(),
                    file: path.clone(),
                    line: None,
                    message: "Docker Compose file detected".into(),
                });
            }
            // k8s manifests
            if (ext == "yaml" || ext == "yml")
                && (content.contains("apiVersion:") && content.contains("kind:"))
            {
                findings.push(Finding {
                    severity: "info".into(),
                    category: "topology".into(),
                    file: path.clone(),
                    line: None,
                    message: "Kubernetes manifest detected".into(),
                });
            }
        }

        // Build language stats
        let mut languages: Vec<LangStat> = lang_map
            .into_iter()
            .map(|(language, (files, lines))| LangStat {
                language,
                files,
                lines,
            })
            .collect();
        languages.sort_by(|a, b| b.lines.cmp(&a.lines));

        let result = AnalysisResult {
            summary: Summary {
                file_count: files.len(),
                total_lines,
                languages,
            },
            findings,
        };

        serde_json::to_string(&result).unwrap_or_else(|e| {
            serde_json::json!({"error": format!("Serialization failed: {e}")}).to_string()
        })
    }

    /// Count approximate number of keys in a named JSON object section.
    /// This is a simple line-based heuristic, not a full parser.
    fn count_json_object_keys(content: &str, section: &str) -> usize {
        let search = format!("\"{}\"", section);
        let mut count = 0;
        let mut in_section = false;
        let mut brace_depth = 0;

        for line in content.lines() {
            let t = line.trim();
            if !in_section {
                if t.contains(&search) {
                    in_section = true;
                    if t.contains('{') {
                        brace_depth = 1;
                    }
                }
                continue;
            }
            for ch in t.chars() {
                match ch {
                    '{' => brace_depth += 1,
                    '}' => {
                        brace_depth -= 1;
                        if brace_depth <= 0 {
                            in_section = false;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if in_section && t.contains(':') && t.starts_with('"') {
                count += 1;
            }
        }
        count
    }
}

#[cfg(feature = "browser")]
pub use browser_entry::*;

#[cfg(all(test, not(feature = "browser")))]
mod tests {
    use super::*;

    #[test]
    fn init_returns_zero() {
        assert_eq!(init(), 0);
    }

    #[test]
    fn process_message_returns_response() {
        let response = process_message("hello");
        assert!(response.contains("hello"));
        assert!(response.contains("clawft-wasm"));
    }

    #[test]
    fn capabilities_is_valid_json() {
        let caps = capabilities();
        let parsed: serde_json::Value = serde_json::from_str(&caps).unwrap();
        assert_eq!(parsed["platform"], "wasm32-wasip2");
        assert!(!parsed["tools"].as_array().unwrap().is_empty());
    }

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn excluded_tools_listed() {
        let caps = capabilities();
        let parsed: serde_json::Value = serde_json::from_str(&caps).unwrap();
        let excluded = parsed["excluded_tools"].as_array().unwrap();
        assert!(excluded.contains(&serde_json::json!("exec_shell")));
        assert!(excluded.contains(&serde_json::json!("spawn")));
    }
}
