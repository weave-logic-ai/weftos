//! `weft mcp-server` -- run clawft as an MCP server over stdio or HTTP/SSE.
//!
//! Exposes a **profiled** tool surface (ADR-076) as an MCP tool server over
//! stdin/stdout (default) or `--listen` HTTP/SSE (ADR-075 G4 / WEFT-696).
//! Product default is `--profile default` (`control` ∪ `workspace`), not a
//! full agent-tool dump.
//!
//! # Public wire (ADR-076 C2 / WEFT-700)
//!
//! - Flat product names — no `builtin__` / `skill__` namespace leak
//! - OS subprocess tool is `process_spawn` (not bare `spawn`)
//! - Default profiles expose `skill_list` + `skill_get` façades only
//! - Proxied MCP re-export requires `--profile full` **and** `--reexport-mcp`
//! - `--attach` (ADR-076 C3) binds control tools to live kernel/weave
//!
//! # Remote listen (ADR-075 G4 / WEFT-696)
//!
//! ```text
//! weft mcp-server --listen 127.0.0.1:8742 --token "$WEFT_MCP_TOKEN"
//! weft mcp-server --listen 0.0.0.0:8742 --token-env WEFT_MCP_TOKEN --dangerously-bind-public
//! ```
//!
//! Auth is **required** by default for listen mode. Non-loopback bind needs
//! `--dangerously-bind-public`. Open (unauthenticated) public bind is refused.
//! Session capability tokens + audit client labels: WEFT-697 / ADR-075 G5.
//!
//! # Lifecycle
//!
//! ```text
//! 1. Optional: connect attach façade (fails clear if daemon down)
//! 2. Load config & build tool registry (profile + reexport flag)
//! 3. Filter tool defs by serve profile; public wire renames
//! 4. skill_list/get façades (control/default); per-skill only on full
//! 5. Build middleware pipeline (security, allowed_tools, audit)
//! 6. Create McpServerShell → stdio OR HTTP/SSE --listen
//! ```
//!
//! # Example
//!
//! ```text
//! weft mcp-server
//! weft mcp-server --profile default
//! weft mcp-server --profile full
//! weft mcp-server --profile full --reexport-mcp
//! weft mcp-server --attach --profile control
//! weft mcp-server --profile control,media --config /path/to/config.json
//! weft mcp-server --listen 127.0.0.1:8742 --token-env WEFT_MCP_TOKEN
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;
use tracing::info;

use clawft_core::agent::skills_v2::SkillRegistry;
use clawft_core::tools::registry::ToolRegistry;
use clawft_platform::NativePlatform;
use clawft_services::mcp::BuiltinToolProvider;
use clawft_services::mcp::SkillToolProvider;
use clawft_services::mcp::ToolDefinition;
use clawft_services::mcp::composite::CompositeToolProvider;
use clawft_services::mcp::middleware::{
    AuditLog, Middleware, PermissionFilter, ResultGuard, SecurityGuard,
};
use clawft_services::mcp::provider::ToolProvider;
use clawft_services::mcp::server::McpServerShell;
use clawft_services::mcp::session_cap::{
    SessionScopes, SessionTokenStore, validate_listen_policy,
};

use super::mcp_attach::{AttachToolProvider, DaemonAttachFacade};
use super::mcp_profile::{filter_tools_by_profile, ProfileSet};
use super::mcp_window::{WindowIntentToolProvider, WINDOW_TOOL_NAMES};
use super::{load_config, CoreToolRegisterOpts};

/// Arguments for the `weft mcp-server` subcommand.
#[derive(Args)]
pub struct McpServerArgs {
    /// Config file path (overrides auto-discovery).
    #[arg(short, long)]
    pub config: Option<String>,

    /// Serve profile: `default` | `control` | `workspace` | `media` | `full`
    /// (comma-compose, e.g. `control,media`). Default = `default`
    /// (`control` ∪ `workspace`) — not full dump. See ADR-076.
    #[arg(long, default_value = "default")]
    pub profile: String,

    /// Re-export inbound/proxied external MCP tools onto this server.
    /// Requires `--profile full` (ADR-076 §8). Off by default even for
    /// `full` — prevents recursion / tool floods through peer clients.
    #[arg(long, default_value_t = false)]
    pub reexport_mcp: bool,

    /// Attach control tools to a live kernel/weave daemon (ADR-076 C3).
    ///
    /// Dispatches `status` / `agent_list` / `agent_spawn` / `agent_stop`
    /// via weave RPC. Fails at startup if no daemon is running.
    /// Standalone (this flag omitted) remains the offline/dev default.
    #[arg(long, default_value_t = false)]
    pub attach: bool,

    /// Listen for remote MCP over HTTP/SSE (ADR-075 G4). Format: `host:port`
    /// (e.g. `127.0.0.1:8742`). When set, stdio is not used.
    ///
    /// Auth is required by default (`--token` / `--token-env` / `WEFT_MCP_TOKEN`).
    /// Non-loopback bind requires `--dangerously-bind-public`.
    #[arg(long, value_name = "HOST:PORT")]
    pub listen: Option<String>,

    /// Bearer capability token for `--listen` auth (WEFT-696 / WEFT-697).
    /// Prefer `--token-env` so the secret does not appear in process listings.
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,

    /// Read bearer token from this environment variable (default search:
    /// `WEFT_MCP_TOKEN` when `--listen` is set and `--token` is omitted).
    #[arg(long, value_name = "ENV_VAR")]
    pub token_env: Option<String>,

    /// Allow unauthenticated MCP (loopback only). Never permits open public bind.
    #[arg(long, default_value_t = false)]
    pub allow_unauthenticated: bool,

