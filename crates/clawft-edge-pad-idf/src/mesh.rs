//! Mesh transport client — std::net port of the embassy-net client at
//! `clawft-edge-pad/src/mesh.rs`.
//!
//! Wire protocol is identical (it's the WeftOS leaf-push protocol, not
//! transport-specific):
//! - Framing: `[4-byte big-endian length][JSON MeshIpcEnvelope]`.
//! - Subscribe: a `MeshIpcEnvelope` with `target = Topic("mesh.subscribe")`
//!   and `payload = Json({"topic": "<our push topic>"})`.
//! - Inbound leaf-push: a `MeshIpcEnvelope` carrying `{"type":"leaf_push",
//!   "cbor_b64":"<base64 CBOR>",...}`. We extract the base64, decode to
//!   CBOR, decode to `LeafPush`.
//!
//! The base64-extract + decode helpers are line-for-line ports; only
//! the I/O changes (TcpStream + std::thread::sleep instead of embassy
//! TcpSocket + Timer).

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use log::{info, warn};

use weftos_leaf_display::{Compositor, LeafPush};
use weftos_leaf_types::push_topic;

use crate::display::DpiDisplay;

// ── Connection constants (spike: hardcoded — same as bare-metal port) ──
const DAEMON_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 73);
const DAEMON_MESH_PORT: u16 = 9470;
/// Device-MAC-derived leaf identity. Identical to the bare-metal port.
const LEAF_ID: &str = "3cdc75fabc7c";

const RX_BUF: usize = 8192;

/// Hand-rolled subscribe `MeshIpcEnvelope` (fixed-shape JSON).
/// Identical to `clawft-edge-pad::mesh::subscribe_envelope`.
fn subscribe_envelope(topic: &str) -> String {
    format!(
        concat!(
            r#"{{"source_node":"{id}","dest_node":"daemon","message":{{"#,
            r#""id":"leaf-sub-1","from":0,"target":{{"Topic":"mesh.subscribe"}},"#,
            r#""payload":{{"Json":{{"topic":"{topic}"}}}},"#,
            r#""timestamp":"2026-05-14T00:00:00Z"}},"#,
            r#""hop_count":0,"envelope_id":"leaf-env-sub-1"}}"#
        ),
        id = LEAF_ID,
        topic = topic,
    )
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
pub fn run(mut surface: DpiDisplay, mut compositor: Compositor) -> ! {
    let topic = push_topic(LEAF_ID);
    info!("[mesh] leaf id '{}', push topic '{}'", LEAF_ID, topic);

    boot_screen(&mut compositor, &mut surface, "connecting to mesh...");
    let mut shown_waiting = false;

    loop {
        let addr = SocketAddr::new(IpAddr::V4(DAEMON_IP), DAEMON_MESH_PORT);
        info!("[mesh] connecting to {addr}");
        let mut sock = match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
            Ok(s) => s,
            Err(e) => {
                warn!("[mesh] connect failed: {e:?} — retry 3s");
                std::thread::sleep(Duration::from_secs(3));
                continue;
            }
        };
        // 120 s read timeout — the daemon sends no keepalives. Tight
        // timeouts just churn reconnects; the bare-metal port chose
        // the same value.
        sock.set_read_timeout(Some(Duration::from_secs(120))).ok();
        sock.set_write_timeout(Some(Duration::from_secs(10))).ok();
        sock.set_nodelay(true).ok();

        info!("[mesh] connected — subscribing to '{topic}'");
        let sub = subscribe_envelope(&topic);
        if let Err(e) = write_frame(&mut sock, sub.as_bytes()) {
            warn!("[mesh] subscribe write failed: {e:?} — retry 3s");
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }
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
            let mut len_buf = [0u8; 4];
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
            info!("[mesh] rx frame: {len} bytes");

            let Some(cbor) = extract_leaf_push_cbor(&frame[..len]) else {
                info!("[mesh]   (no cbor_b64 — not a leaf-push envelope)");
                continue;
            };
            match weftos_leaf_types::decode::<LeafPush>(&cbor) {
                Ok(push) => {
                    info!(
                        "[mesh]   LeafPush decoded ({} cbor bytes) — applying",
                        cbor.len()
                    );
                    compositor.apply(push);
                    if let Err(e) = compositor.compose(&mut surface) {
                        warn!("[mesh] compose failed: {e:?}");
                    }
                }
                Err(e) => warn!("[mesh] LeafPush CBOR decode failed: {e}"),
            }
        }

        // Connection dropped — back off and reconnect.
        drop(sock);
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Draw a single status line via the compositor — used for the boot
/// screen and connection-state messages. Identical to the bare-metal port.
fn boot_screen(comp: &mut Compositor, surface: &mut DpiDisplay, msg: &str) {
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
