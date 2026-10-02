//! Protocol version, handshake payload and the remedy text (ADR-103 D14,
//! Phase 1 package A).
//!
//! One definition shared by the daemon (which builds [`Handshake`]) and the
//! client (which checks it), so the wire shape cannot drift.
//!
//! Deviation from the Phase 1 plan: a request with no `proto` (a legacy
//! client) is accepted for ALL methods in Phase 1, not only read-only ones,
//! so existing clients keep working. Phase 2 may restrict or refuse it.
//! `kernel.handshake` is exempt from the proto refusal: it is the discovery
//! call that tells a client which range the daemon accepts.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::protocol::Response;

/// Protocol version this build speaks.
pub const PROTO_VERSION: u32 = 1;
/// Oldest client protocol this build still accepts.
pub const PROTO_MIN: u32 = 1;

/// `error_kind` of a refused protocol version.
pub const PROTO_MISMATCH_KIND: &str = "proto_mismatch";
/// `error_kind` of a malformed `Request.project`.
pub const INVALID_PROJECT_KIND: &str = "invalid_project";
/// `error_kind` of a `Request.project` that is not the daemon's project.
pub const PROJECT_MISMATCH_KIND: &str = "project_mismatch";

/// Range of client protocols a daemon accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtoRange {
    /// Newest protocol the daemon speaks.
    pub current: u32,
    /// Oldest protocol it still accepts.
    pub min: u32,
}

impl ProtoRange {
    /// The range this build supports.
    pub const fn supported() -> Self {
        Self {
            current: PROTO_VERSION,
            min: PROTO_MIN,
        }
    }

    /// Whether a client speaking `proto` is inside the range.
    pub fn accepts(&self, proto: u32) -> bool {
        (self.min..=self.current).contains(&proto)
    }
}

/// How a daemon came to be bound to its project.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BoundVia {
    /// Runtime root resolved from a project directory (`project.toml` id).
    Project,
    /// Runtime root matched exactly one manifest's `[serve] runtime_dir`.
    Manifest,
    /// Not bound to a project.
    #[default]
    None,
}

/// Identity a daemon reports from `kernel.handshake`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handshake {
    /// Protocol range the daemon accepts.
    pub proto: ProtoRange,
    /// Mesh node id (32 hex), derived from the node key.
    pub node_id: String,
    /// Owning user id once user identity exists; `None` until then.
    /// The user daemon reports the local uid (Phase 1, unverified).
    #[serde(default)]
    pub user_id: Option<String>,
    /// Id of the user key: the node-id-style hash of the chain verifying
    /// key. Phase 1 uses the migrated `chain.key` (plan D-1); it becomes
    /// `~/.weftos/user.key` in Phase 3.
    #[serde(default)]
    pub user_key_id: Option<String>,
    /// Daemon profile (`user`), `None` for the default project/legacy daemon.
    #[serde(default)]
    pub profile: Option<String>,
    /// Roles this daemon runs (`machine`, `user`, ...); empty when unset.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Project ULID the daemon serves, when it is bound to one.
    #[serde(default)]
    pub project_id: Option<String>,
    /// How `project_id` was determined (decided once at daemon startup).
    #[serde(default)]
    pub bound_via: BoundVia,
    /// Nesting depth (0 for a top-level daemon; nested instances: Phase 4).
    #[serde(default)]
    pub depth: u32,
    /// Parent instance id (`None` at depth 0).
    #[serde(default)]
    pub parent: Option<String>,
    /// Runtime directory the daemon owns.
    pub runtime_dir: String,
    /// Daemon OS process id.
    pub pid: u32,
    /// Daemon crate version.
    #[serde(default)]
    pub version: String,
    /// Daemon build stamp (`BUILD_GIT_HASH`, `-dirty` suffix when dirty).
    #[serde(default)]
    pub sha: String,
    /// Path of the daemon binary, when known.
    #[serde(default)]
    pub binary: Option<String>,
    /// How this daemon relates to the mesh (ADR-103 P3-U). Absent from older
    /// daemons and from daemons that have not decided yet; older clients
    /// ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<MeshHandshake>,
}

/// The `mesh` object of [`Handshake`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshHandshake {
    /// `service`, `collapsed` or `off`.
    pub mode: String,
    /// Service mode: `connected` or `reconnecting`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Service mode: the machine node id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_node_id: Option<String>,
    /// Service mode: serial of the current user certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_serial: Option<u64>,
    /// Service mode: expiry of the current user certificate (unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_not_after: Option<u64>,
    /// Service mode: negotiated mesh-local protocol version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proto: Option<u32>,
}

impl MeshHandshake {
    /// One-line summary, e.g. `service (connected)`.
    pub fn summary(&self) -> String {
        match (self.mode.as_str(), self.state.as_deref()) {
            (m, Some(st)) => format!("{m} ({st})"),
            (m, None) => m.to_owned(),
        }
    }
}

