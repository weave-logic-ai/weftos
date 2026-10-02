//! Daemon RPC extension seam (ADR-103 Phase 1, package D0).
//!
//! `daemon.rs` has one big `match` in `dispatch` and one choke point in
//! `dispatch_json_line`. Every package that adds RPC surface would collide
//! there, so new methods register here instead:
//!
//! * **Routes** ([`ExtRoute`]): a method prefix (`"project."`,
//!   `"auth.token."`) or an exact name (`"kernel.handshake"`) mapped to an
//!   async handler **and an explicit required [`Capability`]**. The
//!   capability table in `capability.rs` defaults unlisted methods to
//!   `Read` (anonymous-callable), so a route must state what it needs;
//!   [`authorize`] enforces it. A `None` from [`dispatch_ext`] means "not
//!   mine" and the legacy `match` runs.
//! * **Gates** ([`GateFn`]): hooks that see every authorized-so-far request
//!   and may deny it with a structured error (`error_kind` set). They run
//!   *after* capability resolution and receive the resolved capabilities,
//!   so a gate never sees a request from a caller who failed the
//!   capability check (no pre-auth existence leaks). Package G's D12 scope
//!   gate lands here.
//!
//! Every entry path (JSON lines, RVF frames, the in-process voice
//! consumer) builds a [`CallerCtx`] and goes through [`authorize`] in
//! `daemon.rs`, so there is one place where authorization happens.
//!
//! Ext routes cannot override the streaming intercepts in
//! `dispatch_json_line` (`ipc.subscribe_stream`, `substrate.subscribe`,
//! `kernel.logs_stream`): those take over the connection and are matched
//! before routes are consulted. They are still authorized and gated.
//!
//! A route whose prefix covers an intercepted method would change that
//! method's required capability while its handler never runs, so routes
//! must not cover them (a test asserts this for `ROUTES`).
//!
//! There is no transport marker: the TCP relay forwards into the unix
//! socket, so a relayed caller looks like a local one and no gate may rely
//! on the difference. Instead the relay (`relay_auth`) strips self-asserted
//! literal scope strings from relayed requests, and the daemon honours
//! literal scopes only from a unix peer with its own uid
//! ([`CallerCtx::peer_untrusted`]). Token secrets work on any path.
//!
//! Registration is explicit and stateless: the `ROUTES` and `GATES` const
//! tables below, one line per package. There is no global mutable
//! registry; handlers reach shared state through [`ExtCtx::kernel`].
//! Tests build their own [`ExtRegistry`] and use the `*_with` entry points.

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use clawft_rpc::Response;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::capability::{CallerCapabilities, Capability, required_capability};

/// Shared kernel handle, same shape `daemon.rs` passes around.
pub type KernelRef = Arc<RwLock<Kernel<NativePlatform>>>;

/// A project id the client CLAIMED in `Request.project`.
///
/// UNVERIFIED: the daemon only checks that it is a well-formed ULID and
/// that it equals the daemon's bound project (when it has one). Holding a
/// `ClaimedProject` proves nothing about membership; package G must verify
/// it against the project registry before any scope decision relies on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedProject(String);

impl ClaimedProject {
    /// The claimed id, unverified.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ClaimedProject {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ClaimedProject {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

pub use crate::verified_project::VerifiedProject;

/// The kind of principal behind a request, set by the entry path (never by
/// the client). Gates use it for per-principal deny-lists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Principal {
    /// A socket / RVF caller.
    #[default]
    External,
    /// The daemon's in-process voice consumer ([`CallerCtx::internal_voice`]).
    InternalVoice,
}

/// Who is calling, as established by the entry path before dispatch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallerCtx {
    /// Set by the entry path; see [`Principal`].
    pub principal: Principal,
    /// Bearer / scope token from the request envelope, if any.
    pub auth: Option<String>,
    /// Project the client claims the request is scoped to
    /// (`Request.project`). Unverified; see [`ClaimedProject`].
    pub project: Option<ClaimedProject>,
    /// The connection's peer is NOT the daemon's own uid (unix socket
    /// peer credentials). Such a caller cannot use literal scope strings
    /// (`"admin"`, ...) as auth; only a token secret counts. `false` for
    /// in-process callers and for the default.
    pub peer_untrusted: bool,
}

impl CallerCtx {
    /// A caller presenting `auth` (or none).
    pub fn from_auth(auth: Option<String>) -> Self {
        Self {
            principal: Principal::External,
            auth,
            project: None,
            peer_untrusted: false,
        }
    }