    /// Explicit opt-in to bind a non-loopback address (`0.0.0.0`, LAN IP, …).
    /// Still requires auth; open public bind is refused.
    #[arg(long, default_value_t = false)]
    pub dangerously_bind_public: bool,

    /// Enterprise reduced scopes: force no `agent_spawn` and a restricted
    /// tool pattern set on every session token (WEFT-697).
    #[arg(long, default_value_t = false)]
    pub enterprise: bool,

    /// Issue a one-shot capability token to stdout and exit (JSON:
    /// `{ "token", "scopes", "client_label" }`). Useful for minting remote
    /// Grok headers without running the server. Does not print secrets of
    /// other sessions.
    #[arg(long, default_value_t = false)]
    pub issue_token: bool,

    /// Client label to stamp on `--issue-token` (`grok` | `claude` | `unknown`).
    #[arg(long, default_value = "unknown")]
    pub client_label: String,
}

/// Whether proxied external MCP tools should be registered on the serve
/// path (ADR-076 §8 / WEFT-700).
///
/// Requires **both** the `full` profile and the explicit `--reexport-mcp`
/// flag. `full` alone is not enough.
pub fn should_reexport_mcp(profiles: &ProfileSet, reexport_flag: bool) -> bool {
    profiles.allows_mcp_reexport() && reexport_flag
}

/// Resolve the bearer token for listen mode from CLI / env.
fn resolve_listen_token(args: &McpServerArgs) -> anyhow::Result<Option<String>> {
    if let Some(t) = args.token.as_ref().filter(|s| !s.is_empty()) {
        return Ok(Some(t.clone()));
    }
    let env_name = args
        .token_env
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("WEFT_MCP_TOKEN");
    match std::env::var(env_name) {
        Ok(v) if !v.is_empty() => Ok(Some(v)),
        Ok(_) | Err(_) => {
            if args.token_env.is_some() {
                anyhow::bail!("environment variable '{env_name}' is unset or empty");
            }
            Ok(None)
        }
    }
}

/// Run the MCP server command.
///
/// Loads configuration, builds the tool registry, filters by serve profile
/// (ADR-076 / WEFT-699 / WEFT-700), and serves tools over stdio or HTTP/SSE
/// (`--listen`) using [`McpServerShell`] with the full middleware pipeline.
pub async fn run(args: McpServerArgs) -> anyhow::Result<()> {
    let profiles = ProfileSet::parse(&args.profile).map_err(|e| anyhow::anyhow!(e))?;
    let reexport = should_reexport_mcp(&profiles, args.reexport_mcp);
    info!(
        profile = %profiles.label(),
        full = profiles.includes_full(),
        reexport_mcp = reexport,
        attach = args.attach,
        listen = args.listen.as_deref().unwrap_or(""),
        enterprise = args.enterprise,
        "starting weft mcp-server"
    );

    // ── Issue token only (no serve) ──────────────────────────────────
    if args.issue_token {
        return run_issue_token(&args);
    }

    // ── Attach façade (fail clear before building the rest) ──────────
    let attach_provider = if args.attach {
        let facade = DaemonAttachFacade::connect()
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        let provider = AttachToolProvider::with_profile(Arc::new(facade), |name| {
            profiles.allows_tool(name)
        });
        let attach_names: Vec<String> =
            provider.list_tools().iter().map(|t| t.name.clone()).collect();
        if attach_names.is_empty() {
            info!(
                profile = %profiles.label(),
                "attach connected but profile excludes control tools"
            );
        } else {
            info!(
                tools = ?attach_names,
                "attach façade registered (live kernel control tools)"
            );
        }
        Some(provider)
    } else {
        info!("standalone mode (no --attach); control tools not bound to kernel");
        None
    };

    let platform = Arc::new(NativePlatform::new());
    let config = load_config(&*platform, args.config.as_deref()).await?;
    let BuiltShell {
        mut shell,
        audit_label,
        listed_names,
    } = build_shell(
        &profiles,
        reexport,
        attach_provider,
        &config,
        platform,
        args.listen.is_none(),
    )
    .await;

    // ── HTTP/SSE listen mode (WEFT-696) ──────────────────────────────
    if let Some(listen) = args.listen.as_deref() {
        return run_listen_mode(&args, listen, shell, audit_label).await;
    }

    info!(
        profile = %profiles.label(),
        tools = listed_names.len(),
        names = ?listed_names,
        "MCP server ready, reading from stdin"
    );

    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    let stdout = tokio::io::stdout();
    shell.run(stdin, stdout).await?;

    info!("stdin closed, MCP server shutting down");
    Ok(())
}

/// A built MCP server shell plus what callers need to report on it.
pub(crate) struct BuiltShell {
    pub shell: McpServerShell,
    /// Handle the HTTP layer writes the per-request client label into.
    pub audit_label: Arc<std::sync::RwLock<String>>,
    /// Public tool names the shell lists.
    pub listed_names: Vec<String>,
}

