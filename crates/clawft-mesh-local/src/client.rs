//! `MeshLocalClient`: connect to the machine mesh service, verify the server,
//! negotiate, register with a user key, then correlate requests and replies.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signer, SigningKey};
use rand::RngCore;
use serde_json::Value;
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};

use crate::addr::{AddrError, WeftAddr};
use crate::cert::{node_id_from_pubkey, CertError, UserCert};
use crate::framing::{write_frame, FrameError, FrameReader, HELLO_DEADLINE};
use crate::hexser;
use crate::peer::{own_uid, PeerError, PeerIdentity, Principal, UnixPeer};
use crate::pin::check_pin;
use crate::proto::{
    is_reply_class, negotiate_features, register_signing_bytes, verify_hello_proof, Addresses,
    ErrorBody, Frame, HelloAck, Message, ProjectBinding, RegisterAck, RegisterReq, Role,
    ServiceRecord, PROTO_MAX, PROTO_MIN, SERVICE_ID_FLAG,
};

/// Capacity of the unsolicited-event queue.
pub const EVENT_QUEUE: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("framing: {0}")]
    Frame(#[from] FrameError),
    #[error("server error {:?}: {} ({})", .0.kind, .0.message, .0.remedy)]
    Server(ErrorBody),
    #[error("could not read the server's peer credential: {0}")]
    PeerCred(#[from] PeerError),
    #[error("server runs as {got:?}, expected root or uid {expected} from service.json; refusing")]
    ServerUid { got: Principal, expected: u32 },
    #[error(
        "machine_key_changed: pinned {pinned} but the service presented {presented}; verify \
         out of band, then remove the pin file to re-pin (a `weaver mesh trust` command \
         arrives with Phase 3 package S)"
    )]
    MachineKeyChanged { pinned: String, presented: String },
    #[error("{} is corrupt; remove it to re-pin", .0.display())]
    PinCorrupt(PathBuf),
    #[error("the service reports uid {ack} for this connection but this process runs as {own}")]
    UidMismatch { ack: u32, own: u32 },
    #[error("the service did not prove possession of its machine key (bad or replayed hello proof)")]
    BadServerProof,
    #[error("bad server hello: {0}")]
    BadServerHello(String),
    #[error("server chose protocol {chosen}, outside our range {min}..={max}")]
    ProtoOutOfRange { chosen: u32, min: u32, max: u32 },
    #[error("certificate: {0}")]
    Cert(#[from] CertError),
    #[error("address: {0}")]
    Addr(#[from] AddrError),
    #[error("deadline exceeded")]
    Timeout,
    #[error("connection closed: {0}")]
    Closed(String),
    #[error("unexpected reply: {0}")]
    Unexpected(String),
}

impl ClientError {
    /// Worth retrying with backoff (transport trouble), as opposed to a
    /// verification or protocol failure that retrying cannot fix.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ClientError::Io(_) | ClientError::Timeout | ClientError::Closed(_)
        ) || matches!(self, ClientError::Frame(FrameError::Io(_) | FrameError::Truncated))
    }
}

#[derive(Clone)]
pub struct ClientConfig {
    pub socket_path: PathBuf,
    /// The service's public record (`service.json`).
    pub service: ServiceRecord,
    /// Where the machine key is pinned (`~/.weftos/mesh/machine.pub`). Written
    /// on first contact, compared afterwards. `None` pins to `service` only.
    pub machine_pin: Option<PathBuf>,
    pub role: Role,
    pub features: Vec<String>,
    pub build_sha: String,
    pub exe: String,
    pub proto: (u32, u32),
    /// Deadline for each handshake step and each request.
    pub deadline: Duration,
    /// Test seam: credentials to use for the server instead of reading
    /// `peer_cred` from the socket. Absent from production builds, which
    /// always use the real peer credential.
    #[cfg(feature = "testing")]
    pub server_peer: Option<Arc<dyn PeerIdentity>>,
    /// Test seam: the uid this process claims to run as (the injected server
    /// credentials in tests are not our real euid).
    #[cfg(feature = "testing")]
    pub own_uid: Option<u32>,
}

