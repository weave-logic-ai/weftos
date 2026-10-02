//! `connect_resolved` against fake daemons on temp sockets.

use super::*;
use crate::handshake::{DaemonBuild, handshake_value, proto_mismatch_response};
use crate::resolve::{ResolveFlags, ResolveInputs, resolve_with};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

const ID_A: &str = "01J0000000000000000000000A";
const ID_B: &str = "01J0000000000000000000000B";

fn hs(dir: &Path, project: Option<&str>, node: &str) -> Handshake {
    Handshake {
        proto: ProtoRange::supported(),
        node_id: node.into(),
        user_id: None,
        user_key_id: None,
        profile: None,
        roles: Vec::new(),
        project_id: project.map(String::from),
        bound_via: if project.is_some() { BoundVia::Project } else { BoundVia::None },
        depth: 0,
        parent: None,
        runtime_dir: dir.display().to_string(),
        pid: 1,
        version: "0.8.1".into(),
        sha: "abcd1234".into(),
        binary: None,
        mesh: None,
    }
}

/// Serve one connection: answer every line with `reply(request)`.
fn serve(dir: &Path, reply: impl Fn(&Request) -> Response + Send + 'static) {
    let l = UnixListener::bind(dir.join("kernel.sock")).unwrap();
    tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        let (r, mut w) = s.into_split();
        let mut lines = BufReader::new(r).lines();
        while let Some(line) = lines.next_line().await.unwrap() {
            let req: Request = serde_json::from_str(&line).unwrap();
            let mut out = serde_json::to_string(&reply(&req)).unwrap();
            out.push('\n');
            w.write_all(out.as_bytes()).await.unwrap();
        }
    });
}

fn resolution(dir: &Path, project: Option<&str>) -> Resolution {
    resolve_with(&ResolveInputs {
        flags: ResolveFlags {
            runtime: Some(dir.to_path_buf()),
            project: project.map(String::from),
        },
        ..ResolveInputs::default()
    })
    .unwrap()
}

async fn connect(res: &Resolution) -> Result<Connected, ConnectError> {
    DaemonClient::connect_resolved(res).await
}

#[tokio::test]
async fn happy_path_returns_handshake_and_sends_proto_and_project() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().to_path_buf();
    serve(d.path(), move |req| {
        assert_eq!(req.method, "kernel.handshake");
        assert_eq!(req.proto, Some(PROTO_VERSION));
        assert_eq!(req.project.as_deref(), Some(ID_A));
        Response::success(handshake_value(&hs(&dir, Some(ID_A), "node1")))
    });
    let c = connect(&resolution(d.path(), Some(ID_A))).await.unwrap();
    assert_eq!(c.handshake.node_id, "node1");
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
}

#[tokio::test]
async fn project_mismatch_is_a_hard_error_with_remedy() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().to_path_buf();
    serve(d.path(), move |_| {
        Response::success(handshake_value(&hs(&dir, Some(ID_B), "n")))
    });
    let err = connect(&resolution(d.path(), Some(ID_A))).await.err().unwrap();
    assert!(matches!(err, ConnectError::ProjectMismatch { .. }), "{err}");
    let text = err.to_string();
    assert!(text.contains(ID_A) && text.contains(ID_B), "{text}");
    assert!(text.contains(&format!("--project {ID_A}")), "{text}");
}

#[tokio::test]
async fn unbound_daemon_on_explicit_endpoint_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().to_path_buf();
    serve(d.path(), move |_| {
        Response::success(handshake_value(&hs(&dir, None, "n")))
    });
    let err = connect(&resolution(d.path(), Some(ID_A))).await.err().unwrap();
    assert!(matches!(err, ConnectError::ProjectUnbound { .. }), "{err}");
    assert!(err.to_string().contains("serves none"), "{err}");
}

#[tokio::test]
async fn unbound_daemon_on_default_endpoint_is_a_warning() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().to_path_buf();
    serve(d.path(), move |_| {
        Response::success(handshake_value(&hs(&dir, None, "n")))
    });
    let mut res = resolution(d.path(), Some(ID_A));
    res.source = ResolveSource::Default;
    let c = connect(&res).await.unwrap();
    assert!(c.warnings.iter().any(|w| w.contains("not bound")), "{:?}", c.warnings);
}