/// Build the profile-filtered tool surface and middleware pipeline.
///
/// Shared by `weft mcp-server` and the gateway's `/mcp` mount (ADR-102 D2).
/// `stdio` labels the audit client `unknown` for a local stdio attach.
pub(crate) async fn build_shell(
    profiles: &ProfileSet,
    reexport: bool,
    attach_provider: Option<AttachToolProvider>,
    config: &clawft_types::config::Config,
    platform: Arc<NativePlatform>,
    stdio: bool,
) -> BuiltShell {
    // ── WindowIntent tools (ADR-075 G3 / ADR-076 C4) ─────────────────
    // Always register when the profile allows any window_* tool. Headless
    // mcp-server still enqueues onto an in-process bus (submit_mcp) so
    // clients get a stable wire; a live Agent Workspace drains the same
    // ExternalIntentTx shape when co-located later.
    let window_provider = {
        let allows = |name: &str| profiles.allows_tool(name);
        let (provider, _bus) = WindowIntentToolProvider::local_with_profile(allows);
        let names: Vec<String> = provider.list_tools().iter().map(|t| t.name.clone()).collect();
        if names.is_empty() {
            info!(
                profile = %profiles.label(),
                catalog = ?WINDOW_TOOL_NAMES,
                "window_* tools excluded by profile"
            );
            None
        } else {
            info!(
                tools = ?names,
                catalog = ?WINDOW_TOOL_NAMES,
                "WindowIntent MCP tools registered (submit_mcp)"
            );
            Some(provider)
        }
    };


    // ── Build tool registry (profile + flag gates MCP re-export) ─────
    let mut registry = ToolRegistry::new();
    super::register_core_tools_with(
        &mut registry,
        &config,
        platform.clone(),
        CoreToolRegisterOpts {
            // Proxied external MCP: full **and** --reexport-mcp (ADR-076 §8).
            register_mcp: reexport,
            // `delegate_task` is catalog `full` only.
            register_delegation: profiles.includes_full(),
        },
    )
    .await;

    info!(
        registry_tools = registry.len(),
        reexport_mcp = reexport,
        "tool registry initialized for MCP server"
    );

    // ── Convert registry → defs, profile-filter, public wire renames ─
    let mut tool_defs = filter_tools_by_profile(build_tool_definitions(&registry), &profiles);
    apply_public_wire_renames(&mut tool_defs);
    if attach_provider.is_some() {
        tool_defs = strip_attach_owned_names(tool_defs);
    }

    // ── Skills discovery (façades always when control; expand on full) ─
    let skill_state = Arc::new(load_skill_facade_state().await);

    if profiles.allows_tool("skill_list") {
        // skill_list / skill_get façades — not one tool per skill (ADR-076 §7).
        for facade in skill_facade_definitions() {
            if !tool_defs.iter().any(|t| t.name == facade.name) {
                tool_defs.push(facade);
            }
        }
        info!(
            skills = skill_state.entries.len(),
            "skill_list/skill_get façades registered"
        );
    }

    let served_names: Vec<String> = tool_defs.iter().map(|t| t.name.clone()).collect();
    let served_count = tool_defs.len();
    debug_assert!(
        served_names.iter().all(|n| !n.contains("builtin__")),
        "public wire must not leak builtin__ prefix"
    );
    info!(
        profile = %profiles.label(),
        tools = served_count,
        names = ?served_names,
        "MCP serve profile applied (public wire)"
    );

    let provider = build_public_provider(tool_defs, registry, skill_state.clone());

    // ── Build CompositeToolProvider (empty ns → flat product names) ──
    let mut composite = CompositeToolProvider::new();
    if let Some(ap) = attach_provider {
        composite.register(Box::new(ap));
    }
    if let Some(wp) = window_provider {
        composite.register(Box::new(wp));
    }
    composite.register(Box::new(provider));

    // ── Skills: expand one-tool-per-skill only on `full` (ADR-076 §7) ─
    if profiles.allows_skill_tools() {
        register_skill_tools_expanded(&mut composite, &skill_state);
    } else {
        info!(
            profile = %profiles.label(),
            "skipping per-skill MCP tools (not full); skill_list/get façades only"
        );
    }

    // Final list-tools surface (what clients see after composite).
    let listed = composite.list_tools_all();
    let listed_names: Vec<String> = listed.iter().map(|t| t.name.clone()).collect();
    assert_public_wire_names(&listed_names);

    // ── Build middleware pipeline ────────────────────────────────────
    let security_guard = build_security_guard(&config.tools);

    // Profile filter is primary. Empty `tools.allowed_tools` is NOT a
    // full dump — it only means "no extra allowlist on top of profile".
    // Non-empty allowed_tools further restricts the already-filtered set.
    let permission_filter = if config.tools.allowed_tools.is_empty() {
        info!(
            profile = %profiles.label(),
            "tools.allowed_tools empty; profile filter is authoritative"
        );
        PermissionFilter::new(None)
    } else {
        info!(
            patterns = ?config.tools.allowed_tools,
            "tools.allowed_tools is configured; additional allowlist on profile surface",
        );
        PermissionFilter::from_patterns(config.tools.allowed_tools.clone())
    };

    let audit = AuditLog::new();
    let audit_label = audit.label_handle();
    // stdio local attach: label as unknown (or process can set later).
    if stdio {
        audit.set_client_label("unknown");
    }

    let middlewares: Vec<Box<dyn Middleware>> = vec![
        Box::new(security_guard),
        Box::new(permission_filter),
        Box::new(ResultGuard::default()),
        Box::new(audit),
    ];

    // ── Create McpServerShell ────────────────────────────────────────
    let mut shell = McpServerShell::new(composite);
    for mw in middlewares {
        shell.add_middleware(mw);
    }

    BuiltShell {
        shell,
        audit_label,
        listed_names,
    }
}

