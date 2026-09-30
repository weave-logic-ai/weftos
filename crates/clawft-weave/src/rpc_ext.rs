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
//! There is no transport marker: the TCP relay byte-copies into the unix
//! socket, so a relayed caller is indistinguishable from a local one and
//! no gate may rely on the difference. (The relay checks its bearer
//! before forwarding.)
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

/// Who is calling, as established by the entry path before dispatch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallerCtx {
    /// Bearer / scope token from the request envelope, if any.
    pub auth: Option<String>,
    /// Project the request is scoped to, once the envelope carries one.
    pub project: Option<String>,
}

impl CallerCtx {
    /// A caller presenting `auth` (or none).
    pub fn from_auth(auth: Option<String>) -> Self {
        Self {
            auth,
            project: None,
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
    pub fn internal_voice() -> Self {
        Self::from_auth(Some("read,chat,write".to_owned()))
    }
}

/// Per-request context handed to route handlers.
#[derive(Clone)]
pub struct ExtCtx {
    pub kernel: KernelRef,
    pub auth: Option<String>,
    pub project: Option<String>,
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
    pub method: &'a str,
    pub params: &'a Value,
    pub auth: Option<&'a str>,
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
const ROUTES: &[ExtRoute] = &[];
#[cfg(test)]
const ROUTES: &[ExtRoute] = &[ExtRoute {
    prefix: "rpc_ext.test.",
    capability: Capability::Write,
    handler: test_probe,
}];

/// Registered gates, run in order; the first denial wins.
///
/// Package G adds the D12 scope gate here.
#[cfg(not(test))]
const GATES: &[GateFn] = &[];
#[cfg(test)]
const GATES: &[GateFn] = &[test_deny_gate];

/// Unit-test-only route: proves extension dispatch on the real wire path.
#[cfg(test)]
fn test_probe(call: ExtCall) -> ExtFuture {
    Box::pin(async move {
        Response::success(serde_json::json!({
            "method": call.method,
            "auth": call.ctx.auth,
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
        method,
        params,
        auth: caller.auth.as_deref(),
        project: caller.project.as_deref(),
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
