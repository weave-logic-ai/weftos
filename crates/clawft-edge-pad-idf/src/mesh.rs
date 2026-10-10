//! Mesh transport client — std::net port of the embassy-net client at
//! `clawft-edge-pad/src/mesh.rs`.
//!
//! Outbound control uses the ADR-103 certified leaf protocol:
//! - Framing: `[4-byte big-endian length][WLF1 || CBOR SignedPublish]`.
//! - Subscribe: a signed `MeshIpcEnvelope` with `target = Topic("mesh.subscribe")`
//!   and `payload = Json({"topic": "<our push topic>"})`.
//! - Discovery: a nonce-bound machine-signed UDP advertisement selects the
//!   parent endpoint. The publish remains in NVS until its WLA1 ACK verifies.
//! - Inbound leaf-push: a `MeshIpcEnvelope` carrying `{"type":"leaf_push",
//!   "cbor_b64":"<base64 CBOR>",...}`. We extract the base64, decode to
//!   CBOR, decode to `LeafPush`.
//!
//! Inbound display pushes still use the existing JSON mesh envelope.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use log::{info, warn};

use weftos_leaf_display::{Compositor, LeafPush};
use ed25519_dalek::SigningKey;
use esp_idf_svc::nvs::EspDefaultNvs;
use weftos_leaf_types::push_topic;
use weftos_leaf_scene::{InputEnvelope, InputEvent};
use weftos_leaf_types::link::{LeafCertificate, OfflineJournal, ParentAdvertisement, PublishAck, FRAME_MAGIC, ACK_MAGIC};

use crate::display::DpiDisplay;

// Per-device credentials come from the operator's provisioned files at
// build time. An image built without them boots the display but never
// connects to the mesh or emits unsigned traffic.
include!(concat!(env!("OUT_DIR"), "/leaf_provision.rs"));
const DISCOVERY_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), 9490);
const MAX_NVS_JOURNAL: usize = 4096;

const RX_BUF: usize = 8192;

/// Hand-rolled subscribe `MeshIpcEnvelope` (fixed-shape JSON).
/// Identical to `clawft-edge-pad::mesh::subscribe_envelope`.
fn subscribe_envelope(id: &str, topic: &str) -> String {
    format!(
        concat!(
            r#"{{"source_node":"{id}","dest_node":"daemon","message":{{"#,
            r#""id":"leaf-sub-1","from":0,"target":{{"Topic":"mesh.subscribe"}},"#,
            r#""payload":{{"Json":{{"topic":"{topic}"}}}},"#,
            r#""timestamp":"2026-05-14T00:00:00Z"}},"#,
            r#""hop_count":0,"envelope_id":"leaf-env-sub-1"}}"#
        ),
        id = id,
        topic = topic,
    )
}

fn input_envelope(id: &str, topic: &str, event: InputEvent) -> anyhow::Result<String> {
    let input = InputEnvelope::new(0, None, event);
    let cbor = weftos_leaf_scene::codec::encode(&input)
        .map_err(|e| anyhow::anyhow!("encode touch CBOR: {e:?}"))?;
    let inner = serde_json::json!({
        "type": "leaf_input", "cbor_b64": base64_encode(&cbor), "target_pubkey": id
    }).to_string();
    Ok(serde_json::json!({
        "source_node": id, "dest_node": "daemon",
        "message": {"id": "leaf-input", "from": 0,
            "target": {"Topic": "ipc.publish"},
            "payload": {"Json": {"topic": topic, "message": inner}},
            "timestamp": "2026-05-14T00:00:00Z"},
        "hop_count": 0, "envelope_id": "leaf-env-input"
    }).to_string())
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = *chunk.get(1).unwrap_or(&0);
        let c = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(a >> 2) as usize] as char);
        out.push(TABLE[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[(((b & 15) << 2) | (c >> 6)) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(c & 63) as usize] as char } else { '=' });
    }
    out
}

/// Write a `[4-byte BE length][payload]` frame to the socket.
fn write_frame(sock: &mut TcpStream, payload: &[u8]) -> std::io::Result<()> {
    let len = (payload.len() as u32).to_be_bytes();
    sock.write_all(&len)?;
    sock.write_all(payload)?;
    Ok(())
}

/// Read exactly `buf.len()` bytes or fail.
fn read_exact(sock: &mut TcpStream, buf: &mut [u8]) -> std::io::Result<()> {
    sock.read_exact(buf)
}

struct LeafLink {
    cert: LeafCertificate,
    key: SigningKey,
    queue: OfflineJournal,
    nvs: EspDefaultNvs,
}