/// The `/mcp` surface the gateway mounts (ADR-102 D2): the `full` profile.
///
/// Control tools attach to the live daemon when one answers at startup; if
/// none does they are left out (standalone), and a restart picks them up.
/// Proxied external MCP stays off (`--reexport-mcp` semantics).
#[cfg(feature = "api")]
pub(crate) async fn build_gateway_mount(
    config: &clawft_types::config::Config,
    platform: Arc<NativePlatform>,
) -> Option<Arc<clawft_services::api::mcp_mount::McpMount>> {
    let profiles = ProfileSet::parse("full").ok()?;
    let attach_provider = match DaemonAttachFacade::connect().await {
        Ok(facade) => Some(AttachToolProvider::with_profile(Arc::new(facade), |name| {
            profiles.allows_tool(name)
        })),
        Err(e) => {
            tracing::warn!(error = %e, "gateway /mcp: daemon not attached; control tools omitted");
            None
        }
    };
    let built = build_shell(&profiles, false, attach_provider, config, platform, false).await;
    info!(tools = built.listed_names.len(), "gateway /mcp mounted (full profile)");
    Some(Arc::new(clawft_services::api::mcp_mount::McpMount::new(
        built.shell,
        built.audit_label,
        profiles.label(),
        built.listed_names.len(),
    )))
}

/// Mint a capability token and print JSON (no secrets of other sessions).
fn run_issue_token(args: &McpServerArgs) -> anyhow::Result<()> {
    use clawft_services::mcp::session_cap::ClientLabel;

    let store = SessionTokenStore::new(true);
    if args.enterprise {
        store.enable_enterprise_reduced();
    }
    let label = ClientLabel::parse(&args.client_label);
    let scopes = if args.enterprise {
        SessionScopes::enterprise_reduced()
    } else {
        SessionScopes::owner()
    };
    let token = store
        .issue(scopes.clone(), label, None)
        .ok_or_else(|| anyhow::anyhow!("failed to issue token"))?;
    // Note: store is dropped; token is for the caller to put into a live server's store.
    // For production minting against a running server, use a control API later.
    // Here we print the token + scope document so operators can also set --token.
    let doc = serde_json::json!({
        "token": token,
        "client_label": label.as_str(),
        "scopes": {
            "tools": scopes.tools,
            "workspace_roots": scopes.workspace_roots,
            "agent_spawn": scopes.agent_spawn,
        },
        "note": "Pass token via --token / WEFT_MCP_TOKEN on the listen server; issued map is in-process only — for static bearer use the printed token as --token on the server."
    });
    println!("{}", serde_json::to_string_pretty(&doc)?);
    Ok(())
}

/// HTTP/SSE remote serve path (requires clawft-services `api` feature via CLI `api`).
#[cfg(feature = "api")]
async fn run_listen_mode(
    args: &McpServerArgs,
    listen: &str,
    shell: McpServerShell,
    audit_label: Arc<std::sync::RwLock<String>>,
) -> anyhow::Result<()> {
    use clawft_services::mcp::{HttpMcpConfig, HttpMcpState, serve_mcp_http};

    let (host, port) = HttpMcpConfig::parse_listen(listen).map_err(|e| anyhow::anyhow!(e))?;

    let allow_unauth = args.allow_unauthenticated;
    let auth_required = !allow_unauth;

    validate_listen_policy(
        &host,
        auth_required,
        allow_unauth,
        args.dangerously_bind_public,
    )
    .map_err(|e| anyhow::anyhow!(e))?;

    let token = resolve_listen_token(args)?;
    if auth_required && token.is_none() {
        anyhow::bail!(
            "listen mode requires auth: pass --token, --token-env, or set WEFT_MCP_TOKEN \
             (or use --allow-unauthenticated on loopback only)"
        );
    }

    let store = Arc::new(SessionTokenStore::new(auth_required));
    if let Some(t) = token {
        store.add_static_token(t);
        info!("MCP bearer token loaded (static capability)");
    }
    if args.enterprise {
        store.enable_enterprise_reduced();
        info!("enterprise reduced scopes enabled (no agent_spawn; restricted tools)");
    }

    let state = Arc::new(HttpMcpState::new(shell, store, audit_label));
    let config = HttpMcpConfig {
        host,
        port,
        allow_unauthenticated: allow_unauth,
        dangerously_bind_public: args.dangerously_bind_public,
    };

    info!(
        listen = %listen,
        auth_required,
        enterprise = args.enterprise,
        "MCP HTTP/SSE listen mode (POST /mcp, GET /mcp/sse); TLS via reverse proxy"
    );

    serve_mcp_http(config, state)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(())
}

#[cfg(not(feature = "api"))]
async fn run_listen_mode(
    _args: &McpServerArgs,
    _listen: &str,
    _shell: McpServerShell,
    _audit_label: Arc<std::sync::RwLock<String>>,
) -> anyhow::Result<()> {
    anyhow::bail!(
        "--listen requires the CLI to be built with the `api` feature \
         (default for weft). Rebuild: scripts/build.sh native"
    );
}

/// Metadata for skill_list / skill_get façades.
#[derive(Debug, Clone)]
struct SkillEntry {
    name: String,
    description: String,
    instructions: String,
}

#[derive(Debug, Default)]
struct SkillFacadeState {
    entries: Vec<SkillEntry>,
}

impl SkillFacadeState {
    fn list_json(&self) -> String {
        let items: Vec<serde_json::Value> = self
            .entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "name": e.name,
                    "description": e.description,
                })
            })
            .collect();
        serde_json::to_string(&serde_json::json!({ "skills": items })).unwrap_or_else(|_| {
            "{\"skills\":[]}".into()
        })
    }

    fn get_body(&self, name: &str) -> Result<String, String> {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.instructions.clone())
            .ok_or_else(|| format!("skill '{name}' not found"))
    }

    fn instructions_map(&self) -> HashMap<String, String> {
        self.entries
            .iter()
            .map(|e| (e.name.clone(), e.instructions.clone()))
            .collect()
    }
}

