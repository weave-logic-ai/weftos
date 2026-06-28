//! `weaver leaf` — host-side producer commands for leaf devices.
//!
//! ## Topology
//!
//! Two distinct payload pipelines ride out of this command:
//!
//! 1. **Display** (`weaver leaf scene …`) — emits
//!    [`weftos_leaf_scene::SceneEnvelope`] on `mesh.leaf.<pk>.push`.
//!    The leaf decodes via `weftos_leaf_scene::codec::decode_scene_envelope`
//!    and feeds ops through `SceneStore::apply`. Per the vector-leaf
//!    design (`docs/design/vector-leaf-display.md` §4.3), this is the
//!    only display path; the old `LeafPush::Display*` variants are gone.
//!
//! 2. **Audio** (`weaver leaf push chord|scuttle`) — emits
//!    [`weftos_leaf_types::LeafPush::Audio`] on the same mesh topic.
//!    The outer `LeafPush` envelope survives because the design doc
//!    §C deliberately keeps audio + brightness on the existing topic
//!    fan-out semantics. The leaf still has `LeafPush` decode for
//!    these variants.
//!
//! ## Snapshot / diff cadence (`weaver leaf scene ps`)
//!
//! The `ps` producer keeps the **previous scene state** in
//! `~/.clawft/leaf-state/<pk>-<display>.cbor` (a CBOR-serialized
//! `Scene`). On every invocation:
//!
//! - Load the previous scene (if any) into a `SceneStore`.
//! - Build the current frame via [`weftos_scene_builder::SceneBuilder`].
//! - If no previous state: emit `SceneEnvelope { ops: [Replace(Scene)] }`.
//! - Otherwise: emit `SceneEnvelope { ops: diff(prev, next) }`.
//! - Save the new state back to disk for the next invocation.
//!
//! Producers wanting periodic refresh just call the command repeatedly
//! (or via `scripts/leaf-push-ps.sh`'s loop wrapper).

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use weftos_leaf_scene::{DisplayId, Scene, SceneEnvelope, SceneOp, SceneStore, codec};
use weftos_leaf_types::{AudioDrop, LeafPush, encode as encode_leaf_push, push_topic};

#[derive(Parser)]
#[command(about = "Leaf device control — push vector scenes + audio to leaf devices")]
pub struct LeafArgs {
    #[command(subcommand)]
    pub action: LeafAction,
}

#[derive(Subcommand)]
pub enum LeafAction {
    /// Push an audio payload to a leaf device.
    ///
    /// Display payloads now live under `weaver leaf scene`; the old
    /// `text` / `clear` / `brightness` / `effect` subcommands are
    /// removed (see docs/design/vector-leaf-display.md §10 Migration
    /// Plan, "Phase E").
    Push {
        /// Target leaf pubkey (hex, with or without 0x prefix).
        #[arg(short, long)]
        target: String,
        #[command(subcommand)]
        op: PushOp,
        /// Print encoded payload and exit without sending.
        #[arg(long)]
        dry_run: bool,
    },
    /// Vector-scene operations — host → leaf display path.
    Scene {
        #[command(subcommand)]
        op: SceneAction,
    },
}

#[derive(Subcommand)]
pub enum PushOp {
    /// Push audio chord.
    Chord {
        /// Comma-separated frequencies in Hz.
        #[arg(long)]
        freqs: String,
        /// Gain 0.0–1.0.
        #[arg(long, default_value_t = 0.3)]
        gain: f32,
        /// Duration in milliseconds.
        #[arg(long, default_value_t = 1500)]
        duration: u32,
        /// Present delay in milliseconds from now.
        #[arg(long, default_value_t = 0)]
        at: u32,
    },
    /// Push procedural crab scuttle sound.
    Scuttle {
        /// Number of scuttles.
        #[arg(long, default_value_t = 1)]
        count: u32,
        /// Gain 0.0–1.0.
        #[arg(long, default_value_t = 0.5)]
        gain: f32,
        /// Present delay in milliseconds.
        #[arg(long, default_value_t = 0)]
        at: u32,
    },
}

