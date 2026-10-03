//! mesh-local/1 wire messages, version negotiation and the register signature
//! (plan 1.2). One JSON object per line: `{"t": "<type>", "id": <u64>?, ...}`.
//!
//! Unknown fields are ignored on every struct, and an unknown message `t`
//! decodes to [`Message::Unknown`] rather than failing the connection.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cert::UserCert;
use crate::hexser::{hex32, hex64};
use crate::peer::Principal;

/// Oldest protocol version this build speaks.
pub const PROTO_MIN: u32 = 1;
/// Newest protocol version this build speaks.
pub const PROTO_MAX: u32 = 2;

/// First protocol version whose `deliver` carries the service-stamped
/// [`DeliverOrigin`] (ADR-103 amendment, ADR-106 section 5.3). On a lower
/// negotiated version the field does not exist and every delivery is
/// [`DeliverOrigin::Unadmitted`].
pub const PROTO_ORIGIN: u32 = 2;

/// Topic prefixes reserved for the cluster-owner (steward) daemon. The service
/// lets only that registration send them and routes them only to it; no other
/// tenant may send them or claim a prefix that overlaps them. `AdmittedPeer`
/// vouches for a machine, so without this any tenant on a member could speak
/// for it (ADR-106 5.3).
pub const RESERVED_TOPIC_PREFIXES: [&str; 3] = ["mesh.cog.", "mesh.artifact.", "mesh.licence."];

/// True when `topic` is under a reserved prefix.
pub fn is_reserved_topic(topic: &str) -> bool {
    RESERVED_TOPIC_PREFIXES.iter().any(|p| topic.starts_with(p))
}

/// True when claiming `prefix` could capture (or be captured by) a reserved topic.
pub fn overlaps_reserved(prefix: &str) -> bool {
    RESERVED_TOPIC_PREFIXES.iter().any(|r| prefix.starts_with(r) || r.starts_with(prefix))
}

/// Domain separator of the service's hello proof.
pub const HELLO_DOMAIN: &[u8] = b"weftos/mesh-local/hello/v1\0";

/// Ids at or above this bit are originated by the service (for example
/// `verdict.request`); the client allocates ids below it. The two namespaces
/// cannot collide, so a service-originated id never matches a pending request.
pub const SERVICE_ID_FLAG: u64 = 1 << 63;

/// Domain separator of the register signature.
pub const REGISTER_DOMAIN: &[u8] = b"weftos/mesh-local/register/v1\0";

/// A line on the wire: optional correlation id plus the message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(flatten)]
    pub msg: Message,
}

