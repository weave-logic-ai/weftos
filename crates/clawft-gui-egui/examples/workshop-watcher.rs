//! Workshop TOML watcher — the second-most important half of Track 5.
//!
//! Watches a local TOML file; on change, parses it into the Workshop
//! JSON shape described in
//! `crates/clawft-gui-egui/src/explorer/workshop.rs` and publishes the
//! result to `substrate/<node-id>/ui/workshop/<name>` via the running
//! daemon's Unix-socket RPC (`substrate.publish`).
//!
//! This is what makes the hot-reload loop actually *feel* live: edit
//! a TOML on disk, save, and the Explorer's Workshop pane reconfigures
//! within ~1s. No daemon restart, no GUI rebuild.
//!
//! ## Node identity (Phase 3 gate)
//!
//! Every `substrate.publish` is now node-attributed and signed. On
//! startup the example:
//!
//! 1. Generates a fresh ephemeral ed25519 keypair (one per process —
//!    no on-disk persistence, this is a developer tool).
//! 2. Computes the deterministic node-id (`n-<6-hex>` BLAKE3 prefix
//!    of the pubkey) per `clawft_kernel::node_id_from_pubkey`.
//! 3. Calls `node.register` with the canonical proof-of-possession
//!    over `node_register_payload(pubkey, ts, label)`.
//! 4. Optionally runs a one-shot diff against
//!    `substrate.canonical_publish_payload` to confirm the locally-
//!    built signing payload matches what the daemon's verifier
//!    will reconstruct.
//!
//! Each publish then signs `node_publish_payload(path, value, ts,
//! node_id)` and ships the signature in the RPC params.
//!
//! ## Usage
//!
//! ```bash
//! cargo run --example workshop-watcher -- \
//!     --toml examples/example-workshop.toml \
//!     --workshop mic-diagnostic
//! ```
//!
//! With `--once`, publishes a single snapshot and exits (useful for
//! scripting / CI / initial-seeding).
//!
//! ## Debounce
//!
//! Filesystem events for a single save often arrive in bursts (editors
//! write-then-rename; some emit MODIFY + CLOSE_WRITE pairs). A 100ms
//! debounce collapses them into one publish — the `notify` crate's
//! recommended watcher gives us precise events but not coalescence.
//!
//! ## Graceful failure modes
//!
//! * Daemon not running — retries the next tick; log once per state
//!   transition so we don't spam.
//! * TOML parse error — log the message, keep the old value on the
//!   daemon (no destructive publish of a broken shape).
//! * File missing — wait for it to appear.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;
use clawft_rpc::{DaemonClient, Request};
use ed25519_dalek::{Signer, SigningKey};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::Value;

/// File-change debounce window. Saves from editors like vim
/// (`:w` → rename + write) often emit two events within ~40ms; 100ms
/// is the smallest window that reliably collapses them to one publish
/// on my machine without feeling laggy when triggered by hand.
const DEBOUNCE: Duration = Duration::from_millis(100);

/// Retry cadence when publishing fails (daemon not running, socket
/// gone, …). We don't want to hammer the socket on every tick.
const RETRY_BACKOFF: Duration = Duration::from_secs(2);

#[derive(Parser, Debug)]
#[command(
    name = "workshop-watcher",
    about = "Watch a TOML file; publish it as a Workshop value to substrate."
)]
struct Args {
    /// Path to the TOML file describing the Workshop.
    #[arg(long)]
    toml: PathBuf,

    /// Workshop name — appended to `substrate/<node-id>/ui/workshop/`
    /// to form the publish path.
    #[arg(long)]
    workshop: String,

    /// Publish once and exit. Useful for scripts / CI / seeding.
    #[arg(long, default_value_t = false)]
    once: bool,

    /// Override the substrate publish path entirely. When set,
    /// `--workshop` is only used for log decoration. Any override
    /// MUST sit under `substrate/<this-node-id>/...` or the daemon's
    /// node-identity gate rejects the publish.
    #[arg(long)]
    path: Option<String>,
}