async fn load_skill_facade_state() -> SkillFacadeState {
    let (ws_skills_dir, user_skills_dir) = discover_skill_dirs();
    let skill_registry = match SkillRegistry::discover(
        ws_skills_dir.as_deref(),
        user_skills_dir.as_deref(),
        Vec::new(),
    )
    .await
    {
        Ok(reg) => reg,
        Err(e) => {
            info!(error = %e, "skill discovery failed, continuing without skills");
            match SkillRegistry::discover(None, None, Vec::new()).await {
                Ok(reg) => reg,
                Err(_) => return SkillFacadeState::default(),
            }
        }
    };

    let entries = skill_registry
        .list()
        .into_iter()
        .map(|s| SkillEntry {
            name: s.name.clone(),
            description: s.description.clone(),
            instructions: s.instructions.clone(),
        })
        .collect();
    SkillFacadeState { entries }
}

/// Public MCP façades for skills (ADR-076 §7) — list + get only.
fn skill_facade_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "skill_list".into(),
            description: "List available WeftOS/clawft skills (name + description). \
                 Prefer this over one MCP tool per skill. Agent lifecycle tools \
                 (when present) use agent_* names, not skill tools."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
        },
        ToolDefinition {
            name: "skill_get".into(),
            description: "Get the SKILL.md body for a named skill. Returns instructional \
                 prompt text (not an executed agent run)."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Skill name as returned by skill_list"
                    }
                },
                "required": ["name"]
            }),
        },
    ]
}

/// Rename ambiguous internal tool names to public product names (ADR-076 §4).
///
/// - `spawn` (OS subprocess) → `process_spawn`
///
/// Bare `spawn` is removed from the public list so it cannot be confused with
/// agent lifecycle (`agent_spawn`, etc.).
pub fn apply_public_wire_renames(tools: &mut Vec<ToolDefinition>) {
    tools.retain_mut(|t| {
        if t.name == "spawn" {
            t.name = "process_spawn".into();
            if !t.description.to_ascii_lowercase().contains("process") {
                t.description = format!(
                    "OS subprocess spawn (public name process_spawn). Not an agent spawn — \
                     use agent_spawn for WeftOS agents when available. {}",
                    t.description
                );
            }
        }
        // Drop any accidental bare spawn duplicates after rename.
        true
    });
    // If both spawn and process_spawn somehow exist, keep one process_spawn.
    let mut seen_ps = false;
    tools.retain(|t| {
        if t.name == "process_spawn" {
            if seen_ps {
                return false;
            }
            seen_ps = true;
        }
        true
    });
}

/// Assert client-visible names have no internal namespace leak.
fn assert_public_wire_names(names: &[String]) {
    for n in names {
        debug_assert!(
            !n.starts_with("builtin__") && !n.contains("builtin__"),
            "public MCP name leaked builtin__ prefix: {n}"
        );
    }
}

/// Register one MCP tool per skill (`full` only), flat names (no `skill__`).
fn register_skill_tools_expanded(
    composite: &mut CompositeToolProvider,
    skill_state: &SkillFacadeState,
) {
    if skill_state.entries.is_empty() {
        info!("no skills found, skipping per-skill tool registration");
        return;
    }

    // One ToolDefinition per skill name (full profile expansion only).
    let skill_defs: Vec<ToolDefinition> = skill_state
        .entries
        .iter()
        .map(|e| ToolDefinition {
            name: e.name.clone(),
            description: e.description.clone(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
        })
        .collect();

    let instructions_map = Arc::new(skill_state.instructions_map());

    let skill_provider = SkillToolProvider::new(skill_defs, move |name, _args| {
        let map = instructions_map.clone();
        let name = name.to_string();
        Box::pin(async move {
            match map.get(&name) {
                Some(instructions) => Ok(instructions.clone()),
                None => Err(format!("skill '{name}' not found")),
            }
        })
    })
    // Flat product names on public wire (no skill__ prefix).
    .with_namespace("");

    info!(
        skills = skill_state.entries.len(),
        "skill tools expanded for MCP server (full profile, flat names)"
    );
    composite.register(Box::new(skill_provider));
}

/// Build [`ToolDefinition`] list from a populated [`ToolRegistry`].
fn build_tool_definitions(registry: &ToolRegistry) -> Vec<ToolDefinition> {
    let schemas = registry.schemas();
    schemas
        .into_iter()
        .filter_map(|schema| {
            let func = schema.get("function")?;
            Some(ToolDefinition {
                name: func.get("name")?.as_str()?.to_string(),
                description: func
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                input_schema: func
                    .get("parameters")
                    .cloned()
                    .unwrap_or(serde_json::json!({"type": "object"})),
            })
        })
        .collect()
}

/// Build a flat public [`BuiltinToolProvider`] (empty namespace).
///
/// Maps public wire names to registry execution:
/// - `process_spawn` → registry tool `spawn`
/// - `skill_list` / `skill_get` → skill façades
fn build_public_provider(
    tool_defs: Vec<ToolDefinition>,
    registry: ToolRegistry,
    skills: Arc<SkillFacadeState>,
) -> BuiltinToolProvider {
    let registry = Arc::new(registry);

    BuiltinToolProvider::new(tool_defs, move |name, args| {
        let reg = registry.clone();
        let skills = skills.clone();
        let name = name.to_string();
        Box::pin(async move {
            match name.as_str() {
                "process_spawn" => match reg.execute("spawn", args, None).await {
                    Ok(value) => Ok(serde_json::to_string(&value).unwrap_or_default()),
                    Err(e) => Err(e.to_string()),
                },
                "skill_list" => Ok(skills.list_json()),
                "skill_get" => {
                    let skill_name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "missing required field: name".to_string())?;
                    skills.get_body(skill_name)
                }
                other => match reg.execute(other, args, None).await {
                    Ok(value) => Ok(serde_json::to_string(&value).unwrap_or_default()),
                    Err(e) => Err(e.to_string()),
                },
            }
        })
    })
    // ADR-076 / WEFT-700: no `builtin__` prefix on client-visible names.
    .with_namespace("")
}