impl Frame {
    pub fn new(msg: Message) -> Self {
        Self { id: None, msg }
    }
    pub fn with_id(id: u64, msg: Message) -> Self {
        Self { id: Some(id), msg }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Admin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    ProtoMismatch,
    BindConflict,
    BindPending,
    AddressInUse,
    Forbidden,
    BadSig,
    RateLimited,
    ScopeRequired,
    UnknownScope,
    Unsupported,
    /// `send` named a node that is not connected.
    PeerUnreachable,
    /// The request was well-formed JSON but not a valid request.
    BadRequest,
    /// A kind this build does not know; treated as non-fatal.
    #[serde(other)]
    Unknown,
}

impl ErrorKind {
    /// Fatal kinds are followed by a close from the service.
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            ErrorKind::ProtoMismatch
                | ErrorKind::BindConflict
                | ErrorKind::AddressInUse
                | ErrorKind::Forbidden
                | ErrorKind::BadSig
                | ErrorKind::RateLimited
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub kind: ErrorKind,
    pub message: String,
    /// What the operator should do about it.
    #[serde(default)]
    pub remedy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ErrorBody {
    pub fn new(kind: ErrorKind, message: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self { kind, message: message.into(), remedy: remedy.into(), data: None }
    }

    pub fn proto_mismatch(m: &ProtoMismatch) -> Self {
        Self {
            kind: ErrorKind::ProtoMismatch,
            message: format!(
                "no common protocol version (service {}..={}, client {}..={})",
                m.service.min, m.service.max, m.client.min, m.client.max
            ),
            remedy: m.remedy().to_string(),
            data: serde_json::to_value(m).ok(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectBinding {
    pub project_id: String,
    #[serde(with = "hex32")]
    pub project_pubkey: [u8; 32],
    #[serde(with = "hex64")]
    pub cert_sig: [u8; 64],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Addresses {
    pub user_id: String,
    #[serde(default)]
    pub projects: Vec<ProjectBinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloAck {
    pub proto: u32,
    #[serde(default)]
    pub features: Vec<String>,
    pub node_id: String,
    #[serde(with = "hex32")]
    pub machine_pubkey: [u8; 32],
    #[serde(default)]
    pub service_build_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated_below: Option<u32>,
    /// What the service read from the peer credential.
    pub uid: u32,
    /// Per-connection, single-use register challenge.
    #[serde(with = "hex32")]
    pub challenge: [u8; 32],
    /// Ed25519 by the machine key over [`hello_signing_bytes`]: proof of key
    /// possession bound to this connection's nonce and challenge.
    #[serde(with = "hex64")]
    pub machine_sig: [u8; 64],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterReq {
    #[serde(with = "hex32")]
    pub user_pubkey: [u8; 32],
    #[serde(with = "hex64")]
    pub sig: [u8; 64],
    pub addresses: Addresses,
    #[serde(default)]
    pub topic_prefixes: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub build_sha: String,
    /// User ids (32 hex) or `"*"` whose daemons may send to this one through
    /// the service. Default empty: no other tenant may send here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accept_from: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindState {
    New,
    Existing,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Accepted {
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(default)]
    pub topic_prefixes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejected {
    pub what: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegisterAck {
    pub user_id: String,
    pub cert: UserCert,
    #[serde(default)]
    pub accepted: Accepted,
    #[serde(default)]
    pub rejected: Vec<Rejected>,
    pub bind: BindState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// Class the mesh admitted a peer as (mirrors the kernel's `PeerClass`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginClass {
    /// Verified full node.
    Node,
    /// Verified leaf device.
    Leaf,
    /// Anything else, including a class this build does not know.
    #[serde(other)]
    Other,
}

/// Where a delivery came from, stamped by the machine mesh service and by no
/// one else. A tenant can never supply it: the service writes it on every
/// `deliver` it sends and ignores any such field on what tenants send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeliverOrigin {
    /// A remote peer whose id admission verified, with the class it got.
    AdmittedPeer { node_id: String, class: OriginClass },
    /// Another tenant on this machine, through the service.
    LocalTenant,
    /// A peer admission did not verify (legacy, leaf-claimed, `observe`), a
    /// kind this build does not know, or no stamp at all.
    #[serde(other)]
    Unadmitted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deliver {
    pub source_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_cert: Option<UserCert>,
    pub scope: Scope,
    pub envelope_id: String,
    pub message: Value,
    /// Service-stamped origin; absent on protocol versions below
    /// [`PROTO_ORIGIN`] and from an older service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<DeliverOrigin>,
}

impl Deliver {
    /// The origin to act on, given the negotiated protocol version. The stamp
    /// is honoured only from [`PROTO_ORIGIN`] up; below it, or when absent,
    /// the delivery is [`DeliverOrigin::Unadmitted`]. The caller must also
    /// have passed the client's service peer check (a connected
    /// `MeshLocalClient` has); a field arriving any other way is not a stamp.
    pub fn effective_origin(&self, negotiated: u32) -> DeliverOrigin {
        if negotiated < PROTO_ORIGIN {
            return DeliverOrigin::Unadmitted;
        }
        self.origin.clone().unwrap_or(DeliverOrigin::Unadmitted)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerdictSubject {
    #[serde(rename = "peer.admit")]
    PeerAdmit,
    #[serde(rename = "cluster.join")]
    ClusterJoin,
    #[serde(rename = "publish")]
    Publish,
    #[serde(rename = "subscribe")]
    Subscribe,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    pub node_id: String,
    pub pubkey: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub genesis_hash: String,
    #[serde(default)]
    pub chain_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictRequest {
    pub subject: VerdictSubject,
    pub peer: PeerInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
}

/// Every mesh-local/1 message. The wire tag is the field `t`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Message {
    #[serde(rename = "hello")]
    Hello {
        proto_min: u32,
        proto_max: u32,
        #[serde(default)]
        features: Vec<String>,
        role: Role,
        #[serde(default)]
        build_sha: String,
        #[serde(default)]
        exe: String,
        #[serde(default)]
        pid: u32,
        /// Fresh random nonce; the service signs it (with its challenge) in
        /// `hello_ack.machine_sig` to prove it holds the machine key.
        #[serde(with = "hex32")]
        client_nonce: [u8; 32],
    },
    #[serde(rename = "hello_ack")]
    HelloAck(HelloAck),
    #[serde(rename = "error")]
    Error(ErrorBody),
    #[serde(rename = "register")]
    Register(RegisterReq),
    #[serde(rename = "register_ack")]
    RegisterAck(RegisterAck),
    #[serde(rename = "renew")]
    Renew {},
    /// Reply to `renew`.
    #[serde(rename = "cert")]
    Cert { cert: UserCert },
    #[serde(rename = "address.add")]
    AddressAdd(ProjectBinding),
    #[serde(rename = "address.remove")]
    AddressRemove(ProjectBinding),
    #[serde(rename = "subscribe")]
    Subscribe { prefix: String },
    #[serde(rename = "unsubscribe")]
    Unsubscribe { prefix: String },
    #[serde(rename = "ack")]
    Ack {},
    #[serde(rename = "send")]
    Send {
        dest: String,
        message: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },
    #[serde(rename = "deliver")]
    Deliver(Deliver),
    #[serde(rename = "verdict.request")]
    VerdictRequest(VerdictRequest),
    #[serde(rename = "verdict.reply")]
    VerdictReply {
        allow: bool,
        ttl_s: u64,
        #[serde(default)]
        reason: String,
        #[serde(default)]
        rule_hash: String,
    },
    #[serde(rename = "journal.head")]
    JournalHead {},
    #[serde(rename = "journal.head.reply")]
    JournalHeadReply { seq: u64, hash: String, ts: u64, sig: String },
    #[serde(rename = "status")]
    Status {},
    #[serde(rename = "peers.list")]
    PeersList {},
    #[serde(rename = "facts.get")]
    FactsGet {},
    /// Generic reply to the read-only verbs.
    #[serde(rename = "reply")]
    Reply { data: Value },
    #[serde(rename = "bindings.list")]
    BindingsList {},
    #[serde(rename = "bind.approve")]
    BindApprove {
        uid: u32,
        /// `user_id` (key fingerprint) of the pending key the admin looked at;
        /// the service refuses when the pending key is a different one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_id: Option<String>,
    },
    #[serde(rename = "bind.revoke")]
    BindRevoke { uid: u32, #[serde(default)] reason: String },
    #[serde(rename = "bind.rebind")]
    BindRebind {
        uid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_pubkey: Option<String>,
    },
    #[serde(rename = "peer.revoke")]
    PeerRevoke { node_id: String, #[serde(default)] reason: String },
    #[serde(rename = "peer.unrevoke")]
    PeerUnrevoke { node_id: String },
    #[serde(rename = "policy.set")]
    PolicySet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        admission: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cluster_owner_uid: Option<u32>,
    },
    /// Admin: re-verify the journal's hash chain and signatures.
    #[serde(rename = "journal.verify")]
    JournalVerify {},
    /// Admin: acknowledge a quarantined journal tail (`weaver mesh journal
    /// verify --accept-truncate`). `quarantine_seq` names the pending
    /// `journal.quarantine` record; `floor` can only raise the serial floor.
    #[serde(rename = "journal.accept_truncate")]
    JournalAcceptTruncate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quarantine_seq: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        floor: Option<u64>,
    },
    #[serde(rename = "ping")]
    Ping {},
    #[serde(rename = "pong")]
    Pong {},
    #[serde(rename = "bye")]
    Bye {},
    /// A message type this build does not know.
    #[serde(other)]
    Unknown,
}

/// Bytes the machine key signs in `hello_ack`: domain, the client's nonce, the
/// service's challenge, the machine public key, then the tagged principal the
/// service read from the peer credential (`ack.uid`). Binding the uid stops a
/// relay that holds the service uid from presenting someone else's uid.
pub fn hello_signing_bytes(
    client_nonce: &[u8; 32],
    challenge: &[u8; 32],
    machine_pubkey: &[u8; 32],
    uid: u32,
) -> Vec<u8> {
    let mut b = Vec::with_capacity(HELLO_DOMAIN.len() + 96 + 7);
    b.extend_from_slice(HELLO_DOMAIN);
    b.extend_from_slice(client_nonce);
    b.extend_from_slice(challenge);
    b.extend_from_slice(machine_pubkey);
    b.extend_from_slice(&Principal::Uid(uid).signing_bytes());
    b
}

/// Verify the service's key-possession proof for this connection.
pub fn verify_hello_proof(ack: &HelloAck, client_nonce: &[u8; 32]) -> bool {
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&ack.machine_pubkey) else {
        return false;
    };
    let bytes = hello_signing_bytes(client_nonce, &ack.challenge, &ack.machine_pubkey, ack.uid);
    key.verify_strict(&bytes, &ed25519_dalek::Signature::from_bytes(&ack.machine_sig)).is_ok()
}

/// True for messages that can answer a request. Only these are routed to a
/// waiting `request()`; anything else carrying an id goes to the event stream.
pub fn is_reply_class(m: &Message) -> bool {
    matches!(
        m,
        Message::Ack {}
            | Message::Reply { .. }
            | Message::Error(_)
            | Message::Cert { .. }
            | Message::Pong {}
            | Message::JournalHeadReply { .. }
            | Message::RegisterAck(_)
            | Message::HelloAck(_)
    )
}

/// Bytes the user key signs to register: domain, challenge, the tagged and
/// length-prefixed principal, then the length-prefixed node id text.
pub fn register_signing_bytes(
    challenge: &[u8; 32],
    principal: &Principal,
    node_id: &str,
) -> Vec<u8> {
    let mut b = Vec::with_capacity(REGISTER_DOMAIN.len() + 32 + 8 + node_id.len());
    b.extend_from_slice(REGISTER_DOMAIN);
    b.extend_from_slice(challenge);
    b.extend_from_slice(&principal.signing_bytes());
    let node = &node_id.as_bytes()[..node_id.len().min(u16::MAX as usize)];
    b.extend_from_slice(&(node.len() as u16).to_be_bytes());
    b.extend_from_slice(node);
    b
}

/// Verify a register signature against the request's own user key.
pub fn verify_register_sig(
    req: &RegisterReq,
    challenge: &[u8; 32],
    principal: &Principal,
    node_id: &str,
) -> bool {
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&req.user_pubkey) else {
        return false;
    };
    let bytes = register_signing_bytes(challenge, principal, node_id);
    key.verify_strict(&bytes, &ed25519_dalek::Signature::from_bytes(&req.sig)).is_ok()
}

pub use crate::negotiate::{
    negotiate, negotiate_features, negotiate_service, ProtoMismatch, ServiceRecord, VersionRange,
};