#[derive(Subcommand)]
pub enum SceneAction {
    /// Push a `Vec<SceneOp>` as a single envelope. The ops file is
    /// JSON-serialized `Vec<SceneOp>` (the wire format is CBOR but JSON
    /// is the producer-friendly source format — we transcode internally).
    Push {
        /// Target leaf pubkey (hex, with or without 0x prefix).
        #[arg(short, long)]
        target: String,
        /// Display id on the leaf (0 for single-display leaves).
        #[arg(long, default_value_t = 0)]
        display: DisplayId,
        /// Path to a JSON file containing `Vec<SceneOp>`.
        ops_file: PathBuf,
        /// Print encoded payload and exit without sending.
        #[arg(long)]
        dry_run: bool,
    },
    /// Emit a `Clear` op for the given display.
    Clear {
        #[arg(short, long)]
        target: String,
        #[arg(long, default_value_t = 0)]
        display: DisplayId,
        #[arg(long)]
        dry_run: bool,
    },
    /// Push a full-scene snapshot — `Replace(Scene)`. The scene file is
    /// JSON-serialized [`weftos_leaf_scene::Scene`].
    Snapshot {
        #[arg(short, long)]
        target: String,
        #[arg(long, default_value_t = 0)]
        display: DisplayId,
        scene_file: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    /// Produce a kernel.ps scene and push it as a delta (or snapshot
    /// if this is the first run for this leaf).
    ///
    /// The previous scene state is cached at
    /// `~/.clawft/leaf-state/<target>-<display>.cbor`; deleting that
    /// file forces a fresh snapshot on the next invocation.
    Ps {
        #[arg(short, long)]
        target: String,
        #[arg(long, default_value_t = 0)]
        display: DisplayId,
        /// Force a full snapshot (Replace(Scene)) even if a cached
        /// previous state exists.
        #[arg(long)]
        snapshot: bool,
        #[arg(long)]
        dry_run: bool,
    },
}

pub async fn run(args: LeafArgs) -> anyhow::Result<()> {
    match args.action {
        LeafAction::Push {
            target,
            op,
            dry_run,
        } => run_push(target, op, dry_run).await,
        LeafAction::Scene { op } => run_scene(op).await,
    }
}

// ── Audio path (LeafPush envelope) ─────────────────────────────────

async fn run_push(target: String, op: PushOp, dry_run: bool) -> anyhow::Result<()> {
    let target_hex = normalize_target(&target);
    let payload = build_audio_payload(op)?;
    let topic = push_topic(&target_hex);
    let cbor_bytes = encode_leaf_push(&payload).map_err(|e| anyhow::anyhow!("CBOR encode: {e}"))?;

    if dry_run {
        println!("Target:   {target_hex}");
        println!("Topic:    {topic}");
        println!("Envelope: LeafPush::Audio (outer envelope)");
        println!("Payload:  {payload:?}");
        println!("CBOR:     {} bytes", cbor_bytes.len());
        println!("Hex:      {}", hex_encode(&cbor_bytes));

        // Roundtrip — confirms our serde stays symmetric with the leaf's decode.
        let decoded: LeafPush = weftos_leaf_types::decode(&cbor_bytes)
            .map_err(|e| anyhow::anyhow!("roundtrip decode: {e}"))?;
        assert_eq!(decoded, payload, "roundtrip mismatch");
        println!("Roundtrip: OK");
        return Ok(());
    }

    publish_cbor(&topic, "leaf_push", &cbor_bytes, &target_hex).await
}

fn build_audio_payload(op: PushOp) -> anyhow::Result<LeafPush> {
    match op {
        PushOp::Chord {
            freqs,
            gain,
            duration,
            at,
        } => {
            let freq_list: Vec<f32> = freqs
                .split(',')
                .map(|s| s.trim().parse::<f32>())
                .collect::<Result<_, _>>()
                .map_err(|e| anyhow::anyhow!("bad frequency: {e}"))?;
            Ok(LeafPush::Audio(AudioDrop::Chord {
                freqs: freq_list,
                peak_gain: gain,
                duration_ms: duration,
                present_ms_from_now: at,
            }))
        }
        PushOp::Scuttle { count, gain, at } => Ok(LeafPush::Audio(AudioDrop::Scuttle {
            scuttles: count,
            gain,
            present_ms_from_now: at,
        })),
    }
}

// ── Display path (SceneEnvelope) ───────────────────────────────────

async fn run_scene(op: SceneAction) -> anyhow::Result<()> {
    match op {
        SceneAction::Push {
            target,
            display,
            ops_file,
            dry_run,
        } => {
            let target_hex = normalize_target(&target);
            let raw = std::fs::read_to_string(&ops_file)
                .map_err(|e| anyhow::anyhow!("read {}: {e}", ops_file.display()))?;
            let ops: Vec<SceneOp> =
                serde_json::from_str(&raw).map_err(|e| anyhow::anyhow!("decode ops JSON: {e}"))?;
            let env = SceneEnvelope::new(display, ops);
            publish_scene_envelope(&target_hex, &env, dry_run).await
        }
        SceneAction::Clear {
            target,
            display,
            dry_run,
        } => {
            let target_hex = normalize_target(&target);
            let env = SceneEnvelope::single(display, SceneOp::Clear);
            publish_scene_envelope(&target_hex, &env, dry_run).await
        }
        SceneAction::Snapshot {
            target,
            display,
            scene_file,
            dry_run,
        } => {
            let target_hex = normalize_target(&target);
            let raw = std::fs::read_to_string(&scene_file)
                .map_err(|e| anyhow::anyhow!("read {}: {e}", scene_file.display()))?;
            let mut scene: Scene = serde_json::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("decode Scene JSON: {e}"))?;
            // Normalize: if the caller put the wrong display id in the
            // Scene body, the envelope's display id wins.
            scene.display_id = display;
            let env = SceneEnvelope::single(display, SceneOp::Replace(scene));
            publish_scene_envelope(&target_hex, &env, dry_run).await
        }
        SceneAction::Ps {
            target,
            display,
            snapshot,
            dry_run,
        } => {
            let target_hex = normalize_target(&target);
            run_scene_ps(&target_hex, display, snapshot, dry_run).await
        }
    }
}

