//! The mesh-local/1 server on the unix socket (plan 1.2).
//!
//! One task per connection. After the peer-credential read the state machine
//! is: `hello` (5 s) -> `hello_ack` (negotiated protocol, per-connection
//! single-use challenge, machine-key proof bound to the client's nonce and the
//! uid read from the credential) -> either an admin connection (no keys, admin
//! verbs and read-only verbs) or `register` (challenge signature, binding,
//! certificate, registry) -> the registered loop.
//!
//! The service holds no governance: nothing here evaluates a policy. The
//! only verdict path is `verdicts.rs`, which forwards to the cluster owner.
//!
//! Frames are read by a dedicated task into a channel, so the select loops
//! below only race cancel-safe receivers (the line reader is not cancel-safe).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use clawft_mesh_local::framing::{write_frame, FrameError, FrameReader, HELLO_DEADLINE};
use clawft_mesh_local::peer::{PeerError, PeerIdentity, UnixPeer};
use clawft_mesh_local::proto::{
    hello_signing_bytes, negotiate_features, negotiate_service, ErrorBody, ErrorKind, Frame,
    HelloAck, Message, Role,
};
use clawft_mesh_local::{cert::LEEWAY_S, Principal, UserCert};
use ed25519_dalek::Signer;
use rand::RngCore;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch};

use crate::registry::Registration;
use crate::state::{unix_now, ServiceState};

/// How long an unregistered or admin connection may stay silent.
pub const IDLE_BEFORE_REGISTER: Duration = Duration::from_secs(30);
/// How long one frame write may take before the connection is dropped.
pub const WRITE_DEADLINE: Duration = Duration::from_secs(10);

/// Features this service offers (intersected with the client's).
pub const FEATURES: &[&str] = &["verdicts", "journal.head", "admin"];

/// Reads the peer credential of an accepted connection. Production uses
/// [`real_peer_source`]; tests inject identities for a second uid.
pub type PeerSource =
    Arc<dyn Fn(&UnixStream) -> Result<Arc<dyn PeerIdentity>, PeerError> + Send + Sync>;

/// The real `SO_PEERCRED` / `LOCAL_PEERCRED` reader (fails closed on error).
pub fn real_peer_source() -> PeerSource {
    Arc::new(|s| UnixPeer::from_stream(s).map(|p| Arc::new(p) as Arc<dyn PeerIdentity>))
}

/// What the loop should do after handling one frame.
pub(crate) enum Step {
    Continue,
    Close,
    Registered(Box<(mpsc::Receiver<Frame>, watch::Receiver<Option<String>>)>),
}

pub(crate) type Inbound = Result<Frame, FrameError>;