    /// Mark the connection peer as not the daemon's uid.
    pub fn with_peer_untrusted(mut self, untrusted: bool) -> Self {
        self.peer_untrusted = untrusted;
        self
    }

    /// Caller context for a wire request: its `auth` and `project`.
    pub fn from_request(req: &clawft_rpc::Request) -> Self {
        Self {
            principal: Principal::External,
            auth: req.auth.clone(),
            project: req.project.clone().map(ClaimedProject::from),
            peer_untrusted: false,
        }
    }

    /// The daemon's in-process voice consumer.
    ///
    /// Decision: `read,chat,write`, never `admin`. Voice Level 2 is an
    /// operator opt-in to route arbitrary spoken verbs, so mutating verbs
    /// must work, but daemon-lifecycle and trust-root verbs
    /// (`kernel.shutdown`, `kernel.kill-process`, `cluster.*`,
    /// `workload.revoke`, `chain.checkpoint`) stay out of reach of speech.
    /// Before this the voice path dispatched with no check at all.
    ///
    /// The scope gate additionally denies this principal the cron
    /// mutations (`cron.*` writes), see `scope_gate::VOICE_DENIED`.
    pub fn internal_voice() -> Self {
        Self {
            principal: Principal::InternalVoice,
            ..Self::from_auth(Some("read,chat,write".to_owned()))
        }
    }
}

/// Per-request context handed to route handlers.
#[derive(Clone)]
pub struct ExtCtx {
    pub kernel: KernelRef,
    pub auth: Option<String>,
    /// Unverified client claim; see [`ClaimedProject`].
    pub project: Option<ClaimedProject>,
    /// Capabilities already resolved for this caller.
    pub caps: CallerCapabilities,
}

/// One dispatched call: the method that matched plus its parameters.
pub struct ExtCall {
    pub method: String,
    pub params: Value,
    pub ctx: ExtCtx,
}

pub type ExtFuture = Pin<Box<dyn Future<Output = Response> + Send>>;
pub type ExtHandler = fn(ExtCall) -> ExtFuture;

/// A method-prefix (or exact-name) handler registration.
#[derive(Clone, Copy)]
pub struct ExtRoute {
    /// A prefix ending in `.` matches every method under it; any other
    /// string matches that exact method only.
    pub prefix: &'static str,
    /// Capability a caller must hold. Explicit on purpose: unlisted
    /// methods default to anonymous-callable `Read` in `capability.rs`.
    pub capability: Capability,
    pub handler: ExtHandler,
}

impl ExtRoute {
    fn matches(&self, method: &str) -> bool {
        if self.prefix.ends_with('.') {
            method.starts_with(self.prefix)
        } else {
            method == self.prefix
        }
    }
}

/// What a gate sees. Borrowed so running gates never clones params.
pub struct GateRequest<'a> {
    /// Who is calling (set by the entry path, not the client).
    pub principal: Principal,
    pub method: &'a str,
    pub params: &'a Value,
    pub auth: Option<&'a str>,
    /// Unverified client claim (see [`ClaimedProject`]).
    pub project: Option<&'a str>,
    /// Capabilities resolved for the caller (the capability check has
    /// already passed for `method`).
    pub caps: &'a CallerCapabilities,
    pub kernel: &'a KernelRef,
}

/// A gate's refusal, surfaced as `Response::error_with_kind(kind, message)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    /// snake_case discriminator for clients (e.g. `"scope_denied"`).
    pub kind: String,
    pub message: String,
}

impl Denial {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }
}

pub type GateFuture<'a> = Pin<Box<dyn Future<Output = Result<(), Denial>> + Send + 'a>>;
pub type GateFn = for<'a> fn(&'a GateRequest<'a>) -> GateFuture<'a>;

