//! Mesh transport client — **Phase E** vector ingest + input publish.
//!
//! Two embassy tasks ride on the existing `[kernel.mesh]` plaintext-TCP
//! transport (`noise = false` for the spike):
//!
//! 1. **Display ingest** — `mesh_task` connects, subscribes to
//!    `mesh.leaf.<pk>.push`, and feeds every received [`SceneEnvelope`]
//!    through [`SceneStore::apply`] → [`render_damage`] →
//!    [`DpiSurface`]. The shared store lives behind an
//!    [`embassy_sync::mutex::Mutex`] so the touch task can hit-test
//!    against it.
//!
//! 2. **Input publish** — `input_task` (spawned alongside the display
//!    ingest) connects on its own socket, polls the GT911, hit-tests
//!    every touch event against the shared `SceneStore`, and publishes
//!    `InputEnvelope`s on `mesh.leaf.<pk>.input`.
//!
//! Both tasks reconnect on failure; the panel keeps showing the last
//! rendered frame across reconnect (the DPI bus owns the framebuffer,
//! the mesh ingest just stops painting until subscribe completes).
//!
//! Wire protocol (see `docs/leaf-push-protocol.md` §3 and the kernel
//! `mesh_ipc.rs` / `ipc.rs` / `mesh_runtime.rs` source):
//!
//! - Framing: `[4-byte big-endian length][JSON MeshIpcEnvelope]`.
//! - Subscribe: a `MeshIpcEnvelope` whose `message.target` is
//!   `Topic("mesh.subscribe")` and `message.payload` is
//!   `Json({"topic": "<our push topic>"})`. The kernel auto-registers
//!   the peer by `source_node` and consumes the message.
//! - Outbound publish: same shape, `target = Topic("ipc.publish")`,
//!   `payload = Json({"topic":"<our_topic>", "message":"<wire_json>"})`.
//! - Inbound scene-push: a `MeshIpcEnvelope` whose inner message
//!   carries `{"type":"scene_push","cbor_b64":"<base64 CBOR>",...}`.
//!   We extract the base64, decode to CBOR, decode to `SceneEnvelope`.
//!
//! Outer-`LeafPush::Audio` envelopes (chord, scuttle) ride on the same
//! topic but currently fall through to the audio path — this firmware
//! doesn't drive a speaker so we just log them. The CBOR decode tries
//! `SceneEnvelope` first; on `VersionMismatch` we silently ignore (it
//! was probably an audio envelope, which has no `version` field in the
//! same byte position).

use alloc::string::String;
use alloc::vec::Vec;

use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, Ipv4Address, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer};
use esp_hal::i2c::master::I2c;
use esp_println::println;
use static_cell::StaticCell;

use weftos_leaf_renderer::render_damage;
use weftos_leaf_scene::{codec, SceneStore};
use weftos_leaf_types::push_topic;

use crate::drivers::dpi_surface::DpiSurface;

// ── Connection constants (spike: hardcoded) ──────────────────────────
const DAEMON_IP: Ipv4Address = Ipv4Address::new(192, 168, 1, 73);
const DAEMON_MESH_PORT: u16 = 9470;
/// Device-MAC-derived leaf identity. `weaver leaf scene push --target
/// 3cdc75fabc7c …` targets this leaf. (MAC 3c:dc:75:fa:bc:7c.)
const LEAF_ID: &str = "3cdc75fabc7c";

/// Display id this leaf serves on. Single-display leaf → 0.
const DISPLAY_ID: u8 = 0;

const RX_BUF: usize = 8192;
const TX_BUF: usize = 4096;
const FRAME_BUF: usize = 16384; // SceneEnvelope CBOR can be larger than LeafPush

/// Static cell housing the shared `Mutex<SceneStore>`. Initialised by
/// [`shared_store`] on first call (idempotent — the mutex guards the
/// store's interior, the `'static` lifetime guards the mesh + touch
/// tasks' shared access).
static SCENE_STORE: StaticCell<Mutex<CriticalSectionRawMutex, SceneStore>> = StaticCell::new();

/// Return a `&'static Mutex<SceneStore>` for cross-task sharing. Call
/// **once** at boot from `main`; multiple calls panic by design
/// (StaticCell). The store starts empty; producers MUST send a
/// `SceneOp::Replace(Scene)` snapshot as the first env after subscribe
/// (which the host's `weaver leaf scene ps` does on first run).
pub fn shared_store() -> &'static Mutex<CriticalSectionRawMutex, SceneStore> {
    SCENE_STORE.init(Mutex::new(SceneStore::new()))
}

