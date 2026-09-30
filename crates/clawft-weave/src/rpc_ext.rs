//! Daemon RPC extension seam (ADR-103 Phase 1, package D0).
//!
//! `daemon.rs` has one big `match` in `dispatch` and one choke point in
//! `dispatch_json_line`. Every package that adds RPC surface would collide
//! there, so new methods register here instead:
//!
//! * **Routes** ([`ExtRoute`]): a method prefix (`"project."`,
//!   `"auth.token."`) or an exact name (`"kernel.handshake"`) mapped to an
//!   async handler. `dispatch` consults [`dispatch_ext`] before its legacy
//!   `match`; a `None` result means "not mine" and the legacy match runs.
//! * **Gates** ([`GateFn`]): pre-dispatch hooks that see every JSON request
//!   before the WEFT-479 capability check and may deny it with a structured
//!   error (`error_kind` set). Package G's D12 scope gate lands here.
//!
//! Registration is explicit and stateless: the [`ROUTES`] and [`GATES`]
//! const tables below, one line per package. There is no global mutable
//! registry; handlers reach shared state through [`ExtCtx::kernel`]. Tests
//! build their own [`ExtRegistry`] and use the `*_with` entry points.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use clawft_rpc::Response;
use serde_json::Value;
use tokio::sync::RwLock;

/// Shared kernel handle, same shape `daemon.rs` passes around.
pub type KernelRef = Arc<RwLock<Kernel<NativePlatform>>>;

/// How the request reached the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Unix socket / named pipe.
    Local,
    /// Authenticated TCP listener.
    Tcp,
}

/// Per-request context handed to route handlers.
#[derive(Clone)]
pub struct ExtCtx {
    pub kernel: KernelRef,
    pub transport: Transport,
    /// Project the request is scoped to, once the envelope carries one.
    pub project: Option<String>,
    /// Bearer token presented on the request, if any.
    pub auth: Option<String>,
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
    pub transport: Transport,
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
/// `ExtRoute { prefix: "project.", handler: crate::project_rpc::handle }`.
const ROUTES: &[ExtRoute] = &[];

/// Registered gates, run in order; the first denial wins.
///
/// Package G adds the D12 scope gate here.
const GATES: &[GateFn] = &[];

/// A set of routes and gates. Production uses [`ExtRegistry::builtin`];
/// tests compose their own.
#[derive(Clone, Default)]
pub struct ExtRegistry {
    routes: Vec<ExtRoute>,
    gates: Vec<GateFn>,
}

impl ExtRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The registry built from the [`ROUTES`] / [`GATES`] tables.
    pub fn builtin() -> Self {
        Self {
            routes: ROUTES.to_vec(),
            gates: GATES.to_vec(),
        }
    }

    pub fn route(mut self, prefix: &'static str, handler: ExtHandler) -> Self {
        self.routes.push(ExtRoute { prefix, handler });
        self
    }

    pub fn gate(mut self, gate: GateFn) -> Self {
        self.gates.push(gate);
        self
    }

    fn find(&self, method: &str) -> Option<ExtHandler> {
        self.routes
            .iter()
            .find(|r| r.matches(method))
            .map(|r| r.handler)
    }
}

/// Run `registry`'s handler for `method`, or `None` if no route matches.
pub async fn dispatch_ext_with(
    registry: &ExtRegistry,
    method: &str,
    params: &Value,
    kernel: &KernelRef,
) -> Option<Response> {
    let handler = registry.find(method)?;
    Some(
        handler(ExtCall {
            method: method.to_owned(),
            params: params.clone(),
            ctx: ExtCtx {
                kernel: Arc::clone(kernel),
                transport: Transport::Local,
                project: None,
                auth: None,
            },
        })
        .await,
    )
}

/// Production entry point used by `daemon::dispatch`: `None` means the
/// legacy `match` should handle `method`.
pub async fn dispatch_ext(method: &str, params: &Value, kernel: &KernelRef) -> Option<Response> {
    dispatch_ext_with(&ExtRegistry::builtin(), method, params, kernel).await
}

/// Run `registry`'s gates over a request; `Some(response)` is a denial
/// (without request id; the caller attaches it).
pub async fn check_gates_with(registry: &ExtRegistry, req: &GateRequest<'_>) -> Option<Response> {
    for gate in &registry.gates {
        if let Err(d) = gate(req).await {
            return Some(Response::error_with_kind(d.kind, d.message));
        }
    }
    None
}