/// Accept connections until the task is aborted. Connection tasks live in a
/// `JoinSet`, so aborting this future also ends every connection (and drops
/// their hold on the state, including the journal lock).
pub async fn serve_local(st: Arc<ServiceState>, listener: UnixListener, peers: PeerSource) {
    let mut conns = tokio::task::JoinSet::new();
    loop {
        while conns.try_join_next().is_some() {}
        match listener.accept().await {
            Ok((stream, _)) => {
                let peer = match peers(&stream) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(error = %e, "refusing a connection with no peer credential");
                        continue;
                    }
                };
                let st = Arc::clone(&st);
                conns.spawn(async move { handle_connection(st, stream, peer).await });
            }
            Err(e) => {
                tracing::warn!(error = %e, "mesh-local accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// Serve one connection whose peer identity is already known.
pub async fn handle_connection(st: Arc<ServiceState>, stream: UnixStream, peer: Arc<dyn PeerIdentity>) {
    if let Err(e) = run(&st, stream, peer.as_ref()).await {
        tracing::debug!(error = %e, "mesh-local connection ended");
    }
}

pub(crate) struct Conn {
    pub st: Arc<ServiceState>,
    wr: OwnedWriteHalf,
    pub principal: Principal,
    pub uid: u32,
    pub role: Role,
    /// Admin role requested *and* the uid is root or in `admin_uids`.
    pub admin: bool,
    pub challenge: [u8; 32],
    pub challenge_used: bool,
    pub pid: u32,
    pub exe: String,
    pub conn_id: u64,
    pub reg: Option<Arc<Registration>>,
    pub cert: Option<UserCert>,
}

impl Conn {
    pub(crate) async fn send_frame(&mut self, f: Frame) -> Result<(), FrameError> {
        match tokio::time::timeout(WRITE_DEADLINE, write_frame(&mut self.wr, &f)).await {
            Ok(r) => r,
            Err(_) => Err(FrameError::Deadline),
        }
    }

    pub(crate) async fn send(&mut self, id: Option<u64>, msg: Message) -> Result<(), FrameError> {
        self.send_frame(Frame { id, msg }).await
    }

    pub(crate) async fn error(
        &mut self,
        id: Option<u64>,
        kind: ErrorKind,
        message: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Result<(), FrameError> {
        self.send(id, Message::Error(ErrorBody::new(kind, message, remedy))).await
    }

    /// Send an error and report whether the connection must close afterwards.
    pub(crate) async fn error_step(
        &mut self,
        id: Option<u64>,
        kind: ErrorKind,
        message: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Result<Step, FrameError> {
        let fatal = kind.is_fatal();
        self.error(id, kind, message, remedy).await?;
        Ok(if fatal { Step::Close } else { Step::Continue })
    }

    fn cleanup(&mut self) {
        if let Some(reg) = self.reg.take() {
            self.st.registry.unregister(&reg.user_id, self.conn_id);
        }
        self.st.verdicts.forget_conn(self.conn_id);
    }

    async fn bad_frame(&mut self, e: FrameError) {
        let _ = self.error(None, ErrorKind::BadRequest, format!("bad frame: {e}"), "").await;
    }
}

fn spawn_reader(mut rd: FrameReader<OwnedReadHalf>) -> (mpsc::Receiver<Inbound>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(32);
    let h = tokio::spawn(async move {
        loop {
            match rd.read_frame().await {
                Ok(Some(f)) => {
                    if tx.send(Ok(f)).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    break;
                }
            }
        }
    });
    (rx, h)
}

async fn run(st: &Arc<ServiceState>, stream: UnixStream, peer: &dyn PeerIdentity) -> Result<(), FrameError> {
    // Fail closed: no credential, no conversation.
    let principal = match peer.principal() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "peer credential unavailable; closing");
            return Ok(());
        }
    };
    let Principal::Uid(uid) = principal else {
        tracing::warn!("non-uid principal on a unix socket; closing");
        return Ok(());
    };
    let (rd, mut wr) = stream.into_split();
    let Some(_slot) = st.limiter.acquire_conn(&principal) else {
        let m = ErrorBody::new(ErrorKind::RateLimited, "too many connections", "close idle connections");
        return write_frame(&mut wr, &Frame::new(Message::Error(m))).await;
    };
    let mut rd = FrameReader::new(rd);

    let Some(first) = rd.read_frame_within(HELLO_DEADLINE).await? else { return Ok(()) };
    let Message::Hello { proto_min, proto_max, features, role, build_sha, exe, pid, client_nonce } =
        first.msg
    else {
        let m = ErrorBody::new(ErrorKind::Forbidden, "expected hello", "send hello first");
        return write_frame(&mut wr, &Frame { id: first.id, msg: Message::Error(m) }).await;
    };
    let proto = match negotiate_service(proto_min, proto_max) {
        Ok(p) => p,
        Err(mut m) => {
            m.service.sha = Some(st.cfg.build_sha.clone());
            m.client.sha = Some(build_sha).filter(|s| !s.is_empty());
            let body = Message::Error(ErrorBody::proto_mismatch(&m));
            return write_frame(&mut wr, &Frame::new(body)).await;
        }
    };
    let admin = role == Role::Admin;
    if admin && !st.cfg.is_admin(uid) {
        let m = ErrorBody::new(
            ErrorKind::Forbidden,
            format!("uid {uid} is not an administrator of this service"),
            "ask an admin (root or a uid in admin_uids) to run this command",
        );
        return write_frame(&mut wr, &Frame::new(Message::Error(m))).await;
    }

    let mut challenge = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut challenge);
    let machine_sig = st
        .machine_key
        .sign(&hello_signing_bytes(&client_nonce, &challenge, &st.machine_pubkey, uid))
        .to_bytes();
    let own: Vec<String> = FEATURES.iter().map(|s| (*s).to_string()).collect();
    let ack = HelloAck {
        proto,
        features: negotiate_features(&own, &features),
        node_id: st.node_id.clone(),
        machine_pubkey: st.machine_pubkey,
        service_build_sha: st.cfg.build_sha.clone(),
        deprecated_below: None,
        uid,
        challenge,
        machine_sig,
    };
    write_frame(&mut wr, &Frame::new(Message::HelloAck(ack))).await?;

    let mut conn = Conn {
        st: Arc::clone(st),
        wr,
        principal,
        uid,
        role,
        admin,
        challenge,
        challenge_used: false,
        pid,
        exe,
        conn_id: st.conn_seq.fetch_add(1, Ordering::Relaxed),
        reg: None,
        cert: None,
    };
    let (mut rx, reader) = spawn_reader(rd);
    let result = conn.drive(&mut rx).await;
    reader.abort();
    conn.cleanup();
    result
}

impl Conn {
    async fn drive(&mut self, rx: &mut mpsc::Receiver<Inbound>) -> Result<(), FrameError> {
        loop {
            let item = match tokio::time::timeout(IDLE_BEFORE_REGISTER, rx.recv()).await {
                Ok(Some(i)) => i,
                Ok(None) | Err(_) => return Ok(()),
            };
            let frame = match item {
                Ok(f) => f,
                Err(e) => {
                    self.bad_frame(e).await;
                    return Ok(());
                }
            };
            let id = frame.id;
            let step = match frame.msg {
                Message::Register(req) if self.role == Role::User => self.register(id, req).await?,
                Message::Register(_) => {
                    self.error_step(id, ErrorKind::Forbidden, "admin connections do not register", "").await?
                }
                m => self.handle_common(id, m).await?,
            };
            match step {
                Step::Continue => {}
                Step::Close => return Ok(()),
                Step::Registered(b) => {
                    let (out, killed) = *b;
                    return self.registered_loop(rx, out, killed).await;
                }
            }
        }
    }

    async fn registered_loop(
        &mut self,
        rx: &mut mpsc::Receiver<Inbound>,
        mut out: mpsc::Receiver<Frame>,
        mut killed: watch::Receiver<Option<String>>,
    ) -> Result<(), FrameError> {
        loop {
            let deadline = self.cert_deadline();
            tokio::select! {
                item = rx.recv() => match item {
                    None => return Ok(()),
                    Some(Err(e)) => { self.bad_frame(e).await; return Ok(()); }
                    Some(Ok(f)) => {
                        let id = f.id;
                        if let Step::Close = self.handle_registered(id, f.msg).await? {
                            return Ok(());
                        }
                    }
                },
                Some(frame) = out.recv() => self.send_frame(frame).await?,
                _ = killed.changed() => {
                    let why = killed.borrow().clone().unwrap_or_else(|| "registration closed by the service".into());
                    let _ = self.error(None, ErrorKind::Forbidden, why, "re-register").await;
                    return Ok(());
                }
                _ = tokio::time::sleep_until(deadline) => {
                    let _ = self.error(None, ErrorKind::Forbidden,
                        "certificate expired without renewal", "send renew at half the lifetime").await;
                    return Ok(());
                }
            }
        }
    }

    /// When the registration lapses if the certificate is not renewed.
    fn cert_deadline(&self) -> tokio::time::Instant {
        let not_after = self.cert.as_ref().map_or(0, |c| c.not_after);
        tokio::time::Instant::now() + Duration::from_secs(lapse_in(not_after, unix_now()))
    }

    /// Verbs any connection may use once hello is done, and admin verbs.
    pub(crate) async fn handle_common(&mut self, id: Option<u64>, m: Message) -> Result<Step, FrameError> {
        match m {
            Message::Ping {} => self.send(id, Message::Pong {}).await?,
            Message::Pong {} => {}
            Message::Bye {} => return Ok(Step::Close),
            Message::Status {} => {
                let data = self.st.status_json(self.uid, self.admin);
                self.send(id, Message::Reply { data }).await?;
            }
            Message::PeersList {} => {
                let data = self.peers_json();
                self.send(id, Message::Reply { data }).await?;
            }
            Message::FactsGet {} => {
                let data = self.st.facts.current().unwrap_or(serde_json::Value::Null);
                self.send(id, Message::Reply { data }).await?;
            }
            m if crate::admin::is_admin_verb(&m) => {
                if !self.admin {
                    // Refused but not fatal: the connection keeps its other rights.
                    self.error(id, ErrorKind::Forbidden, "admin verb requires an admin connection", "").await?;
                    return Ok(Step::Continue);
                }
                return self.admin_verb(id, m).await;
            }
            _ => {
                let why = if self.reg.is_none() && self.role == Role::User {
                    "register first"
                } else {
                    "not supported on this connection"
                };
                return self.error_step(id, ErrorKind::Unsupported, why, "").await;
            }
        }
        Ok(Step::Continue)
    }

    fn peers_json(&self) -> serde_json::Value {
        let rt = self.st.router.runtime();
        let peers = rt.as_ref().map(|r| r.peer_ids()).unwrap_or_default();
        let known = rt.as_ref().map(|r| r.discover_peers()).unwrap_or_default();
        serde_json::json!({
            "connected": peers,
            "known": known.iter().map(|(n, a)| serde_json::json!({"node_id": n, "addr": a})).collect::<Vec<_>>(),
        })
    }
}

/// Seconds until a registration whose certificate expires at `not_after`
/// lapses unrenewed: expiry plus the verifier leeway.
pub(crate) fn lapse_in(not_after: u64, now: u64) -> u64 {
    not_after.saturating_add(LEEWAY_S).saturating_sub(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_lapses_at_expiry_plus_leeway() {
        assert_eq!(lapse_in(1000, 900), 100 + LEEWAY_S);
        assert_eq!(lapse_in(1000, 1000 + LEEWAY_S), 0);
        assert_eq!(lapse_in(1000, u64::MAX), 0, "never underflows");
    }
}
