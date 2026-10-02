//! `shared.*`: the user daemon's heavy services, for its projects
//! (ADR-103 Phase 2 F, user-daemon side).
//!
//! | method | what |
//! |---|---|
//! | `shared.embed` | `{texts:[..]}` to `{embeddings, dimension, model, tokens}`; no texts = just the width |
//! | `shared.llm.chat` | an OpenAI-shape chat request to the daemon's `ChatResponse` |
//! | `shared.llm.models` | `{models:[..]}` |
//!
//! Registered as one `shared.` route in `rpc_ext::ROUTES` (`Write`), and
//! refused on a project kernel itself (a child must not relay for a sibling).
//!
//! **Who is calling.** [`identify`] resolves one project id per call:
//!
//! 1. a validated token scoped to a project: the project is the token's
//!    ([`VerifiedProject::from_token`]); a `Request.project` claim or a
//!    `project_id` param naming another project is `project_scope_mismatch`;
//! 2. otherwise an Admin caller (the local operator, or an unscoped owner
//!    token) names the project in `project_id` / `Request.project`;
//! 3. anything else is `verified_project_required`.
//!
//! Package I will add the other two `VerifiedProject` sources (the child's
//! own socket, a user-signed forward header); the hook is marked in
//! [`identify`]. Honest limit, as for `VerifiedProject`: a same-uid local
//! process holding Admin can name any project here; this isolates one user's
//! projects from each other, not from a hostile local process.
//!
//! **No cross-project context.** The handlers are stateless: a call sees only
//! the messages it carries. There is no conversation id, session, memory or
//! graft lookup, so a project cannot use `shared.llm` to read another
//! project's context; unknown params (`conv_id`, `session`, ...) are refused
//! rather than ignored.
//!
//! **Limits and audit.** Every call passes [`UsageMeter`] (rate, token budget,
//! per-call cap) with limits from the manifest's `[shared]` table, and each
//! served call is appended to the user chain as
//! `shared.use {project_id, service, tokens}`: no prompt, no completion, no
//! text of any kind.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::Instant;

use clawft_kernel::embedding::EmbeddingProvider;
use clawft_kernel::token_authority::SECRET_PREFIX;
use clawft_rpc::Response;
use clawft_service_llm::{ChatMessage, SharedLlmClient, Tool, ToolChoice};
use clawft_types::project::{ProjectState, read_manifest};
use serde_json::{Value, json};

use crate::capability::Capability;
use crate::rpc_ext::{ExtCall, ExtFuture, KernelRef, VerifiedProject};
use crate::shared_meter::{Refusal, SharedLimits, UsageMeter, estimate_tokens};

/// Chain source and kind of the per-call audit record.
pub const USE_SOURCE: &str = "shared.services";
pub const USE_KIND: &str = "shared.use";

const MAX_EMBED_TEXTS: usize = 256;
const MAX_EMBED_TEXT_BYTES: usize = 64 * 1024;
const MAX_CHAT_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_MAX_TOKENS: u64 = 512;
const CHAT_KEYS: &[&str] =
    &["model", "messages", "tools", "tool_choice", "temperature", "max_tokens", "stream", "project_id"];

static METER: OnceLock<UsageMeter> = OnceLock::new();
static EMBEDDER: OnceLock<Arc<dyn EmbeddingProvider>> = OnceLock::new();
static LLM: RwLock<Option<SharedLlmClient>> = RwLock::new(None);

/// Serve `shared.llm.*` from `client` instead of the daemon's own (tests).
pub fn install_llm(client: SharedLlmClient) {
    *LLM.write().unwrap_or_else(|e| e.into_inner()) = Some(client);
}

/// Serve `shared.embed` from `embedder` instead of the daemon's own (tests).
/// Only the first install wins.
pub fn install_embedder(embedder: Arc<dyn EmbeddingProvider>) {
    let _ = EMBEDDER.set(embedder);
}

fn llm_client() -> Option<SharedLlmClient> {
    LLM.read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .or_else(crate::daemon::daemon_llm)
}

async fn embedder() -> Arc<dyn EmbeddingProvider> {
    if let Some(e) = EMBEDDER.get() {
        return Arc::clone(e);
    }
    // The model load can take seconds; keep it off the async threads.
    let built = tokio::task::spawn_blocking(|| {
        Arc::from(clawft_kernel::embedding::select_embedding_provider(None))
    })
    .await
    .expect("embedder selection panicked");
    Arc::clone(EMBEDDER.get_or_init(|| built))
}

fn refuse(kind: &str, message: impl Into<String>) -> Response {
    Response::error_with_kind(kind, message)
}

fn refused(r: &Refusal) -> Response {
    refuse(r.kind(), r.message())
}