#[tokio::test]
async fn user_daemon_is_unbound_without_warning_or_error() {
    // D14: a project resolved to the user daemon (explicit manifest source or
    // default) must neither fail nor warn "not bound to a project".
    for source in [ResolveSource::Manifest, ResolveSource::Default] {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().to_path_buf();
        serve(d.path(), move |_| {
            let mut h = hs(&dir, None, "n");
            h.profile = Some("user".into());
            Response::success(handshake_value(&h))
        });
        let mut res = resolution(d.path(), Some(ID_A));
        res.source = source;
        let c = connect(&res).await.unwrap();
        assert!(!c.warnings.iter().any(|w| w.contains("not bound")), "{:?}", c.warnings);
    }
}

#[tokio::test]
async fn node_mismatch_is_a_hard_error() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().to_path_buf();
    serve(d.path(), move |_| {
        Response::success(handshake_value(&hs(&dir, None, "other")))
    });
    let res = resolution(d.path(), None).expect_node("wanted");
    let err = connect(&res).await.err().unwrap();
    assert!(matches!(err, ConnectError::NodeMismatch { .. }), "{err}");
}

#[tokio::test]
async fn runtime_dir_disagreement_warns() {
    let d = tempfile::tempdir().unwrap();
    serve(d.path(), |_| {
        Response::success(handshake_value(&hs(Path::new("/elsewhere"), None, "n")))
    });
    let c = connect(&resolution(d.path(), None)).await.unwrap();
    assert!(c.warnings.iter().any(|w| w.contains("/elsewhere")), "{:?}", c.warnings);
}

#[tokio::test]
async fn proto_mismatch_response_sets_client_too_old_from_daemon_min() {
    let d = tempfile::tempdir().unwrap();
    serve(d.path(), |_| {
        let mut r = proto_mismatch_response(
            0,
            DaemonBuild {
                sha: "abcd1234",
                version: "0.8.1",
            },
        );
        // A daemon whose minimum is above this client.
        r.data = Some(serde_json::json!({"daemon": {"sha": "abcd1234", "min": PROTO_VERSION + 1}}));
        r
    });
    let err = connect(&resolution(d.path(), None)).await.err().unwrap();
    match &err {
        ConnectError::ProtoMismatch { client_too_old, .. } => assert!(*client_too_old),
        other => panic!("{other}"),
    }
    assert_eq!(
        err.failure(),
        Some(Failure::ProtoMismatch {
            daemon_sha: "abcd1234".into(),
            client_too_old: true
        })
    );
}

#[tokio::test]
async fn handshake_reporting_a_range_without_us_is_a_proto_mismatch() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().to_path_buf();
    serve(d.path(), move |_| {
        let mut h = hs(&dir, None, "n");
        h.proto = ProtoRange {
            current: PROTO_VERSION + 3,
            min: PROTO_VERSION + 2,
        };
        Response::success(handshake_value(&h))
    });
    let err = connect(&resolution(d.path(), None)).await.err().unwrap();
    match &err {
        ConnectError::ProtoMismatch { client_too_old, .. } => assert!(*client_too_old),
        other => panic!("{other}"),
    }
    assert!(err.to_string().contains("update this `weft`"), "{err}");
}

fn old_daemon(dir: &Path) {
    serve(dir, |req| {
        if req.method == "kernel.status" {
            Response::success(serde_json::json!({"build": {"sha": "old12345", "version": "0.7"}}))
        } else {
            Response::error(format!("unknown method: {}", req.method))
        }
    });
}

#[tokio::test]
async fn old_daemon_on_default_endpoint_degrades_with_warning() {
    let d = tempfile::tempdir().unwrap();
    old_daemon(d.path());
    let mut res = resolution(d.path(), Some(ID_A));
    res.source = ResolveSource::Default;
    let c = connect(&res).await.unwrap();
    assert_eq!(c.handshake.sha, "old12345");
    assert_eq!(c.handshake.proto, ProtoRange { current: 0, min: 0 });
    assert_eq!(c.warnings.len(), 1);
    assert!(c.warnings[0].contains("weaver kernel restart"), "{:?}", c.warnings);
}

#[tokio::test]
async fn old_daemon_on_explicit_endpoint_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    old_daemon(d.path());
    // resolution() is flag-level.
    let err = connect(&resolution(d.path(), Some(ID_A))).await.err().unwrap();
    assert!(matches!(err, ConnectError::NoHandshake { .. }), "{err}");
    assert!(err.to_string().contains("weaver kernel restart"), "{err}");
}

#[tokio::test]
async fn old_daemon_with_node_pin_is_an_error_even_on_default() {
    let d = tempfile::tempdir().unwrap();
    old_daemon(d.path());
    let mut res = resolution(d.path(), None).expect_node("wanted");
    res.source = ResolveSource::Default;
    let err = connect(&res).await.err().unwrap();
    assert!(matches!(err, ConnectError::NoHandshake { .. }), "{err}");
}