impl ClientConfig {
    pub fn new(socket_path: impl Into<PathBuf>, service: ServiceRecord) -> Self {
        Self {
            socket_path: socket_path.into(),
            service,
            machine_pin: None,
            role: Role::User,
            features: Vec::new(),
            build_sha: String::new(),
            exe: String::new(),
            proto: (PROTO_MIN, PROTO_MAX),
            deadline: HELLO_DEADLINE,
            #[cfg(feature = "testing")]
            server_peer: None,
            #[cfg(feature = "testing")]
            own_uid: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RegisterParams {
    pub projects: Vec<ProjectBinding>,
    pub topic_prefixes: Vec<String>,
    pub capabilities: Vec<String>,
    pub version: String,
    /// Tenants allowed to send to this daemon (user ids or `*`).
    pub accept_from: Vec<String>,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Message>>>>;

pub struct MeshLocalClient {
    writer: Arc<AsyncMutex<OwnedWriteHalf>>,
    pending: Pending,
    next_id: AtomicU64,
    events: mpsc::Receiver<Frame>,
    dropped_events: Arc<AtomicU64>,
    close_reason: Arc<Mutex<Option<String>>>,
    reader: tokio::task::JoinHandle<()>,
    hello_ack: HelloAck,
    features: Vec<String>,
    register_ack: RegisterAck,
    cert: UserCert,
    deadline: Duration,
}

pub fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

async fn within<T>(d: Duration, f: impl std::future::Future<Output = T>) -> Result<T, ClientError> {
    tokio::time::timeout(d, f).await.map_err(|_| ClientError::Timeout)
}

fn server_error(msg: Message) -> ClientError {
    match msg {
        Message::Error(e) => ClientError::Server(e),
        other => ClientError::Unexpected(format!("{other:?}")),
    }
}

impl MeshLocalClient {
    /// Connect, verify the server, negotiate, register, and start the reader.
    pub async fn connect_and_register(
        cfg: &ClientConfig,
        user_key: &SigningKey,
        params: &RegisterParams,
    ) -> Result<Self, ClientError> {
        let stream = within(cfg.deadline, UnixStream::connect(&cfg.socket_path)).await??;

        // Anti-squat: the server must be root or the service account.
        // Captured once, at connect time (see `UnixPeer`).
        #[cfg(feature = "testing")]
        let injected = cfg.server_peer.as_ref().map(|p| p.principal());
        #[cfg(not(feature = "testing"))]
        let injected: Option<Result<Principal, PeerError>> = None;
        let server = match injected {
            Some(p) => p?,
            None => UnixPeer::from_stream(&stream)?.principal()?,
        };
        if server != Principal::Uid(0) && server != Principal::Uid(cfg.service.service_uid) {
            return Err(ClientError::ServerUid { got: server, expected: cfg.service.service_uid });
        }

        #[cfg(feature = "testing")]
        let own = match cfg.own_uid {
            Some(u) => u,
            None => own_uid().await?,
        };
        #[cfg(not(feature = "testing"))]
        let own = own_uid().await?;

        let (rd, mut wr) = stream.into_split();
        let mut rd = FrameReader::new(rd);

        let mut client_nonce = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut client_nonce);
        let hello = Message::Hello {
            client_nonce,
            proto_min: cfg.proto.0,
            proto_max: cfg.proto.1,
            features: cfg.features.clone(),
            role: cfg.role,
            build_sha: cfg.build_sha.clone(),
            exe: cfg.exe.clone(),
            pid: std::process::id(),
        };
        write_frame(&mut wr, &Frame::new(hello)).await?;
        let first = within(cfg.deadline, rd.read_frame()).await??.ok_or_else(eof)?;
        let ack = match first.msg {
            Message::HelloAck(a) => a,
            other => return Err(server_error(other)),
        };
        Self::check_hello_ack(cfg, &ack, &client_nonce, own)?;

        // Register: the signature binds the challenge, the uid the service saw
        // and the node id, so it cannot be replayed on another connection.
        let uid = Principal::Uid(ack.uid);
        let sig = user_key
            .sign(&register_signing_bytes(&ack.challenge, &uid, &ack.node_id))
            .to_bytes();
        let user_pubkey = user_key.verifying_key().to_bytes();
        let req = RegisterReq {
            user_pubkey,
            sig,
            addresses: Addresses {
                user_id: node_id_from_pubkey(&user_pubkey),
                projects: params.projects.clone(),
            },
            topic_prefixes: params.topic_prefixes.clone(),
            capabilities: params.capabilities.clone(),
            version: params.version.clone(),
            build_sha: cfg.build_sha.clone(),
            accept_from: params.accept_from.clone(),
        };
        write_frame(&mut wr, &Frame::new(Message::Register(req))).await?;
        let reply = within(cfg.deadline, rd.read_frame()).await??.ok_or_else(eof)?;
        let register_ack = match reply.msg {
            Message::RegisterAck(a) => a,
            other => return Err(server_error(other)),
        };
        register_ack.cert.verify(&ack.machine_pubkey, now_unix())?;
        if register_ack.cert.user_pubkey != user_pubkey {
            return Err(ClientError::Unexpected("certificate is for a different user key".into()));
        }

        let writer = Arc::new(AsyncMutex::new(wr));
        let pending: Pending = Arc::default();
        let (tx, events) = mpsc::channel(EVENT_QUEUE);
        let dropped_events = Arc::new(AtomicU64::new(0));
        let close_reason = Arc::new(Mutex::new(None));
        let reader = tokio::spawn(read_loop(
            rd,
            writer.clone(),
            pending.clone(),
            tx,
            dropped_events.clone(),
            close_reason.clone(),
        ));
        let features = negotiate_features(&cfg.features, &ack.features);
        Ok(Self {
            writer,
            pending,
            next_id: AtomicU64::new(1),
            events,
            dropped_events,
            close_reason,
            reader,
            features,
            cert: register_ack.cert.clone(),
            register_ack,
            hello_ack: ack,
            deadline: cfg.deadline,
        })
    }

    fn check_hello_ack(
        cfg: &ClientConfig,
        ack: &HelloAck,
        client_nonce: &[u8; 32],
        own_uid: u32,
    ) -> Result<(), ClientError> {
        if ack.proto < cfg.proto.0 || ack.proto > cfg.proto.1 {
            return Err(ClientError::ProtoOutOfRange {
                chosen: ack.proto,
                min: cfg.proto.0,
                max: cfg.proto.1,
            });
        }
        if ack.node_id != node_id_from_pubkey(&ack.machine_pubkey) {
            return Err(ClientError::BadServerHello("node_id does not match machine_pubkey".into()));
        }
        if ack.machine_pubkey != cfg.service.machine_pubkey {
            return Err(ClientError::MachineKeyChanged {
                pinned: hexser::encode(&cfg.service.machine_pubkey),
                presented: hexser::encode(&ack.machine_pubkey),
            });
        }
        // Proof of key possession over our fresh nonce, before anything is
        // pinned or registered: a process that merely holds the service uid, or
        // replays an old hello_ack, cannot produce it.
        if !verify_hello_proof(ack, client_nonce) {
            return Err(ClientError::BadServerProof);
        }
        // The proof covers ack.uid, so it is authentic; it must also be us.
        if ack.uid != own_uid {
            return Err(ClientError::UidMismatch { ack: ack.uid, own: own_uid });
        }
        if let Some(pin) = &cfg.machine_pin {
            check_pin(pin, &ack.machine_pubkey)?;
        }
        Ok(())
    }

    /// Events dropped because the queue ([`EVENT_QUEUE`]) was full.
    pub fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }

    fn closed(&self) -> ClientError {
        let why = self.close_reason.lock().expect("close lock").clone();
        ClientError::Closed(why.unwrap_or_else(|| "connection ended".into()))
    }

    pub fn hello_ack(&self) -> &HelloAck {
        &self.hello_ack
    }
    pub fn register_ack(&self) -> &RegisterAck {
        &self.register_ack
    }
    pub fn cert(&self) -> &UserCert {
        &self.cert
    }
    /// The negotiated protocol version.
    pub fn proto(&self) -> u32 {
        self.hello_ack.proto
    }
    /// Features both sides support.
    pub fn features(&self) -> &[String] {
        &self.features
    }

    /// Send a message with a fresh correlation id and wait for its reply.
    /// A server `error` reply becomes [`ClientError::Server`].
    pub async fn request(&self, msg: Message) -> Result<Message, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending lock").insert(id, tx);
        let sent = {
            let mut w = self.writer.lock().await;
            write_frame(&mut *w, &Frame::with_id(id, msg)).await
        };
        if let Err(e) = sent {
            self.pending.lock().expect("pending lock").remove(&id);
            return Err(e.into());
        }
        let reply = match tokio::time::timeout(self.deadline, rx).await {
            Ok(Ok(m)) => m,
            Ok(Err(_)) => return Err(self.closed()),
            Err(_) => {
                self.pending.lock().expect("pending lock").remove(&id);
                return Err(ClientError::Timeout);
            }
        };
        match reply {
            Message::Error(e) => Err(ClientError::Server(e)),
            m => Ok(m),
        }
    }

    /// Ask for a fresh certificate and verify it against the pinned machine key.
    pub async fn renew(&mut self) -> Result<&UserCert, ClientError> {
        match self.request(Message::Renew {}).await? {
            Message::Cert { cert } => {
                cert.verify(&self.hello_ack.machine_pubkey, now_unix())?;
                if cert.user_pubkey != self.cert.user_pubkey {
                    return Err(ClientError::Unexpected("renewed cert has a different user key".into()));
                }
                self.cert = cert;
                Ok(&self.cert)
            }
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Send a message to a `weft://` address. `local` is allowed here: the
    /// service resolves it to our registered node.
    pub async fn send(&self, dest: &WeftAddr, message: Value) -> Result<Message, ClientError> {
        dest.validate()?;
        self.request(Message::Send { dest: dest.to_string(), message, request_id: None }).await
    }

    /// Next unsolicited frame (`deliver`, `verdict.request`, ...), or `None`
    /// once the connection has closed.
    pub async fn next_event(&mut self) -> Option<Frame> {
        self.events.recv().await
    }

    /// Take the unsolicited-event receiver so another task can drain it while
    /// requests keep going through `&self`. Afterwards [`next_event`] yields
    /// `None`.
    pub fn take_events(&mut self) -> mpsc::Receiver<Frame> {
        let (_tx, closed) = mpsc::channel(1);
        std::mem::replace(&mut self.events, closed)
    }

    /// Reply to a service-initiated request (for example `verdict.request`).
    pub async fn reply(&self, id: u64, msg: Message) -> Result<(), ClientError> {
        let mut w = self.writer.lock().await;
        Ok(write_frame(&mut *w, &Frame::with_id(id, msg)).await?)
    }

    pub async fn close(self) {
        {
            let mut w = self.writer.lock().await;
            let _ = write_frame(&mut *w, &Frame::new(Message::Bye {})).await;
        }
        self.reader.abort();
    }
}

impl Drop for MeshLocalClient {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

fn eof() -> ClientError {
    ClientError::Closed("server closed the connection during the handshake".into())
}

/// Reads frames until the connection ends. Replies (reply-class messages with
/// a client-namespace id) wake their waiter; everything else is an event.
/// When the event queue is full the newest event is dropped and counted, so a
/// consumer that never calls `next_event` cannot stall request replies.
async fn read_loop(
    mut rd: FrameReader<tokio::net::unix::OwnedReadHalf>,
    writer: Arc<AsyncMutex<OwnedWriteHalf>>,
    pending: Pending,
    events: mpsc::Sender<Frame>,
    dropped: Arc<AtomicU64>,
    close_reason: Arc<Mutex<Option<String>>>,
) {
    let why = loop {
        let frame = match rd.read_frame().await {
            Ok(Some(f)) => f,
            Ok(None) => break "server closed the connection".to_string(),
            Err(e) => break format!("{e}"),
        };
        if let Some(id) = frame.id
            && id & SERVICE_ID_FLAG == 0
            && is_reply_class(&frame.msg)
        {
            let waiter = pending.lock().expect("pending lock").remove(&id);
            if let Some(tx) = waiter {
                let _ = tx.send(frame.msg);
                continue;
            }
        }
        if matches!(frame.msg, Message::Ping {}) {
            let mut w = writer.lock().await;
            let _ = write_frame(&mut *w, &Frame { id: frame.id, msg: Message::Pong {} }).await;
            continue;
        }
        // Service-initiated requests keep their id; callers answer with
        // `reply(id, ..)`.
        match events.try_send(frame) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(f)) => {
                dropped.fetch_add(1, Ordering::Relaxed);
                // Fail closed visibly: a dropped verdict request is denied now
                // instead of timing out at the service.
                if let (Message::VerdictRequest(_), Some(id)) = (&f.msg, f.id) {
                    let deny = Message::VerdictReply {
                        allow: false,
                        ttl_s: 0,
                        reason: "client event queue full".into(),
                        rule_hash: String::new(),
                    };
                    let mut w = writer.lock().await;
                    let _ = write_frame(&mut *w, &Frame::with_id(id, deny)).await;
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => break "client dropped".to_string(),
        }
    };
    *close_reason.lock().expect("close lock") = Some(why);
    // Dropping `pending` senders wakes every waiter with `Closed`.
    pending.lock().expect("pending lock").clear();
}