/// A registered local node — ephemeral ed25519 keypair plus the
/// node-id the daemon assigned us. All `substrate.publish` calls
/// route through this so the publishes are correctly signed.
///
/// "Inlined from `clawft-weave/tests/substrate_rpc.rs` `TestNode`"
/// is a deliberate pattern: we don't depend on the test helper
/// publicly, but its shape is the canonical reference for "small
/// client that registers, signs, publishes."
struct LocalNode {
    sk: SigningKey,
    node_id: String,
    label: String,
}

impl LocalNode {
    /// Mint a fresh keypair, register with the daemon, return the
    /// resulting `LocalNode`. The keypair is ephemeral — every run
    /// gets a new node-id. That's fine for a developer tool: nothing
    /// downstream pins the workshop's identity, and the daemon's
    /// registry is in-memory and rebuilt on each daemon boot anyway.
    async fn register(client: &mut DaemonClient, label: String) -> anyhow::Result<Self> {
        // Generate an ephemeral key from the OS RNG. ed25519-dalek's
        // `SigningKey::generate` requires a CSPRNG; `rand::rngs::OsRng`
        // satisfies that and is what the rest of the workspace already
        // uses (see `clawft_kernel::chain` for the persisted-key
        // sibling pattern).
        use rand::rngs::OsRng;
        let sk = SigningKey::generate(&mut OsRng);
        let pk = sk.verifying_key().to_bytes();

        let ts: u64 = unix_ms();
        let payload = clawft_kernel::node_registry::node_register_payload(&pk, ts, &label);
        let proof = sk.sign(&payload);

        let req_params = serde_json::json!({
            "label": label,
            "pubkey": hex_encode(&pk),
            "proof": hex_encode(&proof.to_bytes()),
            "ts": ts,
        });
        let resp = client
            .call(Request::with_params("node.register", req_params))
            .await?;
        if !resp.ok {
            anyhow::bail!(
                "node.register: {}",
                resp.error.unwrap_or_else(|| "unknown error".into())
            );
        }
        let result = resp.result.unwrap_or(Value::Null);
        let node_id = result
            .get("node_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("node.register: missing node_id in response"))?
            .to_string();

        // Cross-check against the locally-derived id; any drift here
        // means the daemon and our copy of `node_id_from_pubkey`
        // disagree on the derivation, which would silently break
        // publishes.
        let expected = clawft_kernel::node_id_from_pubkey(&pk);
        if expected != node_id {
            anyhow::bail!(
                "node.register: daemon returned {node_id} but local derivation gives {expected}"
            );
        }

        Ok(Self { sk, node_id, label })
    }

    /// `substrate/<node-id>/<suffix>` — convenience for path-building.
    fn ns_path(&self, suffix: &str) -> String {
        format!("substrate/{}/{suffix}", self.node_id)
    }

    /// Sign and send a `substrate.publish` for this node. Returns
    /// the new tick on success.
    async fn publish(
        &self,
        client: &mut DaemonClient,
        path: &str,
        value: Value,
    ) -> anyhow::Result<u64> {
        let ts: u64 = unix_ms();
        let value_bytes = serde_json::to_vec(&value)?;
        let value_str = String::from_utf8_lossy(&value_bytes);
        let payload = clawft_kernel::node_publish_payload(path, &value_str, ts, &self.node_id);
        let sig = self.sk.sign(&payload);

        let params = serde_json::json!({
            "path": path,
            "value": value,
            "node_id": self.node_id,
            "node_signature": hex_encode(&sig.to_bytes()),
            "node_ts": ts,
        });
        let resp = client
            .call(Request::with_params("substrate.publish", params))
            .await?;
        if !resp.ok {
            anyhow::bail!(
                "substrate.publish: {}",
                resp.error.unwrap_or_else(|| "unknown error".into())
            );
        }
        let result = resp.result.unwrap_or(Value::Null);
        Ok(result.get("tick").and_then(Value::as_u64).unwrap_or(0))
    }

    /// One-time diagnostic: ask the daemon for the canonical bytes
    /// it would feed to `Ed25519::verify(...)` for a small probe
    /// publish, then byte-compare against the same payload built
    /// locally. If they match, our `node_publish_payload` view of the
    /// world is identical to the daemon's — every subsequent signed
    /// publish should verify. If they disagree, we fail loudly here
    /// rather than at the first publish, which makes the failure
    /// mode obvious during bring-up.
    async fn self_check(&self, client: &mut DaemonClient) -> anyhow::Result<()> {
        let probe_path = self.ns_path("ui/workshop/_self_check_probe");
        let probe_value = serde_json::json!({"probe": "self-check"});
        let ts: u64 = 1; // any opaque nonce; we never publish this
        let value_bytes = serde_json::to_vec(&probe_value)?;
        let local_payload = clawft_kernel::node_publish_payload(
            &probe_path,
            &String::from_utf8_lossy(&value_bytes),
            ts,
            &self.node_id,
        );

        let params = serde_json::json!({
            "path": probe_path,
            "value": probe_value,
            "node_id": self.node_id,
            "node_ts": ts,
        });
        let resp = client
            .call(Request::with_params(
                "substrate.canonical_publish_payload",
                params,
            ))
            .await?;
        if !resp.ok {
            // Self-check is best-effort: if the daemon doesn't wire
            // the diagnostic RPC (older build), don't fail the
            // example — just log and move on.
            eprintln!(
                "[workshop-watcher] self-check: canonical_publish_payload \
                 unavailable ({}); skipping",
                resp.error.unwrap_or_else(|| "unknown error".into())
            );
            return Ok(());
        }
        let result = resp.result.unwrap_or(Value::Null);
        let daemon_hex = result
            .get("payload_hex")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("self-check: missing payload_hex in response"))?;
        let local_hex = hex_encode(&local_payload);
        if daemon_hex != local_hex {
            anyhow::bail!(
                "self-check: canonical payload mismatch.\n  local:  {local_hex}\n  daemon: {daemon_hex}\n  \
                 the local node_publish_payload disagrees with the daemon's verifier; \
                 publishes would fail signature verification."
            );
        }
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Build the tokio runtime once; publishes reuse it.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    // Register a node up front — every publish needs an attribution.
    let mut client = rt
        .block_on(DaemonClient::connect())
        .ok_or_else(|| anyhow::anyhow!("no daemon running (is `weft daemon` up?)"))?;
    let label = format!("workshop-watcher/{}", args.workshop);
    let node = rt.block_on(LocalNode::register(&mut client, label.clone()))?;
    eprintln!(
        "[workshop-watcher] registered node {} ({})",
        node.node_id, node.label,
    );

    // Boot self-check: confirm our payload-builder agrees with the
    // daemon's verifier byte-for-byte. Cheap — one extra round-trip.
    if let Err(e) = rt.block_on(node.self_check(&mut client)) {
        eprintln!("[workshop-watcher] self-check failed: {e}");
        std::process::exit(1);
    }

    // Resolve the publish path. When the user gave `--path`, we use
    // it verbatim, but enforce that it sits under our node namespace
    // — a foreign-prefix path will just bounce off the daemon's gate.
    let publish_path = match args.path.clone() {
        Some(p) => {
            let prefix = clawft_kernel::node_registry::required_path_prefix(&node.node_id);
            if !p.starts_with(&prefix) {
                anyhow::bail!(
                    "--path={p} does not sit under {prefix} (this watcher's node namespace)"
                );
            }
            p
        }
        None => node.ns_path(&format!("ui/workshop/{}", args.workshop)),
    };

    eprintln!(
        "[workshop-watcher] toml={} path={} mode={}",
        args.toml.display(),
        publish_path,
        if args.once { "once" } else { "watch" },
    );

    // Initial publish — the --once short-circuit.
    match read_and_convert(&args.toml) {
        Ok(value) => match rt.block_on(node.publish(&mut client, &publish_path, value)) {
            Ok(tick) => {
                eprintln!("[workshop-watcher] initial publish ok tick={tick}");
            }
            Err(e) => {
                eprintln!("[workshop-watcher] initial publish failed: {e}");
                if args.once {
                    std::process::exit(1);
                }
            }
        },
        Err(e) => {
            eprintln!("[workshop-watcher] initial read failed: {e}");
            if args.once {
                std::process::exit(1);
            }
        }
    }

    if args.once {
        return Ok(());
    }

    // Set up the file watcher. `RecommendedWatcher` picks the best
    // backend per-OS (inotify on Linux, FSEvents on macOS, etc.).
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(tx)?;

    // Watch the parent directory rather than the file itself — many
    // editors replace the file (rename-over) on save, which detaches a
    // file-level watch. Directory watches survive that.
    let watch_dir = args
        .toml
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;

    eprintln!("[workshop-watcher] watching {}", watch_dir.display());

    let canonical_target = args
        .toml
        .canonicalize()
        .unwrap_or_else(|_| args.toml.clone());

    // Debounce + retry loop. `last_event_at` tracks when we last saw
    // a relevant change; we publish at most once per debounce window.
    let mut pending: Option<Instant> = None;
    loop {
        // Wait for either a filesystem event or the debounce timer
        // expiring.
        let timeout = pending
            .map(|when| {
                let elapsed = when.elapsed();
                if elapsed >= DEBOUNCE {
                    Duration::ZERO
                } else {
                    DEBOUNCE - elapsed
                }
            })
            .unwrap_or(Duration::from_millis(500));

        match rx.recv_timeout(timeout) {
            Ok(Ok(event)) => {
                if is_relevant(&event, &canonical_target) {
                    pending = Some(Instant::now());
                }
            }
            Ok(Err(e)) => {
                eprintln!("[workshop-watcher] watch error: {e}");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Fall through; below we check if a pending publish
                // is now past its debounce deadline.
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("watcher channel disconnected");
            }
        }

        if let Some(when) = pending
            && when.elapsed() >= DEBOUNCE
        {
            pending = None;
            match read_and_convert(&args.toml) {
                Ok(value) => match rt.block_on(node.publish(&mut client, &publish_path, value)) {
                    Ok(tick) => {
                        eprintln!("[workshop-watcher] publish ok tick={tick}");
                    }
                    Err(e) => {
                        eprintln!("[workshop-watcher] publish failed: {e}");
                        std::thread::sleep(RETRY_BACKOFF);
                    }
                },
                Err(e) => {
                    eprintln!("[workshop-watcher] parse failed: {e}");
                }
            }
        }
    }
}

/// Is this event relevant to our target file? Filters out unrelated
/// sibling files in the watched directory and noise events (access,
/// metadata-only).
fn is_relevant(event: &Event, target: &Path) -> bool {
    match event.kind {
        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_) => {}
        _ => return false,
    }
    // Any path in the event that resolves to the target matches. We
    // canonicalize both sides so `./foo.toml` and `/abs/foo.toml`
    // compare equal.
    event.paths.iter().any(|p| {
        let canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
        canonical == target || p == target
    })
}