/// Registered routes, in match order (first match wins).
///
/// Each later package adds one line here plus its own module, e.g.
/// `ExtRoute { prefix: "project.", capability: Capability::Read,
/// handler: crate::project_rpc::handle }`.
#[cfg(not(test))]
const ROUTES: &[ExtRoute] = &[
    ExtRoute {
        prefix: "kernel.handshake",
        capability: Capability::Read,
        handler: crate::handshake_rpc::handle,
    },
    ExtRoute {
        prefix: "project.list",
        capability: Capability::Read,
        handler: crate::project_rpc::handle_list,
    },
    ExtRoute {
        prefix: "project.show",
        capability: Capability::Read,
        handler: crate::project_rpc::handle_show,
    },
    ExtRoute {
        prefix: "project.register",
        capability: Capability::Admin,
        handler: crate::project_rpc::handle_register,
    },
    ExtRoute {
        prefix: "auth.token.issue",
        capability: Capability::Admin,
        handler: crate::token_rpc::handle,
    },
    ExtRoute {
        prefix: "auth.token.revoke",
        capability: Capability::Admin,
        handler: crate::token_rpc::handle,
    },
    ExtRoute {
        prefix: "auth.token.list",
        capability: Capability::Admin,
        handler: crate::token_rpc::handle,
    },
    // Read: the gateway and any anonymous caller may ask "is this secret
    // valid?"; a 256-bit secret cannot be searched for.
    ExtRoute {
        prefix: "auth.token.validate",
        capability: Capability::Read,
        handler: crate::token_rpc::handle,
    },
    // One prefix for shared.embed / shared.llm.chat / shared.llm.models: the
    // handler resolves the project itself (token scope or Admin) and refuses
    // anonymous callers; `Write` is the floor.
    ExtRoute {
        prefix: "shared.",
        capability: Capability::Write,
        handler: crate::shared_rpc::handle,
    },
];
#[cfg(test)]
const ROUTES: &[ExtRoute] = &[
    ExtRoute {
        prefix: "kernel.handshake",
        capability: Capability::Read,
        handler: crate::handshake_rpc::handle,
    },
    ExtRoute {
        prefix: "project.list",
        capability: Capability::Read,
        handler: crate::project_rpc::handle_list,
    },
    ExtRoute {
        prefix: "project.show",
        capability: Capability::Read,
        handler: crate::project_rpc::handle_show,
    },
    ExtRoute {
        prefix: "project.register",
        capability: Capability::Admin,
        handler: crate::project_rpc::handle_register,
    },
    ExtRoute {
        prefix: "auth.token.issue",
        capability: Capability::Admin,
        handler: crate::token_rpc::handle,
    },
    ExtRoute {
        prefix: "auth.token.revoke",
        capability: Capability::Admin,
        handler: crate::token_rpc::handle,
    },
    ExtRoute {
        prefix: "auth.token.list",
        capability: Capability::Admin,
        handler: crate::token_rpc::handle,
    },
    // Read: the gateway and any anonymous caller may ask "is this secret
    // valid?"; a 256-bit secret cannot be searched for.
    ExtRoute {
        prefix: "auth.token.validate",
        capability: Capability::Read,
        handler: crate::token_rpc::handle,
    },
    // One prefix for shared.embed / shared.llm.chat / shared.llm.models: the
    // handler resolves the project itself (token scope or Admin) and refuses
    // anonymous callers; `Write` is the floor.
    ExtRoute {
        prefix: "shared.",
        capability: Capability::Write,
        handler: crate::shared_rpc::handle,
    },
    ExtRoute {
        prefix: "rpc_ext.test.",
        capability: Capability::Write,
        handler: test_probe,
    },
];

/// Prefix or exact name and required capability of each built-in route
/// (scope-gate population test).
#[cfg(test)]
pub(crate) fn builtin_route_names() -> Vec<(&'static str, Capability)> {
    ROUTES.iter().map(|r| (r.prefix, r.capability)).collect()
}

/// Registered gates, run in order; the first denial wins.
///
/// The D12 scope gates (package G): the voice deny-list, then the
/// outside-project policy.
#[cfg(not(test))]
const GATES: &[GateFn] = &[
    crate::scope_gate::voice_gate,
    crate::scope_gate::scope_gate,
];
#[cfg(test)]
const GATES: &[GateFn] = &[
    test_deny_gate,
    crate::scope_gate::voice_gate,
    crate::scope_gate::scope_gate,
];

/// Unit-test-only route: proves extension dispatch on the real wire path.
#[cfg(test)]
fn test_probe(call: ExtCall) -> ExtFuture {
    Box::pin(async move {
        Response::success(serde_json::json!({
            "method": call.method,
            "auth": call.ctx.auth,
            "project": call.ctx.project.as_ref().map(ClaimedProject::as_str),
        }))
    })
}