// ── Mesh wire helpers (shared between push + input paths) ───────────

/// Hand-rolled subscribe `MeshIpcEnvelope` (fixed-shape JSON).
fn subscribe_envelope(topic: &str) -> String {
    alloc::format!(
        concat!(
            r#"{{"source_node":"{id}","dest_node":"daemon","message":{{"#,
            r#""id":"leaf-sub-1","from":0,"target":{{"Topic":"mesh.subscribe"}},"#,
            r#""payload":{{"Json":{{"topic":"{topic}"}}}},"#,
            r#""timestamp":"2026-05-15T00:00:00Z"}},"#,
            r#""hop_count":0,"envelope_id":"leaf-env-sub-1"}}"#
        ),
        id = LEAF_ID,
        topic = topic,
    )
}

/// Hand-rolled publish `MeshIpcEnvelope` for outbound CBOR (e.g.
/// `InputEnvelope`). Wraps `cbor_bytes` in base64 and addresses
/// `ipc.publish` on the daemon.
fn publish_envelope(topic: &str, wire_kind: &str, cbor_bytes: &[u8]) -> String {
    let b64 = base64_encode(cbor_bytes);
    // The inner message is a JSON-encoded string; we have to escape the
    // outer quotes by serializing twice. Using format! is fine — the
    // shape is fixed and the only injection points are
    // base64-alphabet-clean (b64) and our own constants.
    let inner = alloc::format!(
        r#"{{\"type\":\"{kind}\",\"cbor_b64\":\"{b64}\",\"target_pubkey\":\"{leaf}\"}}"#,
        kind = wire_kind,
        b64 = b64,
        leaf = LEAF_ID,
    );
    alloc::format!(
        concat!(
            r#"{{"source_node":"{id}","dest_node":"daemon","message":{{"#,
            r#""id":"leaf-pub-1","from":0,"target":{{"Topic":"ipc.publish"}},"#,
            r#""payload":{{"Json":{{"topic":"{topic}","message":"{inner}"}}}},"#,
            r#""timestamp":"2026-05-15T00:00:00Z"}},"#,
            r#""hop_count":0,"envelope_id":"leaf-env-pub-1"}}"#
        ),
        id = LEAF_ID,
        topic = topic,
        inner = inner,
    )
}

/// Write a `[4-byte BE length][payload]` frame to the socket.
async fn write_frame(sock: &mut TcpSocket<'_>, payload: &[u8]) -> Result<(), ()> {
    let len = (payload.len() as u32).to_be_bytes();
    write_all(sock, &len).await?;
    write_all(sock, payload).await?;
    Ok(())
}

async fn write_all(sock: &mut TcpSocket<'_>, mut buf: &[u8]) -> Result<(), ()> {
    while !buf.is_empty() {
        match sock.write(buf).await {
            Ok(0) => return Err(()),
            Ok(n) => buf = &buf[n..],
            Err(_) => return Err(()),
        }
    }
    Ok(())
}

async fn read_exact(sock: &mut TcpSocket<'_>, mut buf: &mut [u8]) -> Result<(), ()> {
    while !buf.is_empty() {
        match sock.read(buf).await {
            Ok(0) => return Err(()),
            Ok(n) => buf = &mut buf[n..],
            Err(_) => return Err(()),
        }
    }
    Ok(())
}