/// CBOR-encode a `SceneEnvelope` and publish (or dry-run print) it.
async fn publish_scene_envelope(
    target_hex: &str,
    env: &SceneEnvelope,
    dry_run: bool,
) -> anyhow::Result<()> {
    let topic = push_topic(target_hex);
    let cbor_bytes =
        codec::encode(env).map_err(|e| anyhow::anyhow!("CBOR encode SceneEnvelope: {e}"))?;

    if dry_run {
        println!("Target:    {target_hex}");
        println!("Topic:     {topic}");
        println!(
            "Envelope:  SceneEnvelope v{} display={}",
            env.version, env.display_id
        );
        println!("Ops:       {} ({})", env.ops.len(), op_summary(&env.ops));
        println!("CBOR:      {} bytes", cbor_bytes.len());

        // Roundtrip for sanity.
        let decoded = codec::decode_scene_envelope(&cbor_bytes)
            .map_err(|e| anyhow::anyhow!("roundtrip decode: {e}"))?;
        assert_eq!(&decoded, env, "roundtrip mismatch");
        println!("Roundtrip: OK");
        return Ok(());
    }

    publish_cbor(&topic, "scene_push", &cbor_bytes, target_hex).await
}

/// Render `kernel.ps` into a scene via `SceneBuilder`, diff against the
/// cached previous state on disk, push the resulting envelope.
async fn run_scene_ps(
    target_hex: &str,
    display: DisplayId,
    force_snapshot: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    use crate::protocol;

    // 1. Fetch the process table via the daemon.
    let mut client = clawft_rpc::DaemonClient::connect().await.ok_or_else(|| {
        anyhow::anyhow!("cannot connect to kernel daemon.\nIs `weaver kernel start` running?")
    })?;
    let resp = client.simple_call("kernel.ps").await?;
    if !resp.ok {
        anyhow::bail!("kernel.ps failed: {}", resp.error.unwrap_or_default());
    }
    let entries: Vec<protocol::ProcessInfo> = serde_json::from_value(
        resp.result
            .ok_or_else(|| anyhow::anyhow!("kernel.ps returned no result"))?,
    )?;

    // 2. Build the current scene.
    let current_store = build_ps_scene(&entries, display);
    let current_scene = current_store.to_snapshot(display);

    // 3. Load the cached previous scene (if any).
    let state_path = ps_state_path(target_hex, display)?;
    let prev_store: Option<SceneStore> = if force_snapshot {
        None
    } else {
        load_state(&state_path)?
    };

    // 4. Diff (or snapshot if first run / forced).
    let env = match prev_store {
        Some(ref prev) => {
            let ops = weftos_scene_builder::diff(prev, &current_store, display);
            if ops.is_empty() {
                println!(
                    "{} processes; no scene changes since last push (state cached at {})",
                    entries.len(),
                    state_path.display(),
                );
                return Ok(());
            }
            SceneEnvelope::new(display, ops)
        }
        None => {
            let env = weftos_scene_builder::to_envelope(&current_store, display);
            println!(
                "{} processes; first-run snapshot (no cached state at {})",
                entries.len(),
                state_path.display(),
            );
            env
        }
    };

    // 5. Publish.
    let result = publish_scene_envelope(target_hex, &env, dry_run).await;

    // 6. Persist the new state — even on dry-run, so subsequent runs
    //    can diff against this view. The producer state is local to
    //    the operator's machine, not the leaf.
    if result.is_ok() && !dry_run {
        save_state(&state_path, &current_scene)?;
    }

    result
}