/// Unit-test-only gate: denies `rpc_ext.gated.*` on the real wire path.
#[cfg(test)]
fn test_deny_gate<'a>(req: &'a GateRequest<'a>) -> GateFuture<'a> {
    Box::pin(async move {
        if req.method.starts_with("rpc_ext.gated.") {
            Err(Denial::new("scope_denied", "test gate denies rpc_ext.gated.*"))
        } else {
            Ok(())
        }
    })
}

/// A set of routes and gates. Production uses [`ExtRegistry::builtin`]
/// (zero-cost: borrows the const tables); tests compose their own.
#[derive(Clone)]
pub struct ExtRegistry {
    routes: Cow<'static, [ExtRoute]>,
    gates: Cow<'static, [GateFn]>,
}

impl Default for ExtRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtRegistry {
    pub fn new() -> Self {
        Self {
            routes: Cow::Owned(Vec::new()),
            gates: Cow::Owned(Vec::new()),
        }
    }

    /// The registry built from the `ROUTES` / `GATES` tables.
    pub fn builtin() -> Self {
        Self {
            routes: Cow::Borrowed(ROUTES),
            gates: Cow::Borrowed(GATES),
        }
    }

    pub fn route(
        mut self,
        prefix: &'static str,
        capability: Capability,
        handler: ExtHandler,
    ) -> Self {
        self.routes.to_mut().push(ExtRoute {
            prefix,
            capability,
            handler,
        });
        self
    }

    pub fn gate(mut self, gate: GateFn) -> Self {
        self.gates.to_mut().push(gate);
        self
    }

    fn find(&self, method: &str) -> Option<&ExtRoute> {
        self.routes.iter().find(|r| r.matches(method))
    }

    /// Capability `method` requires: the route's explicit one, else the
    /// legacy table.
    fn required(&self, method: &str) -> Capability {
        self.find(method)
            .map(|r| r.capability)
            .unwrap_or_else(|| required_capability(method))
    }
}

/// Authorize a request: capability check, then gates. `Err` carries the
/// refusal response (without request id; the caller attaches it).
///
/// Every entry path calls this once per request before any dispatch.
pub async fn authorize_with(
    registry: &ExtRegistry,
    caller: &CallerCtx,
    caps: &CallerCapabilities,
    method: &str,
    params: &Value,
    kernel: &KernelRef,
) -> Result<(), Response> {
    let required = registry.required(method);
    if !caps.allows(required) {
        tracing::warn!(
            method = %method,
            required = ?required,
            "rpc capability check failed; rejecting"
        );
        return Err(Response::error(format!(
            "permission denied: method '{method}' requires capability {required:?}"
        )));
    }
    let req = GateRequest {
        principal: caller.principal,
        method,
        params,
        auth: caller.auth.as_deref(),
        project: caller.project.as_ref().map(ClaimedProject::as_str),
        caps,
        kernel,
    };
    for gate in registry.gates.iter() {
        if let Err(d) = gate(&req).await {
            return Err(Response::error_with_kind(d.kind, d.message));
        }
    }
    Ok(())
}

/// [`authorize_with`] over the built-in registry.
pub async fn authorize(
    caller: &CallerCtx,
    caps: &CallerCapabilities,
    method: &str,
    params: &Value,
    kernel: &KernelRef,
) -> Result<(), Response> {
    authorize_with(&ExtRegistry::builtin(), caller, caps, method, params, kernel).await
}

/// Run `registry`'s handler for `method`, or `None` if no route matches.
/// Call only after [`authorize_with`] succeeded.
pub async fn dispatch_ext_with(
    registry: &ExtRegistry,
    caller: &CallerCtx,
    caps: &CallerCapabilities,
    method: &str,
    params: &Value,
    kernel: &KernelRef,
) -> Option<Response> {
    let route = registry.find(method)?;
    Some(
        (route.handler)(ExtCall {
            method: method.to_owned(),
            params: params.clone(),
            ctx: ExtCtx {
                kernel: Arc::clone(kernel),
                auth: caller.auth.clone(),
                project: caller.project.clone(),
                caps: caps.clone(),
            },
        })
        .await,
    )
}

/// [`dispatch_ext_with`] over the built-in registry; `None` means the
/// legacy `match` should handle `method`.
pub async fn dispatch_ext(
    caller: &CallerCtx,
    caps: &CallerCapabilities,
    method: &str,
    params: &Value,
    kernel: &KernelRef,
) -> Option<Response> {
    dispatch_ext_with(&ExtRegistry::builtin(), caller, caps, method, params, kernel).await
}

#[cfg(test)]
#[path = "rpc_ext_tests.rs"]
mod tests;