/// Find `cbor_b64`'s value inside a raw inbound envelope and base64-
/// decode it. The inner push JSON is escaped inside a `Text` payload,
/// so we scan for the key and read base64 chars until the first
/// non-alphabet byte — robust enough for the spike.
fn extract_cbor(envelope: &[u8]) -> Option<Vec<u8>> {
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

fn base64_encode(bytes: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

// ── Display ingest task ─────────────────────────────────────────────

/// Mesh client task #1 — display ingest.
///
/// Owns the `DpiSurface` for the program's lifetime. Connects to the
/// daemon, subscribes to this leaf's push topic, and feeds every
/// received `SceneEnvelope` through the renderer.
#[embassy_executor::task]
pub async fn mesh_task(
    stack: Stack<'static>,
    store: &'static Mutex<CriticalSectionRawMutex, SceneStore>,
    mut surface: DpiSurface,
) {
    let topic = push_topic(LEAF_ID);
    println!(
        "[mesh] display ingest: leaf id '{}', push topic '{}'",
        LEAF_ID, topic
    );

    loop {
        // Wait for WiFi + DHCP.
        if !stack.is_link_up() || stack.config_v4().is_none() {
            Timer::after(Duration::from_millis(500)).await;
            continue;
        }

        let mut rx = [0u8; RX_BUF];
        let mut tx = [0u8; TX_BUF];
        let mut sock = TcpSocket::new(stack, &mut rx, &mut tx);
        // Generous idle timeout — the daemon sends no keepalives, so a
        // tight timeout just churns reconnects.
        sock.set_timeout(Some(Duration::from_secs(120)));

        println!("[mesh] connecting to {}:{}", DAEMON_IP, DAEMON_MESH_PORT);
        if let Err(e) = sock
            .connect((IpAddress::Ipv4(DAEMON_IP), DAEMON_MESH_PORT))
            .await
        {
            println!("[mesh] connect failed: {:?} — retry 3s", e);
            Timer::after(Duration::from_secs(3)).await;
            continue;
        }
        println!("[mesh] connected — subscribing to '{}'", topic);

        let sub = subscribe_envelope(&topic);
        if write_frame(&mut sock, sub.as_bytes()).await.is_err() {
            println!("[mesh] subscribe write failed — retry 3s");
            Timer::after(Duration::from_secs(3)).await;
            continue;
        }

        // Receive loop: [4-byte len][JSON envelope], repeat.
        let mut frame = [0u8; FRAME_BUF];
        loop {
            let mut len_buf = [0u8; 4];
            if read_exact(&mut sock, &mut len_buf).await.is_err() {
                println!("[mesh] read len failed — reconnecting");
                break;
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            if len == 0 || len > frame.len() {
                println!("[mesh] frame len {} out of range — reconnecting", len);
                break;
            }
            if read_exact(&mut sock, &mut frame[..len]).await.is_err() {
                println!("[mesh] read body failed — reconnecting");
                break;
            }

            let Some(cbor) = extract_cbor(&frame[..len]) else {
                // Not a payload-carrying envelope. Subscribe acks,
                // peer-register acks etc. land here — silently ignore.
                continue;
            };

            // Try to decode as `SceneEnvelope` first. On version
            // mismatch the producer probably sent a `LeafPush` audio
            // payload, which we don't drive on this firmware — log
            // once and move on.
            match codec::decode_scene_envelope(&cbor) {
                Ok(env) => {
                    let op_count = env.ops.len();
                    let display_id = env.display_id;
                    let mut store_guard = store.lock().await;
                    let damage = store_guard.apply(&env);
                    let damage_count = damage.rects().len();
                    let is_full = damage.is_full();
                    match render_damage(
                        &store_guard,
                        display_id,
                        &damage,
                        &mut surface,
                    ) {
                        Ok(stats) => {
                            println!(
                                "[mesh] APPLY display={} ops={} damage_rects={} full={} drawn={}",
                                display_id, op_count, damage_count, is_full, stats.drawn
                            );
                        }
                        Err(e) => {
                            println!("[mesh] render_damage error: {:?}", e);
                        }
                    }
                }
                Err(codec::CodecError::VersionMismatch { found, expected }) => {
                    println!(
                        "[mesh] wire version mismatch (found {}, expected {}) — dropped",
                        found, expected
                    );
                }
                Err(codec::CodecError::Decode) => {
                    // Could be a LeafPush::Audio envelope (different
                    // shape). Try that decode silently; if it also
                    // fails, log once.
                    match weftos_leaf_types::decode::<weftos_leaf_types::LeafPush>(&cbor) {
                        Ok(_lp) => {
                            // Audio payload — this firmware doesn't
                            // drive a speaker yet. v1.1: hand off to
                            // an audio task.
                        }
                        Err(_) => {
                            println!(
                                "[mesh] CBOR decode failed: not SceneEnvelope nor LeafPush ({} bytes)",
                                cbor.len()
                            );
                        }
                    }
                }
                Err(codec::CodecError::Encode) => {
                    // Decode path doesn't yield Encode; defensive arm.
                }
            }
        }

        sock.close();
        Timer::after(Duration::from_secs(2)).await;
    }
}

// ── Input publish task ──────────────────────────────────────────────

/// Mesh client task #2 — input publish.
///
/// Polls the GT911 (via the new `weftos-leaf-touch-gt911` driver),
/// hit-tests each `TouchEvent` against the shared `SceneStore`, and
/// publishes the resulting `InputEnvelope` on
/// `mesh.leaf.<pk>.input`.
///
/// The task connects on its own TCP socket so the inbound subscribe
/// stream isn't interleaved with outbound publishes. The daemon's
/// mesh transport auto-registers peers by `source_node`; we reuse the
/// same `LEAF_ID` for both tasks.
#[embassy_executor::task]
pub async fn input_task(
    stack: Stack<'static>,
    store: &'static Mutex<CriticalSectionRawMutex, SceneStore>,
    i2c: I2c<'static, esp_hal::Async>,
) {
    use weftos_leaf_scene::codec as scene_codec;
    use weftos_leaf_touch_gt911::{hit_test_event, Gt911};

    // Build the topic up-front; it's stable for this leaf's lifetime.
    // The leaf publishes on `mesh.leaf.<pk>.input` — symmetric to
    // `.push` for the display direction. We construct it by string
    // surgery since `weftos-leaf-types` doesn't currently expose an
    // input_topic helper.
    let input_topic = alloc::format!("mesh.leaf.{}.input", LEAF_ID);

    // Boot the GT911. The PCA9557 reset dance has already run
    // synchronously in `main`, BEFORE `DpiSurface::new` — so the chip
    // has been out of reset since main's 100 ms post-IO1-release delay,
    // plus the 200 ms + 500 ms post-DPI delay, plus however long
    // `DpiSurface::new` took. Plenty of time to boot its scan engine.
    Timer::after(Duration::from_millis(100)).await; // small bus settle
    let mut gt911 = match Gt911::new(i2c).await {
        Ok(g) => {
            println!(
                "[input] GT911 probed OK @ 0x{:02x} (factory config left intact)",
                g.address()
            );
            g
        }
        Err(_) => {
            println!("[input] GT911 probe FAILED — both 0x14 and 0x5D unresponsive; task exiting");
            return;
        }
    };

    // Outer loop: (re)connect → publish loop → on disconnect reconnect.
    'outer: loop {
        // Wait for link up.
        if !stack.is_link_up() || stack.config_v4().is_none() {
            Timer::after(Duration::from_millis(500)).await;
            continue;
        }

        let mut rx = [0u8; RX_BUF];
        let mut tx = [0u8; TX_BUF];
        let mut sock = TcpSocket::new(stack, &mut rx, &mut tx);
        sock.set_timeout(Some(Duration::from_secs(120)));

        if let Err(e) = sock
            .connect((IpAddress::Ipv4(DAEMON_IP), DAEMON_MESH_PORT))
            .await
        {
            println!("[input] connect failed: {:?} — retry 3s", e);
            Timer::after(Duration::from_secs(3)).await;
            continue;
        }
        println!("[input] mesh socket open — publishing to '{}'", input_topic);

        // Inner publish loop — on any write failure break out and
        // reconnect.
        let mut poll: u32 = 0;
        loop {
            let events = match gt911.poll_events().await {
                Ok(ev) => ev,
                Err(_) => {
                    println!("[input] GT911 read error — pausing 500 ms");
                    Timer::after(Duration::from_millis(500)).await;
                    Vec::new()
                }
            };

            for ev in events {
                // Hit-test against the shared store.
                let env = {
                    let guard = store.lock().await;
                    hit_test_event(&guard, DISPLAY_ID, ev)
                };
                let cbor = match scene_codec::encode(&env) {
                    Ok(b) => b,
                    Err(e) => {
                        println!("[input] InputEnvelope encode failed: {:?}", e);
                        continue;
                    }
                };
                let wire = publish_envelope(&input_topic, "leaf_input", &cbor);
                if write_frame(&mut sock, wire.as_bytes()).await.is_err() {
                    println!("[input] publish write failed — reconnecting");
                    sock.close();
                    Timer::after(Duration::from_secs(2)).await;
                    continue 'outer;
                }
            }

            if poll.is_multiple_of(500) {
                println!("[input] heartbeat (poll {})", poll);
            }
            poll = poll.wrapping_add(1);
            Timer::after(Duration::from_millis(20)).await;
        }
    }
}
