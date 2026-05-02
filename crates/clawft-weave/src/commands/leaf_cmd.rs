//! `weaver leaf push` — push audio/display/control to leaf devices.

use clap::{Parser, Subcommand};

use weftos_leaf_types::*;

#[derive(Parser)]
#[command(about = "Leaf device control — push audio, display, and effects")]
pub struct LeafArgs {
    #[command(subcommand)]
    pub action: LeafAction,
}

#[derive(Subcommand)]
pub enum LeafAction {
    /// Push a payload to a leaf device.
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
}

#[derive(Subcommand)]
pub enum PushOp {
    /// Push a text message to a display layer.
    Text {
        /// Text to display.
        #[arg(long)]
        text: String,
        /// Display layer: bg, widget, text, alert.
        #[arg(long, default_value = "alert")]
        layer: String,
        #[arg(long, default_value_t = 0)]
        x: i32,
        #[arg(long, default_value_t = 13)]
        y: i32,
        /// RGB hex color (rrggbb, #rrggbb, or named: white/red/green/blue/yellow/cyan/magenta).
        #[arg(long, default_value = "ffffff")]
        color: String,
        /// Clear layer before drawing.
        #[arg(long)]
        clear: bool,
    },
    /// Clear a display layer.
    Clear {
        /// Display layer: bg, widget, text, alert.
        #[arg(long, default_value = "alert")]
        layer: String,
    },
    /// Set display brightness.
    Brightness {
        /// PWM on-time in microseconds.
        #[arg(long)]
        on_us: u32,
    },
    /// Apply a layer effect.
    Effect {
        /// Display layer: bg, widget, text, alert.
        #[arg(long)]
        layer: String,
        /// Effect: none or static:<keep_ratio>.
        #[arg(long, default_value = "none")]
        kind: String,
    },
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

pub async fn run(args: LeafArgs) -> anyhow::Result<()> {
    match args.action {
        LeafAction::Push { target, op, dry_run } => {
            let target_hex = target.strip_prefix("0x").unwrap_or(&target).to_lowercase();
            let payload = build_payload(op)?;
            let topic = push_topic(&target_hex);
            let cbor_bytes = encode(&payload)
                .map_err(|e| anyhow::anyhow!("CBOR encode: {e}"))?;

            if dry_run {
                println!("Target:  {target_hex}");
                println!("Topic:   {topic}");
                println!("Payload: {:?}", payload);
                println!("CBOR:    {} bytes", cbor_bytes.len());
                println!("Hex:     {}", hex_encode(&cbor_bytes));

                // Verify roundtrip.
                let decoded: LeafPush = decode(&cbor_bytes)
                    .map_err(|e| anyhow::anyhow!("roundtrip decode: {e}"))?;
                assert_eq!(decoded, payload, "roundtrip mismatch");
                println!("Roundtrip: OK");
                return Ok(());
            }

            // Publish via kernel daemon IPC.
            use crate::protocol::{IpcPublishParams, Request};

            // Base64-encode CBOR for transport over JSON RPC.
            let b64_payload = base64_encode(&cbor_bytes);
            let wire_message = serde_json::json!({
                "type": "leaf_push",
                "cbor_b64": b64_payload,
                "target_pubkey": target_hex,
            }).to_string();

            println!("Target:  {target_hex}");
            println!("Topic:   {topic}");
            println!("Payload: {} bytes CBOR", cbor_bytes.len());

            let mut client = clawft_rpc::DaemonClient::connect().await
                .ok_or_else(|| anyhow::anyhow!(
                    "cannot connect to kernel daemon.\nIs `weaver kernel start` running?"
                ))?;

            let params = serde_json::to_value(IpcPublishParams {
                topic: topic.clone(),
                message: wire_message,
                actor_id: None,
                signature: None,
                ts: None,
            })?;

            let resp = client.call(Request::with_params("ipc.publish", params)).await?;

            if !resp.ok {
                anyhow::bail!("publish failed: {}", resp.error.unwrap_or_default());
            }

            let subs = resp.result
                .as_ref()
                .and_then(|v: &serde_json::Value| v.get("subscribers"))
                .and_then(|v: &serde_json::Value| v.as_u64())
                .unwrap_or(0);

            println!("Published to '{topic}' ({subs} subscribers)");
            Ok(())
        }
    }
}

fn build_payload(op: PushOp) -> anyhow::Result<LeafPush> {
    match op {
        PushOp::Text { text, layer, x, y, color, clear } => {
            Ok(LeafPush::DisplayText(DisplayText {
                z: parse_layer(&layer)?,
                text,
                x,
                y,
                color: parse_color(&color)?,
                clear_first: clear,
            }))
        }
        PushOp::Clear { layer } => {
            Ok(LeafPush::DisplayClear(DisplayClear {
                z: parse_layer(&layer)?,
            }))
        }
        PushOp::Brightness { on_us } => {
            Ok(LeafPush::DisplayBrightness { on_us })
        }
        PushOp::Effect { layer, kind } => {
            Ok(LeafPush::LayerEffect(LayerEffectCmd {
                z: parse_layer(&layer)?,
                effect: parse_effect(&kind)?,
            }))
        }
        PushOp::Chord { freqs, gain, duration, at } => {
            let freq_list: Vec<f32> = freqs.split(',')
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
        PushOp::Scuttle { count, gain, at } => {
            Ok(LeafPush::Audio(AudioDrop::Scuttle {
                scuttles: count,
                gain,
                present_ms_from_now: at,
            }))
        }
    }
}

fn parse_layer(s: &str) -> anyhow::Result<LayerSlot> {
    match s.to_lowercase().as_str() {
        "bg" | "background" => Ok(LayerSlot::Bg),
        "widget" => Ok(LayerSlot::Widget),
        "text" => Ok(LayerSlot::Text),
        "alert" => Ok(LayerSlot::Alert),
        _ => anyhow::bail!("unknown layer: {s} (expected: bg, widget, text, alert)"),
    }
}

fn parse_color(s: &str) -> anyhow::Result<[u8; 3]> {
    let s = s.strip_prefix('#').unwrap_or(s);
    match s.to_lowercase().as_str() {
        "white" => Ok([255, 255, 255]),
        "red" => Ok([255, 0, 0]),
        "green" => Ok([0, 255, 0]),
        "blue" => Ok([0, 0, 255]),
        "yellow" => Ok([255, 255, 0]),
        "cyan" => Ok([0, 255, 255]),
        "magenta" => Ok([255, 0, 255]),
        "black" => Ok([0, 0, 0]),
        hex if hex.len() == 6 => {
            let r = u8::from_str_radix(&hex[0..2], 16)?;
            let g = u8::from_str_radix(&hex[2..4], 16)?;
            let b = u8::from_str_radix(&hex[4..6], 16)?;
            Ok([r, g, b])
        }
        _ => anyhow::bail!("bad color: {s} (expected: rrggbb, #rrggbb, or named color)"),
    }
}

fn parse_effect(s: &str) -> anyhow::Result<LayerEffectKind> {
    if s == "none" {
        return Ok(LayerEffectKind::None);
    }
    if let Some(ratio_str) = s.strip_prefix("static:") {
        let ratio: u8 = ratio_str.parse()?;
        return Ok(LayerEffectKind::Static { keep_ratio: ratio });
    }
    anyhow::bail!("bad effect: {s} (expected: none or static:<0-255>)")
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
    fn parse_colors() {
        assert_eq!(parse_color("ffffff").unwrap(), [255, 255, 255]);
        assert_eq!(parse_color("#ff0000").unwrap(), [255, 0, 0]);
        assert_eq!(parse_color("white").unwrap(), [255, 255, 255]);
        assert_eq!(parse_color("Red").unwrap(), [255, 0, 0]);
    }

    #[test]
    fn parse_layers() {
        assert_eq!(parse_layer("bg").unwrap(), LayerSlot::Bg);
        assert_eq!(parse_layer("Alert").unwrap(), LayerSlot::Alert);
        assert!(parse_layer("unknown").is_err());
    }

    #[test]
    fn parse_effects() {
        assert_eq!(parse_effect("none").unwrap(), LayerEffectKind::None);
        assert_eq!(parse_effect("static:51").unwrap(), LayerEffectKind::Static { keep_ratio: 51 });
        assert!(parse_effect("invalid").is_err());
    }

    #[test]
    fn dry_run_chord_roundtrip() {
        let payload = build_payload(PushOp::Chord {
            freqs: "440,554.37,659.25".into(),
            gain: 0.2,
            duration: 1500,
            at: 400,
        }).unwrap();
        let bytes = encode(&payload).unwrap();
        let decoded: LeafPush = decode(&bytes).unwrap();
        assert_eq!(decoded, payload);
    }
}