fn build_ps_scene(entries: &[crate::protocol::ProcessInfo], display: DisplayId) -> SceneStore {
    use weftos_leaf_scene::{Layer, Rgba};
    use weftos_scene_builder::SceneBuilder;

    // Layout in display pixels. 10 px gutter on every side — the
    // CrowPanel's RGB panel has timing porches that can manifest
    // visible artifacts in the outermost few pixels; pulling content
    // inset 10 px on all sides keeps glyphs comfortably inside the
    // stable region. Effective drawable area: 780 × 460.
    const PANEL_W: i32 = 800;
    const PANEL_H: i32 = 480;
    const GUTTER: i32 = 50;
    // First-glyph top-left baseline. FONT_6X10 has ascent ~9, so
    // baseline at y = GUTTER + ascent ≈ 19 keeps the glyph wholly
    // inside the gutter; we round up to GUTTER + ROW_H = 26 so the
    // header row sits on the same grid as the data rows below it.
    const X0: i32 = GUTTER;
    const ROW_H: i32 = 16;
    const Y0: i32 = GUTTER + ROW_H; // 26

    let mut b = SceneBuilder::new("kernel.ps", display);
    b.viewport(PANEL_W, PANEL_H)
        .bg(Rgba::opaque(0x00, 0x00, 0x00));

    // Header — cyan, leftmost column.
    let cyan = Rgba::new(0x00, 0xFF, 0xFF, 0xFF);
    let header = b.text(Layer::Text, "PID     AGENT             STATE", X0, Y0, cyan);
    b.insert("ps.header", header);

    // One row per process. Column widths match the legacy bash producer
    // so existing operator muscle memory survives.
    for (i, entry) in entries.iter().enumerate() {
        let y = Y0 + ROW_H * (i as i32 + 1);
        let color = match entry.state.as_str() {
            "running" => Rgba::new(0x00, 0xFF, 0x00, 0xFF), // green
            "degraded" => Rgba::new(0xFF, 0xFF, 0x00, 0xFF), // yellow
            "failed" | "stopped" => Rgba::new(0xFF, 0x00, 0x00, 0xFF), // red
            _ => Rgba::WHITE,
        };
        // Pad like the legacy script: %-7s %-17s %s.
        let line = format!(
            "{:<7} {:<17} {}",
            entry.pid,
            truncate(&entry.agent_id, 17),
            entry.state
        );
        let node = b.text(Layer::Text, line, X0, y, color);
        let path = format!("ps.row[{i}]");
        b.insert(path, node);
    }

    b.build()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect()
    }
}

// ── State cache (~/.clawft/leaf-state/<pk>-<display>.cbor) ──────────

fn ps_state_path(target_hex: &str, display: DisplayId) -> anyhow::Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("$HOME not set; cannot cache leaf state"))?;
    let dir = home.join(".clawft").join("leaf-state");
    std::fs::create_dir_all(&dir).map_err(|e| anyhow::anyhow!("create {}: {e}", dir.display()))?;
    Ok(dir.join(format!("{target_hex}-{display}.cbor")))
}

fn load_state(path: &std::path::Path) -> anyhow::Result<Option<SceneStore>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let scene: Scene = codec::decode(&bytes)
                .map_err(|e| anyhow::anyhow!("decode cached Scene at {}: {e}", path.display()))?;
            let mut store = SceneStore::new();
            let _ = store.apply_op(scene.display_id, &SceneOp::Replace(scene));
            Ok(Some(store))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!("read {}: {e}", path.display())),
    }
}

fn save_state(path: &std::path::Path, scene: &Scene) -> anyhow::Result<()> {
    let bytes =
        codec::encode(scene).map_err(|e| anyhow::anyhow!("encode Scene for state cache: {e}"))?;
    std::fs::write(path, bytes).map_err(|e| anyhow::anyhow!("write {}: {e}", path.display()))?;
    Ok(())
}

// ── Daemon publish (shared between audio + scene paths) ─────────────

