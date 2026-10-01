//! Peer credentials behind a trait, so authorization never depends on how the
//! credential was obtained (kernel peer credential, test injection, and later
//! a Windows token).

use serde::{Deserialize, Serialize};

/// The authenticated local principal on the other end of a connection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Principal {
    Uid(u32),
    Sid(String),
}

impl Principal {
    /// The bytes bound into the register signature (plan 1.2).
    ///
    /// Tagged and length-prefixed (`tag(1) || len(u16 BE) || bytes`) so a uid
    /// and a SID can never produce the same bytes.
    pub fn signing_bytes(&self) -> Vec<u8> {
        let (tag, body): (u8, Vec<u8>) = match self {
            Principal::Uid(u) => (1, u.to_be_bytes().to_vec()),
            Principal::Sid(s) => (2, s.as_bytes()[..s.len().min(u16::MAX as usize)].to_vec()),
        };
        let mut out = Vec::with_capacity(3 + body.len());
        out.push(tag);
        out.extend_from_slice(&(body.len() as u16).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }
}

/// Credentials of a connected peer. `gid` and `pid` are best effort.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerCreds {
    pub principal: Principal,
    pub gid: Option<u32>,
    pub pid: Option<u32>,
}

impl PeerCreds {
    pub fn uid(uid: u32) -> Self {
        Self { principal: Principal::Uid(uid), gid: None, pid: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerError {
    #[error("peer credentials unavailable: {0}")]
    Unavailable(String),
    #[error("peer identity is not supported on this platform")]
    Unsupported,
}

/// Source of a connection's authenticated principal. Fail closed: an error
/// means the caller refuses the connection.
pub trait PeerIdentity: Send + Sync {
    fn credentials(&self) -> Result<PeerCreds, PeerError>;

    fn principal(&self) -> Result<Principal, PeerError> {
        self.credentials().map(|c| c.principal)
    }
}

/// Fixed credentials for tests (also used by downstream crates' tests).
/// Only compiled with the `testing` feature: production code can never inject
/// an identity.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Clone)]
pub struct InjectedPeer(pub PeerCreds);

#[cfg(any(test, feature = "testing"))]
impl InjectedPeer {
    pub fn uid(uid: u32) -> Self {
        Self(PeerCreds::uid(uid))
    }
}

#[cfg(any(test, feature = "testing"))]
impl PeerIdentity for InjectedPeer {
    fn credentials(&self) -> Result<PeerCreds, PeerError> {
        Ok(self.0.clone())
    }
}

/// Credentials read from a unix stream's `SO_PEERCRED` / `LOCAL_PEERCRED`,
/// captured once at construction.
///
/// Semantics: the kernel records the credentials of the process that called
/// `connect()` (client side) or `listen()` (server side) when the socket was
/// set up, not of whichever process later reads or writes the descriptor. A
/// descriptor passed to another process keeps the original identity, and the
/// snapshot is never refreshed, so a uid that changes mid-connection (setuid)
/// is not noticed. Authorization decisions must therefore be made per
/// connection and re-checked on reconnect, never cached across connections.
#[cfg(unix)]
#[derive(Debug, Clone)]
pub struct UnixPeer(PeerCreds);

#[cfg(unix)]
impl UnixPeer {
    /// Read the peer credential; any failure is an error (fail closed).
    pub fn from_stream(stream: &tokio::net::UnixStream) -> Result<Self, PeerError> {
        let cred = stream.peer_cred().map_err(|e| PeerError::Unavailable(e.to_string()))?;
        Ok(Self(PeerCreds {
            principal: Principal::Uid(cred.uid()),
            gid: Some(cred.gid()),
            pid: cred.pid().and_then(|p| u32::try_from(p).ok()),
        }))
    }
}

#[cfg(unix)]
impl PeerIdentity for UnixPeer {
    fn credentials(&self) -> Result<PeerCreds, PeerError> {
        Ok(self.0.clone())
    }
}

/// Windows placeholder: always refuses (ADR-103 section 6).
#[cfg(not(unix))]
#[derive(Debug, Clone, Default)]
pub struct UnsupportedPeer;

#[cfg(not(unix))]
impl PeerIdentity for UnsupportedPeer {
    fn credentials(&self) -> Result<PeerCreds, PeerError> {
        Err(PeerError::Unsupported)
    }
}
