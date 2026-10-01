//! A loopback mesh-local/1 server on a real unix socket, for tests of this
//! crate and of crates that embed the client before the real service exists.
//! Peer credentials come from an injected [`PeerIdentity`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;
use rand::RngCore;
use tokio::net::{UnixListener, UnixStream};

use crate::cert::{node_id_from_pubkey, UserCert, DEFAULT_TTL_S};
use crate::client::now_unix;
use crate::framing::{write_frame, FrameReader, HELLO_DEADLINE};
use crate::peer::{PeerIdentity, Principal};
use crate::proto::{
    negotiate, negotiate_features, verify_register_sig, Accepted, BindState, ErrorBody, ErrorKind,
    Frame, HelloAck, Message, RegisterAck, RegisterReq, ServiceRecord, VersionRange,
};

#[derive(Clone)]
pub struct TestServerConfig {
    pub machine_key: SigningKey,
    /// The range this server speaks.
    pub proto: (u32, u32),
    pub features: Vec<String>,
    pub peer: Arc<dyn PeerIdentity>,
    pub cert_ttl_s: u64,
}

impl TestServerConfig {
    pub fn new(machine_key: SigningKey, peer: Arc<dyn PeerIdentity>) -> Self {
        Self {
            machine_key,
            proto: (crate::PROTO_MIN, crate::PROTO_MAX),
            features: Vec::new(),
            peer,
            cert_ttl_s: DEFAULT_TTL_S,
        }
    }
}

#[derive(Default)]
struct State {
    /// principal -> bound user pubkey
    binds: HashMap<Principal, [u8; 32]>,
    registrations: Vec<(Principal, RegisterReq)>,
    sent: Vec<(String, serde_json::Value)>,
    serial: u64,
}