async fn publish_cbor(
    topic: &str,
    wire_kind: &str,
    cbor_bytes: &[u8],
    target_hex: &str,
) -> anyhow::Result<()> {
    use crate::protocol::{IpcPublishParams, Request};

    let b64_payload = base64_encode(cbor_bytes);
    let wire_message = serde_json::json!({
        "type": wire_kind,
        "cbor_b64": b64_payload,
        "target_pubkey": target_hex,
    })
    .to_string();

    println!("Target:  {target_hex}");
    println!("Topic:   {topic}");
    println!("Payload: {} bytes CBOR ({wire_kind})", cbor_bytes.len());

    let mut client = clawft_rpc::DaemonClient::connect().await.ok_or_else(|| {
        anyhow::anyhow!("cannot connect to kernel daemon.\nIs `weaver kernel start` running?")
    })?;

    let params = serde_json::to_value(IpcPublishParams {
        topic: topic.to_string(),
        message: wire_message,
        actor_id: None,
        signature: None,
        ts: None,
    })?;

    let resp = client
        .call(Request::with_params("ipc.publish", params))
        .await?;

    if !resp.ok {
        anyhow::bail!("publish failed: {}", resp.error.unwrap_or_default());
    }

    let subs = resp
        .result
        .as_ref()
        .and_then(|v: &serde_json::Value| v.get("subscribers"))
        .and_then(|v: &serde_json::Value| v.as_u64())
        .unwrap_or(0);

    println!("Published to '{topic}' ({subs} subscribers)");
    Ok(())
}

// ── Small utilities (shared) ────────────────────────────────────────

fn normalize_target(s: &str) -> String {
    s.strip_prefix("0x").unwrap_or(s).to_lowercase()
}

fn op_summary(ops: &[SceneOp]) -> String {
    let mut inserts = 0;
    let mut updates = 0;
    let mut removes = 0;
    let mut replaces = 0;
    let mut clears = 0;
    let mut others = 0;
    for op in ops {
        match op {
            SceneOp::Insert(_) => inserts += 1,
            SceneOp::Update(_) => updates += 1,
            SceneOp::Remove(_) => removes += 1,
            SceneOp::Replace(_) => replaces += 1,
            SceneOp::Clear => clears += 1,
            _ => others += 1,
        }
    }
    format!("ins={inserts} upd={updates} rem={removes} repl={replaces} clr={clears} other={others}")
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_target_strips_prefix_and_lowercases() {
        assert_eq!(normalize_target("0xABCDEF"), "abcdef");
        assert_eq!(normalize_target("AbCdEf"), "abcdef");
        assert_eq!(normalize_target("abcdef"), "abcdef");
    }

    #[test]
    fn truncate_respects_char_count() {
        assert_eq!(truncate("short", 17), "short");
        assert_eq!(truncate("0123456789abcdefghij", 5), "01234");
    }

    #[test]
    fn build_audio_payload_chord_roundtrip() {
        let payload = build_audio_payload(PushOp::Chord {
            freqs: "440,554.37,659.25".into(),
            gain: 0.2,
            duration: 1500,
            at: 400,
        })
        .unwrap();
        let bytes = encode_leaf_push(&payload).unwrap();
        let decoded: LeafPush = weftos_leaf_types::decode(&bytes).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn build_ps_scene_emits_nodes_for_each_process() {
        let entries = vec![
            crate::protocol::ProcessInfo {
                pid: 1,
                agent_id: "kernel".into(),
                state: "running".into(),
                memory_bytes: 0,
                cpu_time_ms: 0,
                parent_pid: None,
            },
            crate::protocol::ProcessInfo {
                pid: 2,
                agent_id: "agent-foo".into(),
                state: "degraded".into(),
                memory_bytes: 0,
                cpu_time_ms: 0,
                parent_pid: Some(1),
            },
        ];
        let store = build_ps_scene(&entries, 0);
        let display = store.display(0).expect("display 0");
        // 1 header + 2 rows = 3 nodes.
        assert_eq!(display.nodes.len(), 3);
    }

    #[test]
    fn ps_scene_diffs_to_minimal_envelope() {
        let entries_v1 = vec![crate::protocol::ProcessInfo {
            pid: 1,
            agent_id: "kernel".into(),
            state: "running".into(),
            memory_bytes: 0,
            cpu_time_ms: 0,
            parent_pid: None,
        }];
        let entries_v2 = vec![crate::protocol::ProcessInfo {
            pid: 1,
            agent_id: "kernel".into(),
            state: "degraded".into(), // state changed
            memory_bytes: 0,
            cpu_time_ms: 0,
            parent_pid: None,
        }];

        let s1 = build_ps_scene(&entries_v1, 0);
        let s2 = build_ps_scene(&entries_v2, 0);

        // 1 row changed; expect exactly 1 Update.
        let ops = weftos_scene_builder::diff(&s1, &s2, 0);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], SceneOp::Update(_)));
    }

    #[test]
    fn op_summary_counts_variants() {
        let ops = vec![
            SceneOp::Clear,
            SceneOp::Remove(weftos_leaf_scene::NodeId::from_raw(0)),
            SceneOp::Remove(weftos_leaf_scene::NodeId::from_raw(1)),
        ];
        let s = op_summary(&ops);
        assert!(s.contains("rem=2"));
        assert!(s.contains("clr=1"));
    }
}