/// The one project this call is for.
fn identify(call: &ExtCall, authority: Option<&clawft_kernel::token_authority::TokenAuthority>)
-> Result<String, Response> {
    let auth = call.ctx.auth.as_deref().map(str::trim).unwrap_or("");
    let param = call.params.get("project_id").and_then(Value::as_str);
    let claim = call.ctx.project.as_ref().map(|p| p.as_str());
    if auth.starts_with(SECRET_PREFIX) {
        let info = authority
            .and_then(|a| a.validate(auth))
            .ok_or_else(|| refuse("unauthenticated", "token unknown, expired or revoked"))?;
        if let Some(vp) = VerifiedProject::from_token(&info) {
            if claim.is_some_and(|c| c != vp.as_str()) || param.is_some_and(|p| p != vp.as_str()) {
                return Err(refuse(
                    "project_scope_mismatch",
                    "the request names a project other than the one the token is scoped to",
                ));
            }
            return Ok(vp.as_str().to_owned());
        }
    }
    // HOOK(package I): prefer `CallerCtx.verified_project` here once it exists
    // (the child's own socket, or a user-signed forward header).
    if call.ctx.caps.allows(Capability::Admin) {
        return param
            .or(claim)
            .map(str::to_owned)
            .ok_or_else(|| refuse("bad_params", "an operator call must name project_id"));
    }
    Err(refuse(
        "verified_project_required",
        "shared services need a project-scoped token or an operator",
    ))
}

/// The project's manifest limits; the project must be registered and active.
fn limits_for(project: &str) -> Result<SharedLimits, Response> {
    let dir = crate::scope_gate::manifests_dir()
        .ok_or_else(|| refuse("project_store_unavailable", "no manifest store"))?;
    match read_manifest(&dir, project) {
        Ok(Some(m)) if m.state == ProjectState::Active => Ok(SharedLimits::from_manifest(&m)),
        _ => Err(refuse("project_unknown", "project is not registered and active")),
    }
}

async fn record_use(kernel: &KernelRef, project: &str, service: &str, tokens: u64) {
    let k = kernel.read().await;
    if let Some(chain) = k.chain_manager() {
        // Ids and a count only: never prompt, completion or embedded text.
        chain.append(
            USE_SOURCE,
            USE_KIND,
            Some(json!({ "project_id": project, "service": service, "tokens": tokens })),
        );
    }
}

/// Handler for every `shared.*` route.
pub fn handle(call: ExtCall) -> ExtFuture {
    Box::pin(async move { run(call).await })
}

async fn run(call: ExtCall) -> Response {
    if crate::project_profile::is_project_profile() {
        return refuse("not_a_parent", "a project kernel does not serve shared services");
    }
    let authority = crate::token_rpc::authority_for(&call.ctx.kernel).await;
    let project = match identify(&call, authority.as_deref()) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let limits = match limits_for(&project) {
        Ok(l) => l,
        Err(r) => return r,
    };
    match call.method.as_str() {
        "shared.embed" => embed(&call, &project, &limits).await,
        "shared.llm.chat" => chat(&call, &project, &limits).await,
        "shared.llm.models" => models(&call, &project, &limits).await,
        other => refuse("unknown_method", format!("no such shared method: {other}")),
    }
}

fn admit(project: &str, limits: &SharedLimits, estimated: u64) -> Result<crate::shared_meter::Reservation, Response> {
    METER
        .get_or_init(UsageMeter::default)
        .admit(project, limits, estimated, Instant::now())
        .map_err(|r| refused(&r))
}

fn settle(r: &crate::shared_meter::Reservation, actual: u64) -> u64 {
    METER.get_or_init(UsageMeter::default).settle(r, actual)
}

fn only_keys(params: &Value, allowed: &[&str]) -> Result<(), Response> {
    let Some(obj) = params.as_object() else { return Ok(()) };
    match obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        None => Ok(()),
        Some(k) => Err(refuse(
            "bad_params",
            format!("unsupported param '{k}': shared services take only the messages a call carries"),
        )),
    }
}

async fn embed(call: &ExtCall, project: &str, limits: &SharedLimits) -> Response {
    if let Err(r) = only_keys(&call.params, &["texts", "project_id"]) {
        return r;
    }
    let texts: Vec<String> = match call.params.get("texts").map(|t| serde_json::from_value(t.clone())) {
        Some(Ok(t)) => t,
        _ => return refuse("bad_params", "texts must be an array of strings"),
    };
    if texts.len() > MAX_EMBED_TEXTS || texts.iter().any(|t| t.len() > MAX_EMBED_TEXT_BYTES) {
        return refuse(
            "request_too_large",
            format!("at most {MAX_EMBED_TEXTS} texts of {MAX_EMBED_TEXT_BYTES} bytes each"),
        );
    }
    let estimated: u64 = texts.iter().map(|t| estimate_tokens(t)).sum();
    let reservation = match admit(project, limits, estimated) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let provider = embedder().await;
    let rows = if texts.is_empty() {
        Ok(Vec::new())
    } else {
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        provider.embed_batch(&refs).await
    };
    match rows {
        Ok(rows) => {
            let tokens = settle(&reservation, estimated);
            record_use(&call.ctx.kernel, project, "embeddings", tokens).await;
            Response::success(json!({
                "embeddings": rows,
                "dimension": provider.dimensions(),
                "model": provider.model_name(),
                "tokens": tokens,
            }))
        }
        Err(e) => {
            settle(&reservation, 0);
            refuse("upstream_error", format!("embedding failed: {e}"))
        }
    }
}

