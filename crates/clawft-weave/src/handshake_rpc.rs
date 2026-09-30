//! `kernel.handshake` and the request-envelope gate (ADR-103 D14, package A).
//!
//! The handshake tells a client which daemon it reached (node, project,
//! runtime dir, build) so a stale socket pointing at the wrong daemon is
//! caught before any real call. [`envelope_refusal`] rejects requests whose
//! `proto` is outside the supported range or whose `project` is not a ULID.

use std::path::PathBuf;

use clawft_rpc::handshake::{
    DaemonBuild, Handshake, INVALID_PROJECT_KIND, ProtoCheck, ProtoRange, check_proto,
    handshake_value, proto_mismatch_response,
};
use clawft_rpc::Response;
use clawft_types::project::{read_project_toml, validate_id};
use clawft_types::runtime_paths::{RootSource, RuntimePaths};

use crate::rpc_ext::{ExtCall, ExtFuture};

const BUILD_SHA: &str = env!("BUILD_GIT_HASH");
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Project ULID this runtime is bound to: the `project.toml` id when the
/// root was resolved from a project directory, else `None` (the user-level
/// daemon serves no single project in Phase 1).
pub fn bound_project_id(paths: &RuntimePaths) -> Option<String> {
    match paths.source() {
        RootSource::Project(dir) => read_project_toml(dir).ok().flatten().map(|p| p.id),
        _ => None,
    }
}

/// Assemble the handshake for a daemon owning `paths`.
pub fn build_handshake(node_id: String, paths: &RuntimePaths, binary: Option<PathBuf>) -> Handshake {
    Handshake {
        proto: ProtoRange::supported(),
        node_id,
        user_id: None,
        project_id: bound_project_id(paths),
        depth: 0,
        parent: None,
        runtime_dir: paths.root().display().to_string(),
        pid: std::process::id(),
        version: VERSION.to_owned(),
        sha: BUILD_SHA.to_owned(),
        binary: binary.map(|p| p.display().to_string()),
    }
}

/// `kernel.handshake` handler (registered in `rpc_ext::ROUTES`, `Read`).
pub fn handle(call: ExtCall) -> ExtFuture {
    Box::pin(async move {
        let node_id = call
            .ctx
            .kernel
            .read()
            .await
            .cluster_membership()
            .local_node_id()
            .to_owned();
        let h = build_handshake(node_id, &RuntimePaths::resolve(), std::env::current_exe().ok());
        Response::success(handshake_value(&h))
    })
}

/// Refuse a request with an unsupported `proto` or malformed `project`.
///
/// A missing `proto` is a legacy client: accepted in Phase 1 with one
/// warning per process.
pub fn envelope_refusal(proto: Option<u32>, project: Option<&str>) -> Option<Response> {
    match check_proto(proto) {
        ProtoCheck::Supported => {}
        ProtoCheck::Legacy => warn_legacy_once(),
        ProtoCheck::Unsupported(p) => {
            let exe = std::env::current_exe().ok().map(|p| p.display().to_string());
            return Some(proto_mismatch_response(
                p,
                DaemonBuild {
                    sha: BUILD_SHA,
                    version: VERSION,
                    exe: exe.as_deref(),
                },
            ));
        }
    }
    if let Some(id) = project
        && validate_id(id).is_err()
    {
        return Some(Response::error_with_kind(
            INVALID_PROJECT_KIND,
            format!("invalid project id {id:?}: expected a 26-character ULID"),
        ));
    }
    None
}

fn warn_legacy_once() {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            "rpc client sent no `proto` (legacy client, treated as protocol 0); \
             accepted in this release, refused from ADR-103 Phase 2: update the client"
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_accepts_legacy_and_current() {
        assert!(envelope_refusal(None, None).is_none());
        assert!(envelope_refusal(Some(clawft_rpc::PROTO_VERSION), None).is_none());
    }

    #[test]
    fn envelope_refuses_unsupported_proto_with_data() {
        let r = envelope_refusal(Some(clawft_rpc::PROTO_VERSION + 1), None).unwrap();
        assert_eq!(r.error_kind.as_deref(), Some("proto_mismatch"));
        assert_eq!(r.data.unwrap()["daemon"]["proto"], clawft_rpc::PROTO_VERSION);
        assert!(envelope_refusal(Some(0), None).is_some());
    }

    #[test]
    fn envelope_refuses_malformed_project() {
        let r = envelope_refusal(Some(1), Some("../x")).unwrap();
        assert_eq!(r.error_kind.as_deref(), Some("invalid_project"));
        assert!(envelope_refusal(Some(1), Some("01J0000000000000000000000A")).is_none());
    }

    #[test]
    fn handshake_binds_project_from_project_toml() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".weftos")).unwrap();
        std::fs::write(
            d.path().join(".weftos/project.toml"),
            "schema = 1\nid = \"01J0000000000000000000000A\"\nname = \"p\"\n\
             created = 2026-01-01T00:00:00Z\n",
        )
        .unwrap();
        let paths = RuntimePaths::resolve_with(None, Some(d.path()), None);
        let h = build_handshake("node".into(), &paths, None);
        assert_eq!(h.project_id.as_deref(), Some("01J0000000000000000000000A"));
        assert_eq!((h.depth, h.parent), (0, None));
        assert_eq!(h.node_id, "node");
        // An isolated (env) runtime is not bound to a project.
        let iso = RuntimePaths::at(d.path().join("rt"));
        assert_eq!(build_handshake("n".into(), &iso, None).project_id, None);
    }
}
