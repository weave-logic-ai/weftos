//! `mesh-local/1`: the child-registration messages between a per-project
//! kernel and its user daemon (ADR-103 A6, Phase 2 package A).
//!
//! JSON lines over the user daemon's socket. Flow:
//!
//! 1. child calls [`METHOD_CHALLENGE`] (`{project_id}`) and gets a nonce;
//! 2. child calls [`METHOD_REGISTER`] with a [`RegisterRequest`] whose
//!    [`NonceReply::sig`] is its project key over
//!    [`pop_signed_bytes`](clawft_types::project::cert::pop_signed_bytes)
//!    (`weftos-mesh-local-pop-v2\n<op>\n<user_key_id>\n<nonce>\n<project_id>`), plus the spawn
//!    nonce from `spawn.json`;
//! 3. the user daemon answers a [`RegisterAck`];
//! 4. [`METHOD_HEARTBEAT`] every [`RegisterAck::heartbeat_secs`] carries the
//!    child's activity; [`METHOD_UNREGISTER`] ends the session.
//!
//! Until Phase 3 adds peer-credential binding, the guard is the 0600 socket,
//! the spawn nonce and the proof of possession. Mesh delivery to a child is
//! register-only in Phase 2: the address exists, routing does not.
//!
//! Overlap: the `clawft-mesh-local` crate (Phase 3, other branch) defines its
//! own frame protocol. This module is only the Phase 2 child-registry subset
//! the supervisor and registry need; Phase 3 folds the two together.

use clawft_types::project::ProjectCert;
use serde::{Deserialize, Serialize};

pub use crate::handshake::ProtoRange;

/// Protocol number carried by `mesh-local/1`.
pub const PROTO_MESH_LOCAL: u32 = 1;
/// The `protocol` field value of this version.
pub const PROTOCOL_TAG: &str = "mesh-local/1";

/// Method: request a proof-of-possession nonce.
pub const METHOD_CHALLENGE: &str = "mesh.challenge";
/// Method: register a child with the user daemon.
pub const METHOD_REGISTER: &str = "mesh.register";
/// Method: liveness plus activity.
pub const METHOD_HEARTBEAT: &str = "mesh.heartbeat";
/// Method: end a session.
pub const METHOD_UNREGISTER: &str = "mesh.unregister";

fn protocol_tag() -> String {
    PROTOCOL_TAG.to_owned()
}

/// Who is registering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshRole {
    /// A per-project kernel.
    Project,
}

/// `mesh.challenge` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChallengeRequest {
    /// Project the child will register for.
    pub project_id: String,
}

/// `mesh.challenge` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChallengeReply {
    /// Single-use nonce, hex.
    pub nonce: String,
}

/// The signed nonce inside a [`RegisterRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonceReply {
    /// The nonce from [`ChallengeReply`].
    pub nonce: String,
    /// Project-key signature over the PoP bytes, hex (128 chars).
    pub sig: String,
}

/// `mesh.register` params (child to user daemon).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegisterRequest {
    /// Always [`PROTOCOL_TAG`].
    #[serde(default = "protocol_tag")]
    pub protocol: String,
    /// Registering role.
    pub role: MeshRole,
    /// Project ULID.
    pub project_id: String,
    /// Project public key, hex (64 chars).
    pub project_pubkey: String,
    /// The certificate the child holds; `None` on first boot.
    #[serde(default)]
    pub cert: Option<ProjectCert>,
    /// Mesh addresses the child answers to (the project id in Phase 2).
    #[serde(default)]
    pub addresses: Vec<String>,
    /// Chain topic prefixes the child serves (`chain/<project_id>/`).
    #[serde(default)]
    pub topic_prefixes: Vec<String>,
    /// Kernel version.
    pub version: String,
    /// Kernel build sha.
    #[serde(default)]
    pub build_sha: String,
    /// Child process id.
    pub pid: u32,
    /// The child's own socket path.
    pub socket: String,
    /// Capabilities the child offers (`anchor`, `subscribe`).
    #[serde(default)]
    pub features: Vec<String>,
    /// The spawn nonce from `spawn.json`; absent for a child the supervisor
    /// did not start (refused in Phase 2).
    #[serde(default)]
    pub spawn_nonce: Option<String>,
    /// Proof of possession of the project key.
    pub nonce_reply: NonceReply,
}

/// `mesh.register` result (user daemon to child).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegisterAck {
    /// True on success.
    pub ok: bool,
    /// Session id (ULID) for heartbeat and unregister.
    pub session: String,
    /// The certificate the user daemon issued or confirmed.
    #[serde(default)]
    pub cert: Option<ProjectCert>,
    /// Addresses the user daemon accepted.
    #[serde(default)]
    pub accepted: Vec<String>,
    /// Protocol range the user daemon speaks.
    pub proto: ProtoRange,
    /// Heartbeat interval; a session expires after three missed beats.
    pub heartbeat_secs: u64,
    /// Machine certificate (Phase 3); always `None` in Phase 2.
    #[serde(default)]
    pub machine_cert: Option<serde_json::Value>,
}

