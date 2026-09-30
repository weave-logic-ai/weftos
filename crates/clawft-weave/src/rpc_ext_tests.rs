//! Tests for [`crate::rpc_ext`].

use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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
            "auth": call.ctx.auth,
            "project": call.ctx.project,
        }))
    })
}

fn anon() -> CallerCtx {
    CallerCtx::default()
}

#[tokio::test]
async fn prefix_handler_receives_method_params_and_caller() {
    let kernel = test_kernel().await;
    let reg = ExtRegistry::new().route("project.", Capability::Read, echo);
    let params = serde_json::json!({"k": 1});
    let caller = CallerCtx {
        auth: Some("write".into()),
        project: Some("p1".into()),
    };
    let caps = CallerCapabilities::from_scopes(["write"]);
    let r = dispatch_ext_with(&reg, &caller, &caps, "project.list", &params, &kernel)
        .await
        .expect("routed");
    let v = r.result.unwrap();
    assert_eq!(v["method"], "project.list");
    assert_eq!(v["params"], params);
    assert_eq!(v["auth"], "write");
    assert_eq!(v["project"], "p1");
}

#[tokio::test]
async fn exact_route_does_not_match_lookalikes() {
    let kernel = test_kernel().await;
    let reg = ExtRegistry::new().route("kernel.handshake", Capability::Read, echo);
    let (c, caps, null) = (anon(), CallerCapabilities::anonymous(), Value::Null);
    assert!(dispatch_ext_with(&reg, &c, &caps, "kernel.handshake", &null, &kernel).await.is_some());
    assert!(dispatch_ext_with(&reg, &c, &caps, "kernel.handshake2", &null, &kernel).await.is_none());
    assert!(dispatch_ext_with(&reg, &c, &caps, "kernel.status", &null, &kernel).await.is_none());
}

#[tokio::test]
async fn unknown_method_falls_through_to_legacy() {
    let kernel = test_kernel().await;
    let reg = ExtRegistry::new().route("project.", Capability::Read, echo);
    let (c, caps, null) = (anon(), CallerCapabilities::anonymous(), Value::Null);
    assert!(dispatch_ext_with(&reg, &c, &caps, "kernel.status", &null, &kernel).await.is_none());
    assert!(dispatch_ext(&c, &caps, "kernel.status", &null, &kernel).await.is_none());
}

#[tokio::test]
async fn route_capability_is_enforced_not_defaulted_to_read() {
    let kernel = test_kernel().await;
    let reg = ExtRegistry::new().route("project.", Capability::Write, echo);
    let null = Value::Null;
    let anon_caps = CallerCapabilities::anonymous();
    let denied = authorize_with(&reg, &anon(), &anon_caps, "project.create", &null, &kernel)
        .await
        .unwrap_err();
    assert!(denied.error.unwrap().contains("requires capability Write"));
    let w = CallerCapabilities::from_scopes(["write"]);
    assert!(authorize_with(&reg, &anon(), &w, "project.create", &null, &kernel).await.is_ok());
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
    let (caps, null) = (CallerCapabilities::anonymous(), Value::Null);
    let denied = authorize_with(&reg, &anon(), &caps, "secret.read", &null, &kernel)
        .await
        .unwrap_err();
    assert!(!denied.ok);
    assert_eq!(denied.error_kind.as_deref(), Some("scope_denied"));
    assert!(denied.error.unwrap().contains("secret.read"));
    assert!(authorize_with(&reg, &anon(), &caps, "kernel.status", &null, &kernel).await.is_ok());
}

static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record<'a>(req: &'a GateRequest<'a>) -> GateFuture<'a> {
    Box::pin(async move {
        SEEN.lock().unwrap().push(req.method.to_owned());
        Ok(())
    })
}

/// Every method that passes the capability check reaches the gate,
/// whether legacy, extension-owned or unknown.
#[tokio::test]
async fn gate_sees_every_authorized_request() {
    let kernel = test_kernel().await;
    let reg = ExtRegistry::new()
        .route("project.", Capability::Read, echo)
        .gate(record);
    let methods = [
        "kernel.status",
        "ipc.subscribe_stream",
        "substrate.subscribe",
        "project.list",
        "app.list",
        "no.such.method",
        "",
    ];
    let (caps, null) = (CallerCapabilities::anonymous(), Value::Null);
    for m in methods {
        assert!(authorize_with(&reg, &anon(), &caps, m, &null, &kernel).await.is_ok());
    }
    assert_eq!(*SEEN.lock().unwrap(), methods.map(String::from).to_vec());
}

static AFTER_DENY_RAN: AtomicBool = AtomicBool::new(false);
static GATE_RAN_ON_CAP_DENY: AtomicBool = AtomicBool::new(false);

fn record_after_deny<'a>(_: &'a GateRequest<'a>) -> GateFuture<'a> {
    Box::pin(async move {
        AFTER_DENY_RAN.store(true, Ordering::SeqCst);
        Ok(())
    })
}