/// Legacy helper retained for unit tests that only need registry dispatch.
/// Remove registry tool defs whose names are owned by the attach façade.
fn strip_attach_owned_names(tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    use super::mcp_attach::ATTACH_TOOL_NAMES;
    tools
        .into_iter()
        .filter(|t| !ATTACH_TOOL_NAMES.contains(&t.name.as_str()))
        .collect()
}


#[cfg(test)]
fn build_builtin_provider(
    tool_defs: Vec<ToolDefinition>,
    registry: ToolRegistry,
) -> BuiltinToolProvider {
    build_public_provider(tool_defs, registry, Arc::new(SkillFacadeState::default()))
}

/// Build a [`SecurityGuard`] middleware from the tools configuration.
///
/// Translates the CLI-level `CommandPolicyConfig` and `UrlPolicyConfig`
/// into the canonical policy types used by [`SecurityGuard`].
fn build_security_guard(tools_config: &clawft_types::config::ToolsConfig) -> SecurityGuard {
    use clawft_types::security::{CommandPolicy, PolicyMode, UrlPolicy};

    let cmd_cfg = &tools_config.command_policy;
    let mode = match cmd_cfg.mode.as_str() {
        "denylist" => PolicyMode::Denylist,
        _ => PolicyMode::Allowlist,
    };
    let mut mw_cmd = CommandPolicy::safe_defaults();
    mw_cmd.mode = mode;
    if !cmd_cfg.allowlist.is_empty() {
        mw_cmd.allowlist = cmd_cfg.allowlist.iter().cloned().collect();
    }
    if !cmd_cfg.denylist.is_empty() {
        mw_cmd.denylist = cmd_cfg.denylist.clone();
    }

    let url_cfg = &tools_config.url_policy;
    let mw_url = UrlPolicy::new(
        url_cfg.enabled,
        url_cfg.allow_private,
        url_cfg.allowed_domains.iter().cloned().collect(),
        url_cfg.blocked_domains.iter().cloned().collect(),
    );

    SecurityGuard::new(mw_cmd, mw_url)
}