/// What a child is busy with (used for idle stop).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Busy {
    /// Running agents.
    #[serde(default)]
    pub agents: u32,
    /// Running workloads.
    #[serde(default)]
    pub workloads: u32,
    /// Open streams.
    #[serde(default)]
    pub streams: u32,
}

impl Busy {
    /// True when nothing is running.
    pub fn is_idle(&self) -> bool {
        self.agents == 0 && self.workloads == 0 && self.streams == 0
    }
}

/// Activity snapshot inside a heartbeat.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    /// Unix seconds of the last non-status, non-health RPC.
    #[serde(default)]
    pub last_activity_unix: u64,
    /// Current load.
    #[serde(default)]
    pub busy: Busy,
}

/// `mesh.heartbeat` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    /// Session from [`RegisterAck`].
    pub session: String,
    /// Activity since the last beat.
    #[serde(default)]
    pub activity: Activity,
}

/// `mesh.unregister` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnregisterRequest {
    /// Session to end.
    pub session: String,
    /// Why (`shutdown`, `idle`, ...).
    #[serde(default)]
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> RegisterRequest {
        RegisterRequest {
            protocol: PROTOCOL_TAG.into(),
            role: MeshRole::Project,
            project_id: "01JB8Z3Q0V6X9KQ4M2N7T5R1WD".into(),
            project_pubkey: "ab".repeat(32),
            cert: None,
            addresses: vec!["01JB8Z3Q0V6X9KQ4M2N7T5R1WD".into()],
            topic_prefixes: vec!["chain/01JB8Z3Q0V6X9KQ4M2N7T5R1WD/".into()],
            version: "0.8.2".into(),
            build_sha: "deadbeef".into(),
            pid: 4242,
            socket: "/Users/x/.weftos/run/01JB8Z3Q0V6X9KQ4M2N7T5R1WD/kernel.sock".into(),
            features: vec!["anchor".into(), "subscribe".into()],
            spawn_nonce: Some("n0".into()),
            nonce_reply: NonceReply {
                nonce: "00ff".into(),
                sig: "cd".repeat(64),
            },
        }
    }

    #[test]
    fn register_round_trips_with_the_plan_field_names() {
        let v = serde_json::to_value(req()).unwrap();
        assert_eq!(v["protocol"], "mesh-local/1");
        assert_eq!(v["role"], "project");
        assert_eq!(v["cert"], serde_json::Value::Null);
        assert_eq!(v["nonce_reply"]["nonce"], "00ff");
        let back: RegisterRequest = serde_json::from_value(v).unwrap();
        assert_eq!(back, req());
    }

    #[test]
    fn register_defaults_tolerate_a_sparse_message() {
        let v = serde_json::json!({
            "role": "project", "project_id": "p", "project_pubkey": "k",
            "version": "1", "pid": 1, "socket": "/s",
            "nonce_reply": {"nonce": "n", "sig": "s"}
        });
        let r: RegisterRequest = serde_json::from_value(v).unwrap();
        assert_eq!(r.protocol, PROTOCOL_TAG);
        assert!(r.cert.is_none() && r.spawn_nonce.is_none() && r.addresses.is_empty());
    }

    #[test]
    fn ack_and_heartbeat_round_trip() {
        let ack = RegisterAck {
            ok: true,
            session: "01S".into(),
            cert: None,
            accepted: vec!["p".into()],
            proto: ProtoRange { current: 1, min: 1 },
            heartbeat_secs: 15,
            machine_cert: None,
        };
        let back: RegisterAck = serde_json::from_str(&serde_json::to_string(&ack).unwrap()).unwrap();
        assert_eq!(back, ack);
        let hb: HeartbeatRequest = serde_json::from_str(
            r#"{"session":"s","activity":{"last_activity_unix":9,"busy":{"agents":1}}}"#,
        )
        .unwrap();
        assert_eq!(hb.activity.busy, Busy { agents: 1, workloads: 0, streams: 0 });
        assert!(!hb.activity.busy.is_idle());
        assert!(Busy::default().is_idle());
        let bare: HeartbeatRequest = serde_json::from_str(r#"{"session":"s"}"#).unwrap();
        assert_eq!(bare.activity, Activity::default());
    }

    #[test]
    fn proto_constant_matches_the_tag() {
        assert_eq!(PROTOCOL_TAG, format!("mesh-local/{PROTO_MESH_LOCAL}"));
    }
}
