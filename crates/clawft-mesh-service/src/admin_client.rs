//! A key-less mesh-local client for `weaver mesh ...` (admin and read-only
//! verbs). It verifies the server exactly as `MeshLocalClient` does (server
//! uid is root or the `service.json` uid, the machine key matches the record
//! and the optional pin, the hello proof binds our nonce and our uid) but
//! never registers and holds no key: authorisation is the peer credential.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clawft_mesh_local::framing::{write_frame, FrameError, FrameReader, HELLO_DEADLINE};
use clawft_mesh_local::peer::{own_uid, PeerError, UnixPeer};
use clawft_mesh_local::proto::{
    verify_hello_proof, ErrorBody, Frame, HelloAck, Message, Role, ServiceRecord, PROTO_MAX, PROTO_MIN,
};
use clawft_mesh_local::{hexser, Principal};
use rand::RngCore;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("framing: {0}")]
    Frame(#[from] FrameError),
    #[error("{} ({}): {}", .0.kind_name(), .0.remedy, .0.message)]
    Server(ErrorBodyView),
    #[error("could not read the server's peer credential: {0}")]
    Peer(#[from] PeerError),
    #[error("the socket is served by {got:?}, expected root or uid {expected} (service.json); refusing")]
    ServerUid { got: Principal, expected: u32 },
    #[error(
        "machine_key_changed: expected {expected} but the service presented {presented}; verify out \
         of band, then run `weaver mesh trust --replace`"
    )]
    KeyChanged { expected: String, presented: String },
    #[error("the service did not prove possession of its machine key")]
    BadProof,
    #[error("the service reports uid {ack} for this connection but this process runs as {own}")]
    UidMismatch { ack: u32, own: u32 },
    #[error("deadline exceeded")]
    Timeout,
    #[error("connection closed")]
    Closed,
    #[error("unexpected reply: {0}")]
    Unexpected(String),
    #[error("{0}")]
    Pin(String),
}

/// An [`ErrorBody`] for display and matching.
#[derive(Debug, Clone)]
pub struct ErrorBodyView(pub ErrorBody);

impl ErrorBodyView {
    fn kind_name(&self) -> String {
        format!("{:?}", self.0.kind)
    }
}

impl std::ops::Deref for ErrorBodyView {
    type Target = ErrorBody;
    fn deref(&self) -> &ErrorBody {
        &self.0
    }
}

/// How to reach and verify the service.
#[derive(Clone)]
pub struct ConnectConfig {
    pub socket: PathBuf,
    /// `service.json` (copy beside the socket).
    pub service: ServiceRecord,
    /// Machine-key pin to compare against, if any. Read only here.
    pub pin: Option<PathBuf>,
    pub role: Role,
    pub build_sha: String,
    pub deadline: Duration,
}

impl ConnectConfig {
    pub fn new(socket: impl Into<PathBuf>, service: ServiceRecord, role: Role) -> Self {
        Self {
            socket: socket.into(),
            service,
            pin: None,
            role,
            build_sha: String::new(),
            deadline: HELLO_DEADLINE,
        }
    }
}

pub struct AdminClient {
    rd: FrameReader<OwnedReadHalf>,
    wr: OwnedWriteHalf,
    pub ack: HelloAck,
    next_id: u64,
    deadline: Duration,
}

async fn within<T>(d: Duration, f: impl std::future::Future<Output = T>) -> Result<T, AdminError> {
    tokio::time::timeout(d, f).await.map_err(|_| AdminError::Timeout)
}