/// Read the TOML file and convert it to the Workshop JSON shape.
/// Accepts either a bare TOML document (matching the Workshop schema)
/// or a nested `[workshop]` table — both are common conventions.
fn read_and_convert(toml_path: &Path) -> anyhow::Result<Value> {
    let text = std::fs::read_to_string(toml_path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", toml_path.display()))?;
    let parsed: toml::Value =
        toml::from_str(&text).map_err(|e| anyhow::anyhow!("toml parse: {e}"))?;

    // If the document has a `[workshop]` table, unwrap it; otherwise
    // treat the whole document as the Workshop.
    let workshop_value: toml::Value = match parsed {
        toml::Value::Table(mut table) => {
            if let Some(w) = table.remove("workshop") {
                w
            } else {
                toml::Value::Table(table)
            }
        }
        other => other,
    };

    // Serialize through serde_json to land in the wire shape the
    // daemon expects.
    let json_value: Value =
        serde_json::to_value(workshop_value).map_err(|e| anyhow::anyhow!("toml→json: {e}"))?;

    // Sanity: must be an object. The parsed Workshop side will
    // reject deeper mistakes; here we short-circuit on the most
    // obvious one so the publish log is actionable.
    if !json_value.is_object() {
        anyhow::bail!("workshop TOML must produce a JSON object at top level");
    }
    Ok(json_value)
}

/// Hex-encode a byte slice. Matches the lowercase fixed-width form
/// the daemon's `decode_bytes` parser accepts as the canonical hex
/// shape (see `clawft-weave/src/daemon.rs::decode_bytes`).
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Wall-clock unix milliseconds. Used as the monotonic nonce on
/// signing payloads. The daemon checks the signature, not the value,
/// so any opaque nonce works as long as the same bytes go into the
/// signature and into the request.
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