#[tokio::test]
async fn non_unknown_method_handshake_errors_never_downgrade() {
    for (kind, msg) in [
        (None, "permission denied: method 'kernel.handshake' requires capability Read"),
        (Some("scope_denied"), "unknown method: nope"),
    ] {
        let d = tempfile::tempdir().unwrap();
        serve(d.path(), move |_| match kind {
            Some(k) => Response::error_with_kind(k, msg),
            None => Response::error(msg),
        });
        let mut res = resolution(d.path(), None);
        res.source = ResolveSource::Default;
        let err = connect(&res).await.err().unwrap();
        assert!(matches!(err, ConnectError::HandshakeRefused { .. }), "{err}");
    }
}

#[tokio::test]
async fn unreachable_prints_what_was_tried() {
    let d = tempfile::tempdir().unwrap();
    let err = connect(&resolution(d.path(), None)).await.err().unwrap();
    assert_eq!(err.failure(), Some(Failure::NoSocket));
    let text = err.to_string();
    assert!(text.contains("no socket file"), "{text}");
    assert!(text.contains("tried, in order"), "{text}");
}

#[test]
fn stamp_defaults_proto_and_keeps_explicit_fields() {
    let mut r = Request::new("x");
    stamp_request(&mut r, None);
    assert_eq!(r.auth.as_deref(), Some("admin"));
    assert_eq!(r.proto, Some(PROTO_VERSION));
    let mut r = Request::new("x").with_auth("read");
    r.proto = Some(7);
    r.project = Some("P".into());
    stamp_request(&mut r, None);
    assert_eq!(r.auth.as_deref(), Some("read"));
    assert_eq!(r.proto, Some(7));
    assert_eq!(r.project.as_deref(), Some("P"));
}

/// Two clients with different contexts never share stamps (the gateway
/// case), and a per-client context wins over the process default.
#[tokio::test]
async fn contexts_are_per_client() {
    let seen = |dir: PathBuf, want: Option<&'static str>| {
        serve(&dir, move |req| {
            assert_eq!(req.project.as_deref(), want);
            Response::success(serde_json::json!({}))
        });
    };
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    seen(a.path().to_path_buf(), Some(ID_A));
    seen(b.path().to_path_buf(), Some(ID_B));
    let ctx = |p: &str| ClientContext {
        project: Some(p.into()),
        proto: PROTO_VERSION,
    };
    let mut ca = DaemonClient::connect_path(a.path().join("kernel.sock"))
        .await
        .unwrap()
        .with_context(ctx(ID_A));
    let mut cb = DaemonClient::connect_path(b.path().join("kernel.sock"))
        .await
        .unwrap()
        .with_context(ctx(ID_B));
    assert!(ca.simple_call("x").await.unwrap().ok);
    assert!(cb.simple_call("x").await.unwrap().ok);
}

/// A child-kernel resolution over a fake home: the user daemon's socket is
/// `<home>/.weftos/run/kernel.sock`, the child's `<home>/.weftos/run/<id>/`.
struct ChildWorld {
    _d: tempfile::TempDir,
    home: PathBuf,
    run: PathBuf,
    res: Resolution,
}

fn child_world() -> ChildWorld {
    let d = tempfile::Builder::new().prefix("rpcc").tempdir_in("/tmp").unwrap();
    let home = d.path().join("home");
    let proj = home.join("app");
    std::fs::create_dir_all(proj.join(".weftos")).unwrap();
    std::fs::write(
        proj.join(".weftos/project.toml"),
        format!("schema = 1\nid = \"{ID_A}\"\nname = \"app\"\ncreated = 2026-01-01T00:00:00Z\n"),
    )
    .unwrap();
    let mdir = crate::resolve::manifests_dir(&home);
    std::fs::create_dir_all(&mdir).unwrap();
    std::fs::write(
        mdir.join(format!("{ID_A}.toml")),
        format!(
            "schema = 1\nid = \"{ID_A}\"\nname = \"app\"\nroot = \"{}\"\n\
             created = 2026-01-01T00:00:00Z\nlast_seen = 2026-01-01T00:00:00Z\n\
             [serve]\nvia = \"child-kernel\"\n",
            proj.display()
        ),
    )
    .unwrap();
    let res = resolve_with(&ResolveInputs {
        cwd: Some(proj),
        home: Some(home.clone()),
        ..ResolveInputs::default()
    })
    .unwrap();
    let run = home.join(".weftos/run");
    std::fs::create_dir_all(run.join(ID_A)).unwrap();
    ChildWorld { _d: d, home, run, res }
}