pub struct TestServer {
    path: PathBuf,
    cfg: TestServerConfig,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
    conns: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl TestServer {
    pub fn start(path: &Path, cfg: TestServerConfig) -> std::io::Result<Self> {
        let listener = UnixListener::bind(path)?;
        let state: Arc<Mutex<State>> = Arc::default();
        let conns: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>> = Arc::default();
        let (c, s, k) = (cfg.clone(), state.clone(), conns.clone());
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let h = tokio::spawn(serve(stream, c.clone(), s.clone()));
                k.lock().expect("conns").push(h);
            }
        });
        Ok(Self { path: path.to_path_buf(), cfg, state, task, conns })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn machine_pubkey(&self) -> [u8; 32] {
        self.cfg.machine_key.verifying_key().to_bytes()
    }

    /// The `service.json` record a client would pin, claiming `service_uid`.
    pub fn service_record(&self, service_uid: u32) -> ServiceRecord {
        let pk = self.machine_pubkey();
        ServiceRecord {
            node_id: node_id_from_pubkey(&pk),
            machine_pubkey: pk,
            service_uid,
            proto: VersionRange { min: self.cfg.proto.0, max: self.cfg.proto.1, sha: None },
            build_sha: "test".into(),
            started_at: now_unix(),
        }
    }

    /// Registrations accepted so far, in order.
    pub fn registrations(&self) -> Vec<(Principal, RegisterReq)> {
        self.state.lock().expect("state").registrations.clone()
    }

    /// `(dest, message)` of every `send` received.
    pub fn sent(&self) -> Vec<(String, serde_json::Value)> {
        self.state.lock().expect("state").sent.clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
        for h in self.conns.lock().expect("conns").drain(..) {
            h.abort();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn err(kind: ErrorKind, msg: &str, remedy: &str) -> Message {
    Message::Error(ErrorBody::new(kind, msg, remedy))
}

async fn serve(stream: UnixStream, cfg: TestServerConfig, state: Arc<Mutex<State>>) {
    let _ = serve_inner(stream, cfg, state).await;
}

async fn serve_inner(
    stream: UnixStream,
    cfg: TestServerConfig,
    state: Arc<Mutex<State>>,
) -> Result<(), crate::framing::FrameError> {
    let Ok(principal) = cfg.peer.principal() else { return Ok(()) };
    let Principal::Uid(uid) = principal else { return Ok(()) };
    let machine_pubkey = cfg.machine_key.verifying_key().to_bytes();
    let node_id = node_id_from_pubkey(&machine_pubkey);
    let (rd, mut wr) = stream.into_split();
    let mut rd = FrameReader::new(rd);

    let Some(first) = rd.read_frame_within(HELLO_DEADLINE).await? else { return Ok(()) };
    let (client_features, proto) = match first.msg {
        Message::Hello { proto_min, proto_max, features, .. } => {
            match negotiate(cfg.proto, (proto_min, proto_max)) {
                Ok(p) => (features, p),
                Err(m) => {
                    let body = Message::Error(ErrorBody::proto_mismatch(&m));
                    return write_frame(&mut wr, &Frame::new(body)).await;
                }
            }
        }
        _ => {
            let m = err(ErrorKind::Forbidden, "expected hello", "send hello first");
            return write_frame(&mut wr, &Frame::new(m)).await;
        }
    };
    let mut challenge = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut challenge);
    let ack = HelloAck {
        proto,
        features: negotiate_features(&cfg.features, &client_features),
        node_id: node_id.clone(),
        machine_pubkey,
        service_build_sha: "test".into(),
        deprecated_below: None,
        uid,
        challenge,
    };
    write_frame(&mut wr, &Frame::new(Message::HelloAck(ack))).await?;

    let mut user_pubkey: Option<[u8; 32]> = None;
    while let Some(frame) = rd.read_frame_within(std::time::Duration::from_secs(30)).await? {
        let reply = match frame.msg {
            Message::Register(req) if user_pubkey.is_none() => {
                match register(&cfg, &state, &principal, &node_id, &challenge, req.clone()) {
                    Ok(r) => {
                        user_pubkey = Some(req.user_pubkey);
                        Message::RegisterAck(r)
                    }
                    Err(e) => {
                        write_frame(&mut wr, &Frame { id: frame.id, msg: *e }).await?;
                        return Ok(());
                    }
                }
            }
            Message::Renew {} => match user_pubkey {
                Some(pk) => Message::Cert { cert: issue(&cfg, &state, pk) },
                None => err(ErrorKind::Forbidden, "register first", "send register"),
            },
            Message::Ping {} => Message::Pong {},
            Message::Status {} => Message::Reply { data: serde_json::json!({"node_id": node_id}) },
            Message::Send { dest, message, .. } => {
                state.lock().expect("state").sent.push((dest, message));
                Message::Ack {}
            }
            Message::Bye {} => return Ok(()),
            _ => err(ErrorKind::Unsupported, "not supported by the test server", ""),
        };
        write_frame(&mut wr, &Frame { id: frame.id, msg: reply }).await?;
    }
    Ok(())
}

fn issue(cfg: &TestServerConfig, state: &Mutex<State>, user_pubkey: [u8; 32]) -> UserCert {
    let mut st = state.lock().expect("state");
    st.serial += 1;
    UserCert::issue(&cfg.machine_key, user_pubkey, st.serial, now_unix(), cfg.cert_ttl_s)
}

fn register(
    cfg: &TestServerConfig,
    state: &Mutex<State>,
    principal: &Principal,
    node_id: &str,
    challenge: &[u8; 32],
    req: RegisterReq,
) -> Result<RegisterAck, Box<Message>> {
    if !verify_register_sig(&req, challenge, principal, node_id) {
        return Err(Box::new(err(ErrorKind::BadSig, "register signature does not verify", "re-register")));
    }
    let user_id = node_id_from_pubkey(&req.user_pubkey);
    if req.addresses.user_id != user_id {
        return Err(Box::new(err(ErrorKind::Forbidden, "user_id does not match user_pubkey", "")));
    }
    let bind = {
        let mut st = state.lock().expect("state");
        match st.binds.get(principal) {
            Some(pk) if *pk != req.user_pubkey => {
                return Err(Box::new(err(
                    ErrorKind::BindConflict,
                    "uid is bound to a different key",
                    "run `weaver mesh bind rebind`",
                )));
            }
            Some(_) => BindState::Existing,
            None => {
                st.binds.insert(principal.clone(), req.user_pubkey);
                BindState::New
            }
        }
    };
    let cert = issue(cfg, state, req.user_pubkey);
    let accepted = Accepted {
        addresses: req.addresses.projects.iter().map(|p| p.project_id.clone()).collect(),
        topic_prefixes: req.topic_prefixes.clone(),
    };
    state.lock().expect("state").registrations.push((principal.clone(), req));
    Ok(RegisterAck { user_id, cert, accepted, rejected: Vec::new(), bind })
}