impl LeafLink {
    fn load(nvs: EspDefaultNvs) -> anyhow::Result<Self> {
        anyhow::ensure!(LEAF_SEED.len() == 32 && !LEAF_CERT.is_empty(), "leaf image has no provisioned identity");
        let seed: [u8; 32] = LEAF_SEED.try_into()?;
        let key = SigningKey::from_bytes(&seed);
        let cert: LeafCertificate = weftos_leaf_types::decode(LEAF_CERT)
            .map_err(|e| anyhow::anyhow!("decode leaf certificate: {e:?}"))?;
        let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        cert.verify(&cert.parent_pubkey, wall.max(cert.issued_at)).map_err(|e| anyhow::anyhow!("leaf certificate: {e:?}"))?;
        anyhow::ensure!(cert.leaf_pubkey == key.verifying_key().to_bytes(), "leaf key/certificate mismatch");
        let queue = match nvs.blob_len("journal")? {
            None => OfflineJournal::default(),
            Some(len) => {
                anyhow::ensure!(len <= MAX_NVS_JOURNAL, "NVS leaf journal exceeds bound");
                let mut buf = vec![0u8; len];
                let bytes = nvs.get_blob("journal", &mut buf)?.ok_or_else(|| anyhow::anyhow!("NVS leaf journal disappeared"))?;
                weftos_leaf_types::decode(bytes)
                    .map_err(|e| anyhow::anyhow!("decode NVS leaf journal: {e:?}"))?
            }
        };
        queue.validate(&cert).map_err(|e| anyhow::anyhow!("NVS leaf journal invalid: {e:?}"))?;
        Ok(Self { cert, key, queue, nvs })
    }

    fn save_queue(&mut self, next: OfflineJournal) -> anyhow::Result<()> {
        let bytes = weftos_leaf_types::encode(&next)
            .map_err(|e| anyhow::anyhow!("encode NVS leaf journal: {e:?}"))?;
        anyhow::ensure!(bytes.len() <= MAX_NVS_JOURNAL, "NVS leaf journal full");
        // EspNvs::set_blob commits before returning. Never send a frame
        // until this succeeds; never forget one until ACK persistence does.
        self.nvs.set_blob("journal", &bytes)?;
        self.queue = next;
        Ok(())
    }

    fn subscribe(&mut self) -> anyhow::Result<()> {
        let id = self.cert.leaf_id();
        let topic = push_topic(&id);
        let envelope = subscribe_envelope(&id, &topic);
        let mut next = self.queue.clone();
        next.enqueue(&self.cert, &self.key, "mesh.subscribe".into(), envelope.into_bytes())
            .map_err(|e| anyhow::anyhow!("signed subscribe: {e:?}"))?;
        self.save_queue(next)
    }

    /// False means the bounded journal is full. The caller retains the
    /// event and backpressures the touch producer until replay frees space.
    fn enqueue_input(&mut self, event: InputEvent) -> anyhow::Result<bool> {
        let id = self.cert.leaf_id();
        let topic = format!("mesh.leaf.{id}.input");
        let envelope = input_envelope(&id, &topic, event)?;
        let mut next = self.queue.clone();
        match next.enqueue(&self.cert, &self.key, topic, envelope.into_bytes()) {
            Ok(_) => {},
            Err(weftos_leaf_types::link::LinkError::JournalFull) => return Ok(false),
            Err(e) => return Err(anyhow::anyhow!("signed input journal: {e:?}")),
        }
        let bytes = weftos_leaf_types::encode(&next)
            .map_err(|e| anyhow::anyhow!("encode NVS input journal: {e:?}"))?;
        if bytes.len() > MAX_NVS_JOURNAL { return Ok(false); }
        self.nvs.set_blob("journal", &bytes)?;
        self.queue = next;
        Ok(true)
    }

    fn stage_touch(&mut self, rx: &Receiver<InputEvent>, held: &mut Option<InputEvent>) -> anyhow::Result<()> {
        loop {
            let event = match held.take() {
                Some(event) => event,
                None => match rx.try_recv() {
                    Ok(event) => event,
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(()),
                }
            };
            if !self.enqueue_input(event)? {
                *held = Some(event);
                warn!("[mesh] NVS leaf journal full; touch producer backpressured until replay");
                return Ok(());
            }
        }
    }