/// Outcome of checking the `proto` a request carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoCheck {
    /// Inside the supported range.
    Supported,
    /// Field absent: a pre-A client, treated as protocol 0. Accepted in
    /// Phase 1; refused from Phase 2.
    Legacy,
    /// Outside `[PROTO_MIN, PROTO_VERSION]`.
    Unsupported(u32),
}

/// Classify the `proto` field of a request.
pub fn check_proto(proto: Option<u32>) -> ProtoCheck {
    match proto {
        None => ProtoCheck::Legacy,
        Some(p) if ProtoRange::supported().accepts(p) => ProtoCheck::Supported,
        Some(p) => ProtoCheck::Unsupported(p),
    }
}

/// Facts about the running daemon that go into a mismatch response.
#[derive(Debug, Clone, Copy)]
pub struct DaemonBuild<'a> {
    pub sha: &'a str,
    pub version: &'a str,
}

/// The refusal for an unsupported `proto` (`error_kind = "proto_mismatch"`).
///
/// `error` is one line ending in the remedy; `data` is
/// `{client:{proto}, daemon:{proto,min,version,sha}}`. The binary path is
/// deliberately absent: this response is sent before authorization.
pub fn proto_mismatch_response(client: u32, daemon: DaemonBuild<'_>) -> Response {
    let range = ProtoRange::supported();
    let msg = format!(
        "protocol mismatch: client speaks {client}, daemon accepts {}..={} ({}); {}",
        range.min,
        range.current,
        daemon.sha,
        remedy_for_skew(client < range.min, daemon.sha),
    );
    let mut resp = Response::error_with_kind(PROTO_MISMATCH_KIND, msg);
    resp.data = Some(json!({
        "client": {"proto": client},
        "daemon": {
            "proto": range.current,
            "min": range.min,
            "version": daemon.version,
            "sha": daemon.sha,
        },
    }));
    resp
}

/// `true` when a build stamp carries the `-dirty` marker of a dev build.
pub fn is_dirty(sha: &str) -> bool {
    sha.ends_with("-dirty")
}

/// The next command for a client/daemon version skew.
///
/// Dirty or dev builds rebuild and restart; release builds update and
/// restart. Kept in one function so the install-update channel can change
/// the text in one place.
pub fn remedy(build_sha: &str, dirty: bool) -> String {
    if dirty || is_dirty(build_sha) || build_sha.is_empty() || build_sha == "unknown" {
        "run `scripts/build.sh install`, then `weaver kernel restart`".to_owned()
    } else {
        "run `weaver update`, then `weaver kernel restart`".to_owned()
    }
}

/// Remedy for a proto mismatch: the side that is older must be updated.
fn remedy_for_skew(client_too_old: bool, daemon_sha: &str) -> String {
    if client_too_old {
        "update this `weft` to match the daemon (`weaver update` or `scripts/build.sh install`)"
            .to_owned()
    } else {
        remedy(daemon_sha, false)
    }
}

/// A connection or handshake failure, for [`remedy_for`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// No socket file at the endpoint.
    NoSocket,
    /// Socket file exists, nothing accepts on it.
    StaleSocket,
    /// The socket belongs to another user.
    PermissionDenied,
    /// The daemon refused this client's protocol version.
    ProtoMismatch { daemon_sha: String, client_too_old: bool },
    /// The daemon serves a different project than expected.
    ProjectMismatch { expected: String, actual: String },
    /// The daemon is bound to no project but one was expected.
    ProjectUnbound { expected: String },
    /// The daemon's node id is not the expected one.
    NodeMismatch { expected: String, actual: String },
    /// Client and daemon builds differ (warning-level).
    VersionSkew { daemon_sha: String },
}

/// Map a failure to the exact next command.
pub fn remedy_for(failure: &Failure) -> String {
    match failure {
        Failure::NoSocket => "start a kernel with `weaver kernel start`".to_owned(),
        Failure::StaleSocket => {
            "start a kernel with `weaver kernel start` (it replaces the stale socket)".to_owned()
        }
        Failure::PermissionDenied => {
            "run as the user that owns the kernel, or point at your own with WEFTOS_RUNTIME_DIR"
                .to_owned()
        }
        Failure::ProtoMismatch {
            daemon_sha,
            client_too_old,
        } => remedy_for_skew(*client_too_old, daemon_sha),
        Failure::ProjectMismatch { expected, actual } => format!(
            "this socket serves project {actual}, not {expected}; \
             unset WEFTOS_RUNTIME_DIR or pass `--project {expected}`"
        ),
        Failure::ProjectUnbound { expected } => format!(
            "the daemon on this socket serves no project, but {expected} was expected; \
             run `weaver kernel start` inside that project, or drop --runtime / \
             WEFTOS_RUNTIME_DIR to use the user daemon"
        ),
        Failure::NodeMismatch { expected, actual } => format!(
            "this socket belongs to node {actual}, not {expected}; \
             the expected daemon is not the one listening here"
        ),
        Failure::VersionSkew { daemon_sha } => remedy(daemon_sha, false),
    }
}