fn parse_tool_choice(v: &Value) -> Option<ToolChoice> {
    match v {
        Value::String(s) => match s.as_str() {
            "auto" => Some(ToolChoice::Auto),
            "none" => Some(ToolChoice::None),
            "required" => Some(ToolChoice::Required),
            _ => None,
        },
        Value::Object(_) => v
            .pointer("/function/name")
            .and_then(Value::as_str)
            .map(|n| ToolChoice::Function(n.to_owned())),
        _ => None,
    }
}

fn prompt_tokens(messages: &[ChatMessage], tools: &[Tool]) -> u64 {
    let msgs: u64 = messages
        .iter()
        .map(|m| {
            let calls: u64 = m
                .tool_calls
                .iter()
                .flatten()
                .map(|c| estimate_tokens(&c.function.name) + estimate_tokens(&c.function.arguments))
                .sum();
            estimate_tokens(&m.content.as_text()) + calls + 4
        })
        .sum();
    let schemas: u64 = tools
        .iter()
        .map(|t| estimate_tokens(&serde_json::to_string(t).unwrap_or_default()))
        .sum();
    msgs + schemas
}

async fn chat(call: &ExtCall, project: &str, limits: &SharedLimits) -> Response {
    let p = &call.params;
    if let Err(r) = only_keys(p, CHAT_KEYS) {
        return r;
    }
    if p.get("stream").and_then(Value::as_bool) == Some(true) {
        return refuse("bad_params", "shared.llm.chat does not stream");
    }
    if p.to_string().len() > MAX_CHAT_BYTES {
        return refuse("request_too_large", "chat request too large");
    }
    let messages: Vec<ChatMessage> = match p.get("messages").map(|m| serde_json::from_value(m.clone())) {
        Some(Ok(m)) if !Vec::<ChatMessage>::is_empty(&m) => m,
        _ => return refuse("bad_params", "messages must be a non-empty array of chat messages"),
    };
    let tools: Vec<Tool> = match p.get("tools").filter(|t| !t.is_null()) {
        None => Vec::new(),
        Some(t) => match serde_json::from_value(t.clone()) {
            Ok(t) => t,
            Err(_) => return refuse("bad_params", "tools must be an array of tool definitions"),
        },
    };
    let tool_choice = p.get("tool_choice").filter(|t| !t.is_null()).and_then(parse_tool_choice);
    let temperature = p.get("temperature").and_then(Value::as_f64).map(|t| t as f32);
    let max_tokens = p.get("max_tokens").and_then(Value::as_u64).map(|n| n.min(u32::MAX as u64) as u32);
    let model = p.get("model").and_then(Value::as_str).map(str::to_owned);

    let Some(client) = llm_client() else {
        return refuse("service_unavailable", "the user daemon has no llm service");
    };
    let prompt = prompt_tokens(&messages, &tools);
    let estimated = prompt + max_tokens.map(u64::from).unwrap_or(DEFAULT_MAX_TOKENS);
    let reservation = match admit(project, limits, estimated) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let result = client
        .read()
        .await
        .complete_with_tools(messages, tools, tool_choice, temperature, max_tokens, model.as_deref())
        .await;
    match result {
        Ok(resp) => {
            let actual = if resp.usage.total_tokens > 0 {
                u64::from(resp.usage.total_tokens)
            } else {
                let out: u64 = resp.choices.iter().map(|c| estimate_tokens(&c.message.content.as_text())).sum();
                prompt + out
            };
            let tokens = settle(&reservation, actual);
            record_use(&call.ctx.kernel, project, "llm", tokens).await;
            match serde_json::to_value(&resp) {
                Ok(v) => Response::success(v),
                Err(e) => refuse("upstream_error", format!("reply not serialisable: {e}")),
            }
        }
        Err(e) => {
            settle(&reservation, 0);
            refuse("upstream_error", format!("llm: {e}"))
        }
    }
}

async fn models(call: &ExtCall, project: &str, limits: &SharedLimits) -> Response {
    if let Err(r) = only_keys(&call.params, &["project_id"]) {
        return r;
    }
    let Some(client) = llm_client() else {
        return refuse("service_unavailable", "the user daemon has no llm service");
    };
    let reservation = match admit(project, limits, 0) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let listed = client.read().await.list_models().await;
    let tokens = settle(&reservation, 0);
    record_use(&call.ctx.kernel, project, "llm.models", tokens).await;
    match listed {
        Ok(models) => Response::success(json!({ "models": models })),
        Err(e) => refuse("upstream_error", format!("llm models: {e}")),
    }
}