    fn discover(&self) -> anyhow::Result<SocketAddr> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_broadcast(true)?;
        socket.set_read_timeout(Some(Duration::from_secs(3)))?;
        let mut nonce = [0u8; 16];
        unsafe { esp_idf_svc::sys::esp_fill_random(nonce.as_mut_ptr().cast(), nonce.len()) };
        let mut request = Vec::with_capacity(52);
        request.extend_from_slice(b"WLD1");
        request.extend_from_slice(self.cert.leaf_id().as_bytes());
        request.extend_from_slice(&nonce);
        socket.send_to(&request, DISCOVERY_ADDR)?;
        let mut response = [0u8; 1024];
        loop {
            let (n, _) = socket.recv_from(&mut response)?;
            let Ok(ad) = weftos_leaf_types::decode::<ParentAdvertisement>(&response[..n]) else { continue };
            // The fresh random nonce prevents replay even before this
            // device's wall clock has been synchronized.
            let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            if ad.verify_for(&self.cert.mesh_pubkey, &self.cert.parent_scope, wall, &nonce).is_ok() {
                return ad.endpoint.parse().map_err(Into::into);
            }
        }
    }

    fn replay(&mut self, sock: &mut TcpStream, mut on_push: impl FnMut(&[u8])) -> anyhow::Result<()> {
        while let Some(frame) = self.queue.oldest().cloned() {
            let mut wire = FRAME_MAGIC.to_vec();
            wire.extend_from_slice(&weftos_leaf_types::encode(&frame)
                .map_err(|e| anyhow::anyhow!("encode signed leaf frame: {e:?}"))?);
            anyhow::ensure!(wire.len() <= 32 * 1024, "signed leaf frame exceeds mesh bound");
            write_frame(sock, &wire)?;
            let ack = loop {
                let mut len_bytes = [0u8; 4];
                read_exact(sock, &mut len_bytes)?;
                let len = u32::from_be_bytes(len_bytes) as usize;
                anyhow::ensure!((1..=RX_BUF).contains(&len), "invalid mesh reply length");
                let mut reply = vec![0u8; len];
                read_exact(sock, &mut reply)?;
                if reply.starts_with(ACK_MAGIC) {
                    let ack: PublishAck = weftos_leaf_types::decode(&reply[ACK_MAGIC.len()..])
                        .map_err(|e| anyhow::anyhow!("decode signed ACK: {e:?}"))?;
                    break ack;
                }
                on_push(&reply);
            };
            let mut next = self.queue.clone();
            next.acknowledge(&ack).map_err(|e| anyhow::anyhow!("signed ACK rejected: {e:?}"))?;
            self.save_queue(next)?;
        }
        Ok(())
    }
}

/// Extract `cbor_b64`'s base64 value from a raw envelope. Identical
/// algorithm to the bare-metal port — scan for the key, read base64
/// chars until the first non-alphabet byte.
fn extract_leaf_push_cbor(envelope: &[u8]) -> Option<Vec<u8>> {
    let needle = b"cbor_b64";
    let key_at = envelope.windows(needle.len()).position(|w| w == needle)?;
    let mut i = key_at + needle.len();
    let mut b64 = Vec::new();
    let mut started = false;
    while i < envelope.len() {
        let c = envelope[i];
        if is_b64(c) {
            started = true;
            b64.push(c);
        } else if started {
            break;
        }
        i += 1;
    }
    if b64.is_empty() {
        return None;
    }
    base64_decode(&b64)
}

#[inline]
fn is_b64(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'='
}