fn record_cap_deny<'a>(_: &'a GateRequest<'a>) -> GateFuture<'a> {
    Box::pin(async move {
        GATE_RAN_ON_CAP_DENY.store(true, Ordering::SeqCst);
        Ok(())
    })
}

#[tokio::test]
async fn first_denial_short_circuits_later_gates() {
    let kernel = test_kernel().await;
    let reg = ExtRegistry::new().gate(deny_secret).gate(record_after_deny);
    let (caps, null) = (CallerCapabilities::anonymous(), Value::Null);
    assert!(authorize_with(&reg, &anon(), &caps, "secret.read", &null, &kernel).await.is_err());
    assert!(!AFTER_DENY_RAN.load(Ordering::SeqCst));
}

#[tokio::test]
async fn gates_never_run_for_callers_failing_the_capability_check() {
    let kernel = test_kernel().await;
    let reg = ExtRegistry::new().gate(record_cap_deny);
    let (caps, null) = (CallerCapabilities::anonymous(), Value::Null);
    let e = authorize_with(&reg, &anon(), &caps, "kernel.shutdown", &null, &kernel)
        .await
        .unwrap_err();
    assert!(e.error.unwrap().contains("permission denied"));
    assert!(!GATE_RAN_ON_CAP_DENY.load(Ordering::SeqCst));
}

// ---- Wire-level: real `handle_connection`, cfg(test) probe route/gate ----

async fn json_roundtrip(kernel: &KernelRef, line: &str) -> Response {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (tx, _rx) = tokio::sync::watch::channel(false);
    tokio::spawn(crate::daemon::handle_connection(server, Arc::clone(kernel), tx));
    let (r, mut w) = tokio::io::split(client);
    w.write_all(format!("{line}\n").as_bytes()).await.unwrap();
    let mut out = String::new();
    BufReader::new(r).read_line(&mut out).await.unwrap();
    serde_json::from_str(&out).unwrap()
}

#[tokio::test]
async fn wire_json_gate_denial_is_structured() {
    let kernel = test_kernel().await;
    let resp = json_roundtrip(&kernel, r#"{"method":"rpc_ext.gated.x","params":null,"id":"7"}"#).await;
    assert!(!resp.ok);
    assert_eq!(resp.error_kind.as_deref(), Some("scope_denied"));
    assert_eq!(resp.id.as_deref(), Some("7"));
}

#[tokio::test]
async fn wire_json_ext_route_enforces_capability_and_passes_auth() {
    let kernel = test_kernel().await;
    let anon = json_roundtrip(&kernel, r#"{"method":"rpc_ext.test.echo","params":null}"#).await;
    assert!(!anon.ok);
    assert!(anon.error.unwrap().contains("requires capability Write"));
    let ok = json_roundtrip(
        &kernel,
        r#"{"method":"rpc_ext.test.echo","params":null,"auth":"write"}"#,
    )
    .await;
    assert!(ok.ok, "{:?}", ok.error);
    assert_eq!(ok.result.unwrap()["auth"], "write");
}

#[cfg(feature = "rvf-rpc")]
async fn rvf_roundtrip(kernel: &KernelRef, req: clawft_rpc::Request) -> Response {
    use crate::rvf_codec::{RvfFrameReader, RvfFrameWriter};
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (tx, _rx) = tokio::sync::watch::channel(false);
    tokio::spawn(crate::daemon::handle_connection(server, Arc::clone(kernel), tx));
    let (r, mut w) = tokio::io::split(client);
    w.write_all(b"RVFS").await.unwrap();
    let mut fw = RvfFrameWriter::new(w);
    let (t, p, f, id) = crate::rvf_rpc::encode_request(&req, 1);
    fw.write_frame(t, &p, f, id).await.unwrap();
    let frame = RvfFrameReader::new(r).read_frame().await.unwrap().unwrap();
    crate::rvf_rpc::decode_response(&frame).unwrap()
}

/// Regression: RVF frames used to skip the capability check entirely,
/// so an unauthenticated `kernel.shutdown` ran as Admin.
#[cfg(feature = "rvf-rpc")]
#[tokio::test]
async fn wire_rvf_enforces_capabilities_and_gates() {
    let kernel = test_kernel().await;
    let req = |method: &str, auth: Option<&str>| clawft_rpc::Request {
        method: method.into(),
        params: Value::Null,
        id: Some("1".into()),
        auth: auth.map(String::from),
    };
    let r = rvf_roundtrip(&kernel, req("kernel.shutdown", None)).await;
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("permission denied"));
    let r = rvf_roundtrip(&kernel, req("rpc_ext.gated.x", None)).await;
    assert_eq!(r.error_kind.as_deref(), Some("scope_denied"));
    let r = rvf_roundtrip(&kernel, req("rpc_ext.test.echo", Some("write"))).await;
    assert!(r.ok, "{:?}", r.error);
}

#[test]
fn voice_principal_is_not_admin() {
    let caps = CallerCapabilities::from_scopes(
        CallerCtx::internal_voice().auth.unwrap().split(',').map(str::to_owned),
    );
    assert!(caps.allows(Capability::Write));
    assert!(!caps.allows(Capability::Admin));
}