/// A fake user daemon: `project.ensure_running` starts the fake child
/// (reporting `child_project`) and counts the calls.
fn fake_user_daemon(w: &ChildWorld, child_project: &'static str) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let l = UnixListener::bind(w.run.join("kernel.sock")).unwrap();
    let (c, child_dir) = (calls.clone(), w.run.join(ID_A));
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            let (c, child_dir) = (c.clone(), child_dir.clone());
            tokio::spawn(async move {
                let (r, mut w) = s.into_split();
                let mut lines = BufReader::new(r).lines();
                while let Some(line) = lines.next_line().await.unwrap() {
                    let req: Request = serde_json::from_str(&line).unwrap();
                    assert_eq!(req.method, "project.ensure_running");
                    assert_eq!(req.params["id"], ID_A);
                    if c.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        let dir = child_dir.clone();
                        serve(&child_dir, move |_| {
                            Response::success(handshake_value(&hs(&dir, Some(child_project), "childnode")))
                        });
                    }
                    let mut out = serde_json::to_string(&Response::success(
                        serde_json::json!({"started": true, "pid": 4242}),
                    ))
                    .unwrap();
                    out.push('\n');
                    w.write_all(out.as_bytes()).await.unwrap();
                }
            });
        }
    });
    calls
}

#[tokio::test]
async fn child_kernel_is_ensured_once_then_handshaken() {
    let w = child_world();
    let calls = fake_user_daemon(&w, ID_A);
    let c = connect(&w.res).await.unwrap();
    assert_eq!(c.handshake.project_id.as_deref(), Some(ID_A));
    assert_eq!(c.handshake.node_id, "childnode");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(c.warnings.iter().any(|m| m.contains("started") && m.contains("4242")), "{:?}", c.warnings);
    let _ = &w.home;
}

#[tokio::test]
async fn a_child_answering_for_another_project_stays_a_hard_error() {
    let w = child_world();
    let _calls = fake_user_daemon(&w, ID_B);
    match connect(&w.res).await {
        Err(ConnectError::ProjectMismatch { expected, actual }) => {
            assert_eq!((expected.as_str(), actual.as_str()), (ID_A, ID_B));
        }
        other => panic!("expected a project mismatch, got {:?}", other.map(|c| c.handshake.node_id)),
    }
}

#[tokio::test]
async fn no_user_daemon_means_a_clear_ensure_failure() {
    let w = child_world();
    match connect(&w.res).await {
        Err(ConnectError::EnsureFailed { project_id, detail, .. }) => {
            assert_eq!(project_id, ID_A);
            assert!(detail.contains("weaver kernel start --profile user"), "{detail}");
        }
        other => panic!("expected EnsureFailed, got {:?}", other.map(|c| c.handshake.node_id)),
    }
}

#[tokio::test]
async fn a_refused_ensure_keeps_the_user_daemons_error_kind() {
    let w = child_world();
    let l = UnixListener::bind(w.run.join("kernel.sock")).unwrap();
    tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        let (r, mut wr) = s.into_split();
        let mut lines = BufReader::new(r).lines();
        if lines.next_line().await.unwrap().is_some() {
            let mut out = serde_json::to_string(&Response::error_with_kind(
                "project_revoked",
                "project was revoked; delete the marker by hand",
            ))
            .unwrap();
            out.push('\n');
            wr.write_all(out.as_bytes()).await.unwrap();
        }
    });
    match connect(&w.res).await {
        Err(e @ ConnectError::EnsureFailed { .. }) => {
            let ConnectError::EnsureFailed { kind, .. } = &e else { unreachable!() };
            assert_eq!(kind.as_deref(), Some("project_revoked"));
            assert!(e.to_string().contains("(project_revoked)"), "{e}");
        }
        other => panic!("expected EnsureFailed, got {:?}", other.map(|c| c.handshake.node_id)),
    }
}

#[tokio::test]
async fn a_running_child_is_not_ensured_again() {
    let w = child_world();
    let dir = w.run.join(ID_A);
    serve(&dir, {
        let dir = dir.clone();
        move |_| Response::success(handshake_value(&hs(&dir, Some(ID_A), "childnode")))
    });
    // No user daemon at all: nothing may be asked of it.
    let c = connect(&w.res).await.unwrap();
    assert!(c.warnings.iter().all(|m| !m.contains("started")), "{:?}", c.warnings);
}