/// Production entry point used by `daemon::dispatch_json_line`.
pub async fn check_gates(req: &GateRequest<'_>) -> Option<Response> {
    check_gates_with(&ExtRegistry::builtin(), req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    async fn test_kernel() -> KernelRef {
        use clawft_types::config::{ChainConfig, Config, KernelConfig};
        let kcfg = KernelConfig {
            chain: Some(ChainConfig::isolated_in(&tempfile::tempdir().unwrap().keep())),
            ..KernelConfig::default()
        };
        let k = Kernel::boot(Config::default(), kcfg, Arc::new(NativePlatform::new()))
            .await
            .expect("kernel boots");
        Arc::new(RwLock::new(k))
    }

    fn echo(call: ExtCall) -> ExtFuture {
        Box::pin(async move {
            Response::success(serde_json::json!({
                "method": call.method,
                "params": call.params,
            }))
        })
    }

    #[tokio::test]
    async fn prefix_handler_receives_its_method_and_params() {
        let kernel = test_kernel().await;
        let reg = ExtRegistry::new().route("project.", echo);
        let params = serde_json::json!({"k": 1});
        let r = dispatch_ext_with(&reg, "project.list", &params, &kernel)
            .await
            .expect("routed");
        let v = r.result.unwrap();
        assert_eq!(v["method"], "project.list");
        assert_eq!(v["params"], params);
    }

    #[tokio::test]
    async fn exact_route_does_not_match_lookalikes() {
        let kernel = test_kernel().await;
        let reg = ExtRegistry::new().route("kernel.handshake", echo);
        let null = Value::Null;
        assert!(dispatch_ext_with(&reg, "kernel.handshake", &null, &kernel).await.is_some());
        assert!(dispatch_ext_with(&reg, "kernel.handshake2", &null, &kernel).await.is_none());
        assert!(dispatch_ext_with(&reg, "kernel.status", &null, &kernel).await.is_none());
    }

    #[tokio::test]
    async fn unknown_method_falls_through_to_legacy() {
        let kernel = test_kernel().await;
        let reg = ExtRegistry::new().route("project.", echo);
        let null = Value::Null;
        assert!(dispatch_ext_with(&reg, "kernel.status", &null, &kernel).await.is_none());
        // The shipped registry is empty until packages register: every
        // legacy method must fall through untouched.
        assert!(dispatch_ext("kernel.status", &null, &kernel).await.is_none());
    }

    fn deny_secret<'a>(req: &'a GateRequest<'a>) -> GateFuture<'a> {
        Box::pin(async move {
            if req.method == "secret.read" {
                Err(Denial::new("scope_denied", "no scope for secret.read"))
            } else {
                Ok(())
            }
        })
    }

    #[tokio::test]
    async fn gate_can_deny_with_structured_error() {
        let kernel = test_kernel().await;
        let reg = ExtRegistry::new().gate(deny_secret);
        let params = Value::Null;
        let mk = |m| GateRequest {
            method: m,
            params: &params,
            auth: None,
            project: None,
            transport: Transport::Local,
            kernel: &kernel,
        };
        let denied = check_gates_with(&reg, &mk("secret.read")).await.expect("denied");
        assert!(!denied.ok);
        assert_eq!(denied.error_kind.as_deref(), Some("scope_denied"));
        assert!(denied.error.unwrap().contains("secret.read"));
        assert!(check_gates_with(&reg, &mk("kernel.status")).await.is_none());
    }

    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn record<'a>(req: &'a GateRequest<'a>) -> GateFuture<'a> {
        Box::pin(async move {
            SEEN.lock().unwrap().push(req.method.to_owned());
            Ok(())
        })
    }

    static AFTER_DENY_RAN: AtomicBool = AtomicBool::new(false);

    fn record_after_deny<'a>(_: &'a GateRequest<'a>) -> GateFuture<'a> {
        Box::pin(async move {
            AFTER_DENY_RAN.store(true, Ordering::SeqCst);
            Ok(())
        })
    }

    /// Population, not just "it ran": every method in a representative
    /// set (legacy, extension-owned, unknown) must reach the gate.
    #[tokio::test]
    async fn gate_sees_every_request() {
        let kernel = test_kernel().await;
        let reg = ExtRegistry::new().route("project.", echo).gate(record);
        let methods = [
            "kernel.status",
            "ipc.subscribe_stream",
            "substrate.subscribe",
            "project.list",
            "app.list",
            "workload.place",
            "no.such.method",
            "",
        ];
        let params = Value::Null;
        for m in methods {
            let req = GateRequest {
                method: m,
                params: &params,
                auth: Some("tok"),
                project: None,
                transport: Transport::Tcp,
                kernel: &kernel,
            };
            assert!(check_gates_with(&reg, &req).await.is_none());
        }
        assert_eq!(*SEEN.lock().unwrap(), methods.map(String::from).to_vec());
    }

    #[tokio::test]
    async fn first_denial_short_circuits_later_gates() {
        let kernel = test_kernel().await;
        let reg = ExtRegistry::new().gate(deny_secret).gate(record_after_deny);
        let params = Value::Null;
        let req = GateRequest {
            method: "secret.read",
            params: &params,
            auth: None,
            project: None,
            transport: Transport::Local,
            kernel: &kernel,
        };
        assert!(check_gates_with(&reg, &req).await.is_some());
        assert!(!AFTER_DENY_RAN.load(Ordering::SeqCst));
    }
}