/// Discover workspace and user skill directories.
///
/// Returns `(workspace_dir, user_dir)`. Either may be `None` if the
/// directory does not exist or cannot be located.
///
/// - **Workspace skills**: Walk upward from `cwd` looking for
///   `.clawft/skills/`.
/// - **User skills**: `~/.clawft/skills/`.
fn discover_skill_dirs() -> (Option<PathBuf>, Option<PathBuf>) {
    let user_dir = dirs::home_dir().map(|h| h.join(".clawft").join("skills"));

    let ws_dir = std::env::current_dir().ok().and_then(|cwd| {
        let mut dir: &Path = cwd.as_path();
        loop {
            let candidate = dir.join(".clawft").join("skills");
            if candidate.is_dir() {
                return Some(candidate);
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => return None,
            }
        }
    });

    (ws_dir, user_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_args() -> McpServerArgs {
        McpServerArgs {
            config: None,
            profile: "default".into(),
            reexport_mcp: false,
            attach: false,
            listen: None,
            token: None,
            token_env: None,
            allow_unauthenticated: false,
            dangerously_bind_public: false,
            enterprise: false,
            issue_token: false,
            client_label: "unknown".into(),
        }
    }

    #[test]
    fn mcp_server_args_attach() {
        let mut args = sample_args();
        args.profile = "control".into();
        args.attach = true;
        assert!(args.attach);
        assert_eq!(args.profile, "control");
    }

    #[test]
    fn strip_attach_owned_names_drops_control_wires() {
        let tools = vec![
            ToolDefinition {
                name: "status".into(),
                description: "x".into(),
                input_schema: serde_json::json!({}),
            },
            ToolDefinition {
                name: "read_file".into(),
                description: "x".into(),
                input_schema: serde_json::json!({}),
            },
        ];
        let kept = strip_attach_owned_names(tools);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "read_file");
    }

    #[test]
    fn mcp_server_args_defaults() {
        let args = sample_args();
        assert!(args.config.is_none());
        assert_eq!(args.profile, "default");
        assert!(!args.reexport_mcp);
        assert!(args.listen.is_none());
        assert!(!args.enterprise);
        assert!(!args.dangerously_bind_public);
    }

    #[test]
    fn mcp_server_args_with_config() {
        let mut args = sample_args();
        args.config = Some("/tmp/config.json".into());
        args.profile = "full".into();
        assert_eq!(args.config.as_deref(), Some("/tmp/config.json"));
        assert_eq!(args.profile, "full");
        assert!(!args.reexport_mcp);
    }

    #[test]
    fn mcp_server_args_listen_flags() {
        let mut args = sample_args();
        args.listen = Some("127.0.0.1:8742".into());
        args.token = Some("sekret".into());
        args.enterprise = true;
        assert_eq!(args.listen.as_deref(), Some("127.0.0.1:8742"));
        assert!(args.enterprise);
        assert!(resolve_listen_token(&args).unwrap().as_deref() == Some("sekret"));
    }

    #[test]
    fn validate_listen_policy_wired() {
        assert!(validate_listen_policy("127.0.0.1", true, false, false).is_ok());
        assert!(validate_listen_policy("0.0.0.0", true, false, false).is_err());
    }

    // ── WEFT-700 public wire ─────────────────────────────────────────

    #[test]
    fn reexport_requires_full_and_flag() {
        let default = ProfileSet::product_default();
        let full = ProfileSet::parse("full").unwrap();

        assert!(!should_reexport_mcp(&default, false));
        assert!(!should_reexport_mcp(&default, true));
        assert!(
            !should_reexport_mcp(&full, false),
            "full without --reexport-mcp must not re-export"
        );
        assert!(should_reexport_mcp(&full, true));
    }

    #[test]
    fn public_wire_renames_spawn_to_process_spawn() {
        let mut tools = vec![
            ToolDefinition {
                name: "spawn".into(),
                description: "Run a subprocess".into(),
                input_schema: serde_json::json!({"type": "object"}),
            },
            ToolDefinition {
                name: "read_file".into(),
                description: "Read".into(),
                input_schema: serde_json::json!({"type": "object"}),
            },
        ];
        apply_public_wire_renames(&mut tools);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"process_spawn"));
        assert!(!names.contains(&"spawn"));
        assert!(names.contains(&"read_file"));
    }

    #[test]
    fn public_wire_dedupes_process_spawn() {
        let mut tools = vec![
            ToolDefinition {
                name: "spawn".into(),
                description: "OS".into(),
                input_schema: serde_json::json!({"type": "object"}),
            },
            ToolDefinition {
                name: "process_spawn".into(),
                description: "Already public".into(),
                input_schema: serde_json::json!({"type": "object"}),
            },
        ];
        apply_public_wire_renames(&mut tools);
        let count = tools.iter().filter(|t| t.name == "process_spawn").count();
        assert_eq!(count, 1);
        assert!(!tools.iter().any(|t| t.name == "spawn"));
    }

    #[test]
    fn skill_facade_definitions_are_list_and_get_only() {
        let defs = skill_facade_definitions();
        let names: Vec<&str> = defs.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["skill_list", "skill_get"]);
    }

    #[test]
    fn default_profile_surface_has_no_builtin_prefix_and_has_process_spawn() {
        use clawft_services::mcp::composite::CompositeToolProvider;

        let p = ProfileSet::product_default();
        let sample = vec![
            "read_file",
            "write_file",
            "exec_shell",
            "spawn",
            "web_search",
            "voice_listen",
            "delegate_task",
            "status",
            "claude-flow__swarm_init",
        ]
        .into_iter()
        .map(|name| ToolDefinition {
            name: name.into(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
        })
        .collect::<Vec<_>>();

        let mut filtered = filter_tools_by_profile(sample, &p);
        apply_public_wire_renames(&mut filtered);

        // skill façades for control ∪ workspace default
        if p.allows_tool("skill_list") {
            filtered.extend(skill_facade_definitions());
        }

        let provider = BuiltinToolProvider::new(filtered.clone(), |_n, _a| {
            Box::pin(async { Ok(String::new()) })
        })
        .with_namespace("");

        let mut composite = CompositeToolProvider::new();
        composite.register(Box::new(provider));
        let listed = composite.list_tools_all();
        let names: Vec<&str> = listed.iter().map(|t| t.name.as_str()).collect();

        assert!(
            names.iter().all(|n| !n.contains("builtin__")),
            "no builtin__ leak: {names:?}"
        );
        assert!(names.contains(&"process_spawn"));
        assert!(!names.contains(&"spawn"));
        assert!(names.contains(&"skill_list"));
        assert!(names.contains(&"skill_get"));
        assert!(!names.contains(&"voice_listen"));
        assert!(!names.contains(&"delegate_task"));
        assert!(!names.contains(&"claude-flow__swarm_init"));
        // No one-tool-per-skill flood: only façades for skills.
        assert_eq!(
            names.iter().filter(|n| n.starts_with("skill_")).count(),
            2,
            "only skill_list + skill_get"
        );
    }

    #[tokio::test]
    async fn process_spawn_dispatches_to_registry_spawn() {
        use clawft_core::tools::registry::{Tool, ToolError as CoreToolError, ToolRegistry};
        use clawft_services::mcp::ToolProvider;

        struct SpawnToolStub;

        #[async_trait::async_trait]
        impl Tool for SpawnToolStub {
            fn name(&self) -> &str {
                "spawn"
            }
            fn description(&self) -> &str {
                "OS spawn"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({"type": "object"})
            }
            async fn execute(
                &self,
                args: serde_json::Value,
            ) -> Result<serde_json::Value, CoreToolError> {
                Ok(serde_json::json!({"ran": args.get("command")}))
            }
        }

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(SpawnToolStub));

        let mut defs = build_tool_definitions(&registry);
        apply_public_wire_renames(&mut defs);
        assert_eq!(defs[0].name, "process_spawn");

        let provider = build_public_provider(defs, registry, Arc::new(SkillFacadeState::default()));
        let result = provider
            .call_tool(
                "process_spawn",
                serde_json::json!({"command": "echo"}),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        match &result.content[0] {
            clawft_services::mcp::ContentBlock::Text { text } => {
                let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
                assert_eq!(parsed["ran"], "echo");
            }
        }
    }

    #[tokio::test]
    async fn skill_facades_list_and_get() {
        use clawft_services::mcp::ToolProvider;

        let skills = Arc::new(SkillFacadeState {
            entries: vec![SkillEntry {
                name: "demo".into(),
                description: "Demo skill".into(),
                instructions: "Do the demo.".into(),
            }],
        });
        let defs = skill_facade_definitions();
        let provider = build_public_provider(defs, ToolRegistry::new(), skills);

        let list = provider
            .call_tool("skill_list", serde_json::json!({}))
            .await
            .unwrap();
        match &list.content[0] {
            clawft_services::mcp::ContentBlock::Text { text } => {
                let v: serde_json::Value = serde_json::from_str(text).unwrap();
                assert_eq!(v["skills"][0]["name"], "demo");
            }
        }

        let get = provider
            .call_tool("skill_get", serde_json::json!({"name": "demo"}))
            .await
            .unwrap();
        match &get.content[0] {
            clawft_services::mcp::ContentBlock::Text { text } => {
                assert_eq!(text, "Do the demo.");
            }
        }
    }

    #[test]
    fn build_tool_definitions_from_registry() {
        use clawft_core::tools::registry::{Tool, ToolError as CoreToolError, ToolRegistry};

        struct TestTool;

        #[async_trait::async_trait]
        impl Tool for TestTool {
            fn name(&self) -> &str {
                "test_tool"
            }
            fn description(&self) -> &str {
                "A test tool"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "input": { "type": "string" }
                    }
                })
            }
            async fn execute(
                &self,
                _args: serde_json::Value,
            ) -> Result<serde_json::Value, CoreToolError> {
                Ok(serde_json::json!({"output": "ok"}))
            }
        }

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool));

        let defs = build_tool_definitions(&registry);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "test_tool");
        assert_eq!(defs[0].description, "A test tool");
        assert_eq!(defs[0].input_schema["type"], "object");
    }

    #[test]
    fn build_tool_definitions_empty_registry() {
        let registry = ToolRegistry::new();
        let defs = build_tool_definitions(&registry);
        assert!(defs.is_empty());
    }

    #[tokio::test]
    async fn builtin_provider_dispatches_to_registry() {
        use clawft_core::tools::registry::{Tool, ToolError as CoreToolError, ToolRegistry};
        use clawft_services::mcp::ToolProvider;

        struct EchoTool;

        #[async_trait::async_trait]
        impl Tool for EchoTool {
            fn name(&self) -> &str {
                "echo"
            }
            fn description(&self) -> &str {
                "Echo input"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string" }
                    },
                    "required": ["text"]
                })
            }
            async fn execute(
                &self,
                args: serde_json::Value,
            ) -> Result<serde_json::Value, CoreToolError> {
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| CoreToolError::InvalidArgs("missing 'text'".into()))?;
                Ok(serde_json::json!({"output": text}))
            }
        }

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let defs = build_tool_definitions(&registry);
        let provider = build_builtin_provider(defs, registry);

        let result = provider
            .call_tool("echo", serde_json::json!({"text": "hello"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            clawft_services::mcp::ContentBlock::Text { text } => {
                // The output is JSON-serialized from the tool result.
                let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
                assert_eq!(parsed["output"], "hello");
            }
        }
    }

    // ── PermissionFilter wiring (WEFT-480 / WEFT-699) ──────────────
    //
    // Empty `tools.allowed_tools` is no longer a "full dump" for the
    // serve path: the profile already limited `tool_defs`. Empty
    // allowlist means PermissionFilter is permissive *over the
    // profile surface*. Non-empty allowlist further restricts.

    #[tokio::test]
    async fn permission_filter_empty_allowed_tools_is_permissive_over_surface() {
        use clawft_services::mcp::ToolDefinition;
        use clawft_services::mcp::middleware::{Middleware, PermissionFilter};

        let allowed: Vec<String> = vec![];
        // Reproduce the branch in `run()` for the empty case.
        let filter = if allowed.is_empty() {
            PermissionFilter::new(None)
        } else {
            PermissionFilter::from_patterns(allowed)
        };

        let tools = vec![
            ToolDefinition {
                name: "alpha".into(),
                description: "".into(),
                input_schema: serde_json::json!({}),
            },
            ToolDefinition {
                name: "beta".into(),
                description: "".into(),
                input_schema: serde_json::json!({}),
            },
        ];
        let filtered = filter.filter_tools(tools).await;
        assert_eq!(
            filtered.len(),
            2,
            "empty allowlist must pass through the (already profile-filtered) surface"
        );
    }

    #[tokio::test]
    async fn permission_filter_allowed_tools_restricts_surface() {
        use clawft_services::mcp::ToolDefinition;
        use clawft_services::mcp::middleware::{Middleware, PermissionFilter};

        let allowed: Vec<String> = vec!["foo".into(), "bar.*".into()];
        let filter = if allowed.is_empty() {
            PermissionFilter::new(None)
        } else {
            PermissionFilter::from_patterns(allowed)
        };

        let tools = vec![
            ToolDefinition {
                name: "foo".into(),
                description: "".into(),
                input_schema: serde_json::json!({}),
            },
            ToolDefinition {
                name: "bar.list".into(),
                description: "".into(),
                input_schema: serde_json::json!({}),
            },
            ToolDefinition {
                name: "qux".into(),
                description: "".into(),
                input_schema: serde_json::json!({}),
            },
        ];
        let filtered = filter.filter_tools(tools).await;
        let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar.list"));
        assert!(!names.contains(&"qux"), "qux must not pass the filter");
    }

    #[tokio::test]
    async fn builtin_provider_not_found() {
        use clawft_services::mcp::ToolProvider;

        let registry = ToolRegistry::new();
        let defs = build_tool_definitions(&registry);
        let provider = build_builtin_provider(defs, registry);

        let result = provider
            .call_tool("nonexistent", serde_json::json!({}))
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            clawft_services::mcp::ToolError::NotFound(name) => {
                assert_eq!(name, "nonexistent");
            }
            other => panic!("expected NotFound, got: {other}"),
        }
    }
}