/// Minimal standard-alphabet base64 decoder. Identical to the bare-metal port.
fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &c in input {
        let v = match val(c) {
            Some(v) => v,
            None => continue,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Run the mesh client forever on the calling thread. Owns the display
/// stack (surface + compositor). The bare-metal port spawned this as
/// an embassy task; the IDF port runs it on a FreeRTOS-backed std
/// thread the caller dedicates.
pub fn run(mut surface: DpiDisplay, mut compositor: Compositor, nvs: EspDefaultNvs, touch_rx: Receiver<InputEvent>) -> ! {
    let mut leaf = match LeafLink::load(nvs) {
        Ok(leaf) => leaf,
        Err(e) => {
            warn!("[mesh] certified identity/journal unavailable: {e}; mesh disabled");
            boot_screen(&mut compositor, &mut surface, "leaf identity unavailable");
            loop { std::thread::sleep(Duration::from_secs(30)); }
        }
    };
    let id = leaf.cert.leaf_id();
    let topic = push_topic(&id);
    info!("[mesh] certified leaf id '{}', push topic '{}'", id, topic);

    boot_screen(&mut compositor, &mut surface, "connecting to mesh...");
    let mut shown_waiting = false;
    let mut held_touch = None;

    loop {
        if let Err(e) = leaf.stage_touch(&touch_rx, &mut held_touch) {
            warn!("[mesh] cannot persist touch observation: {e}; mesh disabled");
            boot_screen(&mut compositor, &mut surface, "leaf journal unavailable");
            loop { std::thread::sleep(Duration::from_secs(30)); }
        }
        let addr = match leaf.discover() {
            Ok(addr) => addr,
            Err(e) => {
                warn!("[mesh] certified parent discovery failed: {e}; retry 3s");
                std::thread::sleep(Duration::from_secs(3));
                continue;
            }
        };
        info!("[mesh] connecting to {addr}");
        let mut sock = match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
            Ok(s) => s,
            Err(e) => {
                warn!("[mesh] connect failed: {e:?} — retry 3s");
                std::thread::sleep(Duration::from_secs(3));
                continue;
            }
        };
        // Give signed ACKs time for parent fsync. The live display loop
        // switches to a short poll after backlog and subscribe replay.
        sock.set_read_timeout(Some(Duration::from_secs(10))).ok();
        sock.set_write_timeout(Some(Duration::from_secs(10))).ok();
        sock.set_nodelay(true).ok();

        info!("[mesh] connected — replaying durable backlog before new subscribe to '{topic}'");
        if let Err(e) = leaf.replay(&mut sock, |bytes| apply_push(bytes, &mut compositor, &mut surface)) {
            warn!("[mesh] signed replay failed: {e}; retry 3s");
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }
        if let Err(e) = leaf.subscribe().and_then(|_| leaf.replay(&mut sock, |bytes| apply_push(bytes, &mut compositor, &mut surface))) {
            warn!("[mesh] signed subscribe failed: {e}; retry 3s");
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }
        sock.set_read_timeout(Some(Duration::from_millis(500))).ok();
        if !shown_waiting {
            boot_screen(
                &mut compositor,
                &mut surface,
                "subscribed -- waiting for pushes",
            );
            shown_waiting = true;
        }

        let mut frame = [0u8; RX_BUF];
        loop {
            if let Err(e) = leaf.stage_touch(&touch_rx, &mut held_touch) {
                warn!("[mesh] cannot persist touch observation: {e}; mesh disabled");
                boot_screen(&mut compositor, &mut surface, "leaf journal unavailable");
                loop { std::thread::sleep(Duration::from_secs(30)); }
            }
            if !leaf.queue.pending.is_empty() { sock.set_read_timeout(Some(Duration::from_secs(10))).ok(); }
            if let Err(e) = leaf.replay(&mut sock, |bytes| apply_push(bytes, &mut compositor, &mut surface)) {
                warn!("[mesh] signed touch replay failed: {e}; reconnecting");
                break;
            }
            sock.set_read_timeout(Some(Duration::from_millis(500))).ok();
            let mut len_buf = [0u8; 4];
            match sock.peek(&mut len_buf) {
                Ok(0) => break,
                Ok(n) if n < len_buf.len() => continue,
                Ok(_) => {},
                Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => continue,
                Err(e) => { warn!("[mesh] peek failed: {e:?} — reconnecting"); break; }
            }
            if let Err(e) = read_exact(&mut sock, &mut len_buf) {
                warn!("[mesh] read len failed: {e:?} — reconnecting");
                break;
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            if len == 0 || len > frame.len() {
                warn!("[mesh] frame len {len} out of range — reconnecting");
                break;
            }
            if let Err(e) = read_exact(&mut sock, &mut frame[..len]) {
                warn!("[mesh] read body failed: {e:?} — reconnecting");
                break;
            }
            apply_push(&frame[..len], &mut compositor, &mut surface);
        }

        // Connection dropped — back off and reconnect.
        drop(sock);
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn apply_push(bytes: &[u8], compositor: &mut Compositor, surface: &mut DpiDisplay) {
    let Some(cbor) = extract_leaf_push_cbor(bytes) else { return };
    match weftos_leaf_types::decode::<LeafPush>(&cbor) {
        Ok(push) => {
            compositor.apply(push);
            if let Err(e) = compositor.compose(surface) { warn!("[mesh] compose failed: {e:?}"); }
        }
        Err(e) => warn!("[mesh] LeafPush CBOR decode failed: {e}"),
    }
}

/// Draw a single status line via the compositor — used for the boot
/// screen and connection-state messages. Identical to the bare-metal port.
/// `main` also calls it once before WiFi so the panel proves itself offline.
pub(crate) fn boot_screen(comp: &mut Compositor, surface: &mut DpiDisplay, msg: &str) {
    use weftos_leaf_types::{DisplayClear, DisplayText, LayerSlot};
    comp.apply(LeafPush::DisplayClear(DisplayClear { z: LayerSlot::Text }));
    comp.apply(LeafPush::DisplayText(DisplayText {
        z: LayerSlot::Text,
        text: String::from("clawft-edge-pad-idf :: mesh terminal"),
        x: 40,
        y: 50,
        color: [255, 255, 255],
        clear_first: false,
    }));
    comp.apply(LeafPush::DisplayText(DisplayText {
        z: LayerSlot::Text,
        text: String::from(msg),
        x: 40,
        y: 90,
        color: [0, 255, 255],
        clear_first: false,
    }));
    let _ = comp.compose(surface);
}
