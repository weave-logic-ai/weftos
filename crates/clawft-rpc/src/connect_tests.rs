//! `connect_resolved` against fake daemons on temp sockets.

use super::*;
use crate::handshake::{ProtoRange, handshake_value, proto_mismatch_response, DaemonBuild};
use crate::resolve::{ResolveInputs, resolve_with};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

const ID_A: &str = "01J0000000000000000000000A";
const ID_B: &str = "01J0000000000000000000000B";

fn hs(project: Option<&str>, node: &str) -> Handshake {
    Handshake {
        proto: ProtoRange::supported(),
        node_id: node.into(),
        user_id: None,
        project_id: project.map(String::from),
        depth: 0,
        parent: None,
        runtime_dir: "/r".into(),
        pid: 1,
        version: "0.8.1".into(),
        sha: "abcd1234".into(),
        binary: None,
    }
}

/// Serve one connection: answer every line with `reply(request)`.
fn serve(dir: &std::path::Path, reply: fn(&Request) -> Response) {
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

fn resolution(dir: &std::path::Path, project: Option<&str>) -> Resolution {
    resolve_with(&ResolveInputs {
        flags: crate::resolve::ResolveFlags {
            runtime: Some(dir.to_path_buf()),
            project: project.map(String::from),
        },
        ..ResolveInputs::default()
    })
    .unwrap()
}

#[tokio::test]
async fn happy_path_returns_handshake_and_sends_proto() {
    let d = tempfile::tempdir().unwrap();
    serve(d.path(), |req| {
        assert_eq!(req.method, "kernel.handshake");
        assert_eq!(req.proto, Some(PROTO_VERSION));
        Response::success(handshake_value(&hs(Some(ID_A), "node1")))
    });
    let (_c, h) = DaemonClient::connect_resolved(&resolution(d.path(), Some(ID_A)))
        .await
        .unwrap();
    assert_eq!(h.node_id, "node1");
}

#[tokio::test]
async fn project_mismatch_is_a_hard_error_with_remedy() {
    let d = tempfile::tempdir().unwrap();
    serve(d.path(), |_| {
        Response::success(handshake_value(&hs(Some(ID_B), "node1")))
    });
    let err = DaemonClient::connect_resolved(&resolution(d.path(), Some(ID_A)))
        .await
        .err()
        .expect("must fail");
    assert!(matches!(err, ConnectError::ProjectMismatch { .. }), "{err}");
    let text = err.to_string();
    assert!(text.contains(ID_A) && text.contains(ID_B), "{text}");
    assert!(text.contains(&format!("--project {ID_A}")), "{text}");
}

#[tokio::test]
async fn daemon_without_project_does_not_conflict() {
    let d = tempfile::tempdir().unwrap();
    serve(d.path(), |_| Response::success(handshake_value(&hs(None, "n"))));
    assert!(
        DaemonClient::connect_resolved(&resolution(d.path(), Some(ID_A)))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn node_mismatch_is_a_hard_error() {
    let d = tempfile::tempdir().unwrap();
    serve(d.path(), |_| Response::success(handshake_value(&hs(None, "other"))));
    let res = resolution(d.path(), None).expect_node("wanted");
    let err = DaemonClient::connect_resolved(&res).await.err().unwrap();
    assert!(matches!(err, ConnectError::NodeMismatch { .. }), "{err}");
    assert!(err.to_string().contains("wanted"));
}

#[tokio::test]
async fn proto_mismatch_response_surfaces_remedy() {
    let d = tempfile::tempdir().unwrap();
    serve(d.path(), |_| {
        proto_mismatch_response(
            99,
            DaemonBuild {
                sha: "abcd1234",
                version: "0.8.1",
                exe: None,
            },
        )
    });
    let err = DaemonClient::connect_resolved(&resolution(d.path(), None))
        .await
        .err()
        .unwrap();
    assert!(matches!(err, ConnectError::ProtoMismatch { .. }), "{err}");
    assert!(err.to_string().contains("weaver"), "{err}");
}

#[tokio::test]
async fn old_daemon_without_handshake_is_reported() {
    let d = tempfile::tempdir().unwrap();
    serve(d.path(), |_| Response::error("unknown method: kernel.handshake"));
    let err = DaemonClient::connect_resolved(&resolution(d.path(), None))
        .await
        .err()
        .unwrap();
    assert!(matches!(err, ConnectError::NoHandshake { .. }), "{err}");
}

#[tokio::test]
async fn unreachable_prints_what_was_tried() {
    let d = tempfile::tempdir().unwrap();
    let err = DaemonClient::connect_resolved(&resolution(d.path(), None))
        .await
        .err()
        .unwrap();
    assert_eq!(err.failure(), Some(Failure::NoSocket));
    let text = err.to_string();
    assert!(text.contains("no socket file"), "{text}");
    assert!(text.contains("tried, in order"), "{text}");
}

#[test]
fn stamp_defaults_proto_and_keeps_explicit_fields() {
    let mut r = Request::new("x");
    stamp_request(&mut r);
    assert_eq!(r.auth.as_deref(), Some("admin"));
    assert_eq!(r.proto, Some(PROTO_VERSION));
    let mut r = Request::new("x").with_auth("read");
    r.proto = Some(7);
    r.project = Some("P".into());
    stamp_request(&mut r);
    assert_eq!(r.auth.as_deref(), Some("read"));
    assert_eq!(r.proto, Some(7));
    assert_eq!(r.project.as_deref(), Some("P"));
}