/// Compare `key` to the pin file when it exists (a missing pin is not an error here).
fn check_pin_readonly(pin: &Path, key: &[u8; 32]) -> Result<(), AdminError> {
    match std::fs::read_to_string(pin) {
        Ok(s) => match hexser::decode::<32>(s.trim().to_ascii_lowercase().as_str()) {
            Some(p) if &p == key => Ok(()),
            Some(p) => Err(AdminError::KeyChanged {
                expected: hexser::encode(&p),
                presented: hexser::encode(key),
            }),
            None => Err(AdminError::Pin(format!("{} is corrupt; run `weaver mesh trust --replace`", pin.display()))),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

impl AdminClient {
    pub async fn connect(cfg: &ConnectConfig) -> Result<Self, AdminError> {
        let stream = within(cfg.deadline, UnixStream::connect(&cfg.socket)).await??;
        let server = UnixPeer::from_stream(&stream)?;
        let server = clawft_mesh_local::peer::PeerIdentity::principal(&server)?;
        if server != Principal::Uid(0) && server != Principal::Uid(cfg.service.service_uid) {
            return Err(AdminError::ServerUid { got: server, expected: cfg.service.service_uid });
        }
        let own = own_uid().await?;
        let (rd, mut wr) = stream.into_split();
        let mut rd = FrameReader::new(rd);
        let mut nonce = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let hello = Message::Hello {
            proto_min: PROTO_MIN,
            proto_max: PROTO_MAX,
            features: Vec::new(),
            role: cfg.role,
            build_sha: cfg.build_sha.clone(),
            exe: std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
            pid: std::process::id(),
            client_nonce: nonce,
        };
        write_frame(&mut wr, &Frame::new(hello)).await?;
        let first = within(cfg.deadline, rd.read_frame()).await??.ok_or(AdminError::Closed)?;
        let ack = match first.msg {
            Message::HelloAck(a) => a,
            Message::Error(e) => return Err(AdminError::Server(ErrorBodyView(e))),
            other => return Err(AdminError::Unexpected(format!("{other:?}"))),
        };
        if ack.machine_pubkey != cfg.service.machine_pubkey {
            return Err(AdminError::KeyChanged {
                expected: hexser::encode(&cfg.service.machine_pubkey),
                presented: hexser::encode(&ack.machine_pubkey),
            });
        }
        if !verify_hello_proof(&ack, &nonce) {
            return Err(AdminError::BadProof);
        }
        if ack.uid != own {
            return Err(AdminError::UidMismatch { ack: ack.uid, own });
        }
        if let Some(pin) = &cfg.pin {
            check_pin_readonly(pin, &ack.machine_pubkey)?;
        }
        Ok(Self { rd, wr, ack, next_id: 1, deadline: cfg.deadline.max(Duration::from_secs(15)) })
    }

    /// Send one request and wait for its reply (an `error` becomes `Err`).
    pub async fn request(&mut self, msg: Message) -> Result<Message, AdminError> {
        let id = self.next_id;
        self.next_id += 1;
        write_frame(&mut self.wr, &Frame::with_id(id, msg)).await?;
        loop {
            let f = within(self.deadline, self.rd.read_frame()).await??.ok_or(AdminError::Closed)?;
            if f.id != Some(id) {
                continue;
            }
            return match f.msg {
                Message::Error(e) => Err(AdminError::Server(ErrorBodyView(e))),
                m => Ok(m),
            };
        }
    }

    pub async fn bye(mut self) {
        let _ = write_frame(&mut self.wr, &Frame::new(Message::Bye {})).await;
    }
}

/// Write the machine-key pin (mode 0600, parent 0700) after a human verified
/// the key out of band. Refuses to replace a different pin unless `replace`.
pub fn write_pin(path: &Path, key: &[u8; 32], replace: bool) -> Result<(), AdminError> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let same = hexser::decode::<32>(existing.trim().to_ascii_lowercase().as_str()).as_ref() == Some(key);
        if same {
            return Ok(());
        }
        if !replace {
            return Err(AdminError::Pin(format!(
                "{} holds a different key; pass --replace after verifying the new key out of band",
                path.display()
            )));
        }
    }
    if let Some(dir) = path.parent()
        && std::fs::symlink_metadata(dir).is_err()
    {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    }
    crate::facts::write_with_mode(path, format!("{}\n", hexser::encode(key)).as_bytes(), 0o600)?;
    Ok(())
}

/// Short fingerprint of a machine key for out-of-band comparison.
pub fn fingerprint(key: &[u8; 32]) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(key);
    d[..8].chunks(2).map(hexser::encode).collect::<Vec<_>>().join(":")
}