/// Handshake payload as the `result` of a `kernel.handshake` response.
pub fn handshake_value(h: &Handshake) -> Value {
    serde_json::to_value(h).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_accepts_only_supported() {
        let r = ProtoRange::supported();
        assert!(r.accepts(PROTO_VERSION));
        assert!(!r.accepts(PROTO_VERSION + 1));
        assert!(!r.accepts(0));
    }

    #[test]
    fn check_proto_classifies() {
        assert_eq!(check_proto(None), ProtoCheck::Legacy);
        assert_eq!(check_proto(Some(1)), ProtoCheck::Supported);
        assert_eq!(check_proto(Some(0)), ProtoCheck::Unsupported(0));
        assert_eq!(check_proto(Some(99)), ProtoCheck::Unsupported(99));
    }

    #[test]
    fn mismatch_response_shape_client_newer() {
        let r = proto_mismatch_response(
            99,
            DaemonBuild {
                sha: "abcd1234",
                version: "0.8.1",
            },
        );
        assert!(!r.ok);
        assert_eq!(r.error_kind.as_deref(), Some("proto_mismatch"));
        let line = r.error.unwrap();
        assert!(!line.contains('\n'));
        assert!(line.contains("client speaks 99"), "{line}");
        assert!(line.contains("weaver update"), "{line}");
        let d = r.data.unwrap();
        assert_eq!(d["client"]["proto"], 99);
        assert_eq!(d["daemon"]["proto"], PROTO_VERSION);
        assert_eq!(d["daemon"]["min"], PROTO_MIN);
        assert_eq!(d["daemon"]["sha"], "abcd1234");
        assert!(d["daemon"].get("exe").is_none());
    }

    #[test]
    fn mismatch_response_client_older_says_update_weft() {
        let r = proto_mismatch_response(
            0,
            DaemonBuild {
                sha: "abcd1234",
                version: "0.8.1",
            },
        );
        assert!(r.error.unwrap().contains("update this `weft`"));
    }

    #[test]
    fn remedy_picks_channel() {
        assert!(remedy("abcd1234-dirty", false).contains("scripts/build.sh install"));
        assert!(remedy("abcd1234", true).contains("scripts/build.sh install"));
        assert!(remedy("unknown", false).contains("scripts/build.sh install"));
        assert!(remedy("abcd1234", false).contains("weaver update"));
    }

    #[test]
    fn remedy_for_names_command_per_failure() {
        assert!(remedy_for(&Failure::NoSocket).contains("weaver kernel start"));
        assert!(remedy_for(&Failure::StaleSocket).contains("stale"));
        assert!(remedy_for(&Failure::PermissionDenied).contains("WEFTOS_RUNTIME_DIR"));
        let m = remedy_for(&Failure::ProjectMismatch {
            expected: "A".into(),
            actual: "B".into(),
        });
        assert!(m.contains("--project A"), "{m}");
        assert!(
            remedy_for(&Failure::VersionSkew {
                daemon_sha: "abcd1234".into()
            })
            .contains("weaver update")
        );
    }

    #[test]
    fn handshake_roundtrips_and_tolerates_missing_optionals() {
        let h = Handshake {
            proto: ProtoRange::supported(),
            node_id: "n".into(),
            user_id: None,
            user_key_id: None,
            profile: None,
            roles: Vec::new(),
            project_id: Some("P".into()),
            bound_via: BoundVia::Manifest,
            depth: 0,
            parent: None,
            runtime_dir: "/r".into(),
            pid: 7,
            version: "0.8.1".into(),
            sha: "abcd".into(),
            binary: Some("/bin/weaver".into()),
            mesh: None,
        };
        let back: Handshake = serde_json::from_value(handshake_value(&h)).unwrap();
        assert_eq!(back, h);
        let minimal: Handshake = serde_json::from_value(json!({
            "proto": {"current": 1, "min": 1},
            "node_id": "n", "runtime_dir": "/r", "pid": 1
        }))
        .unwrap();
        assert_eq!(minimal.project_id, None);
        assert_eq!(minimal.depth, 0);
    }

    #[test]
    fn mesh_object_is_optional_and_additive() {
        let minimal: Handshake = serde_json::from_value(json!({
            "proto": {"current": 1, "min": 1},
            "node_id": "n", "runtime_dir": "/r", "pid": 1
        }))
        .unwrap();
        assert_eq!(minimal.mesh, None, "older daemons send no mesh object");
        let with: Handshake = serde_json::from_value(json!({
            "proto": {"current": 1, "min": 1},
            "node_id": "n", "runtime_dir": "/r", "pid": 1,
            "mesh": {"mode": "service", "state": "connected", "cert_serial": 3, "unknown_future_field": 1}
        }))
        .unwrap();
        let m = with.mesh.as_ref().unwrap();
        assert_eq!((m.mode.as_str(), m.cert_serial), ("service", Some(3)));
        assert_eq!(m.summary(), "service (connected)");
        // And a handshake without it serialises without the key.
        let v = handshake_value(&minimal);
        assert!(v.get("mesh").is_none(), "{v}");
        assert_eq!(MeshHandshake { mode: "off".into(), ..Default::default() }.summary(), "off");
    }
}
