//! Daemon-brain `VoiceLlm` adapter (WEFT-614 lite).
//!
//! `weft voice talk` used to run TWO brains per utterance: its own local
//! `LocalProviderVoiceLlm` (a bare Hermes prompt) AND the kernel daemon's
//! full §W2.1 voice loop, reached by mirroring every turn through
//! `agent.turn.record`. Two replies landed per turn; TTS spoke the weaker
//! local one. [`DaemonBrainVoiceLlm`] fixes that by never calling an LLM
//! itself — its `complete()` waits for the daemon's own reply to commit on
//! `conversation.graph` and hands back its text, so the daemon is the ONLY
//! brain that speaks. The user turn is still mirrored via the existing
//! recorder (unchanged); [`resolve_brain`] additionally tells the recorder
//! to stop mirroring the assistant's reply in this mode, since the daemon
//! already recorded it through its own sink.
//!
//! Ordering caveat: `complete()` is called right after the `UserTurn` event
//! fires, but the recorder posts `agent.turn.record` from a spawned task
//! (`spawn_turn_recorder` in `voice.rs`) — the user turn may not have
//! reached the daemon yet. The adapter handles this by snapshotting the
//! highest committed, non-attempt assistant `chain_seq` BEFORE waiting and
//! looking only for a node strictly newer than that snapshot, rather than
//! trying to correlate by the recorder's post count.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use clap::ValueEnum;
use clawft_rpc::{DaemonClient, Request};
use serde_json::Value;

use clawft_channels::voice::policy::{VoiceLlm, VoiceTurnRequest};
use clawft_channels::voice::types::VoiceError;

/// Poll cadence while waiting for the daemon's reply.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Hard wait ceiling. Hermes tool loops are slow; this only bounds a single
/// voice turn's stall before the policy layer degrades like any other brain
/// failure (see [`resolve_brain`]'s printed caveat on `voice_loop = false`).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);

/// Which brain generates `weft voice talk`'s spoken replies.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum BrainMode {
    /// Daemon brain when a kernel daemon is reachable and serving
    /// `conversation.graph`; local Hermes otherwise.
    #[default]
    Auto,
    /// Force the daemon's own §W2.1 voice-loop reply. Errors out at startup
    /// if no daemon is reachable.
    Daemon,
    /// Force the local `LocalProviderVoiceLlm` (bare Hermes prompt) brain —
    /// the pre-WEFT-614 behavior.
    Local,
}

/// [`VoiceLlm`] that never calls an LLM — it waits for the daemon's own
/// voice-loop reply to land on `conversation.graph` and returns its text.
pub struct DaemonBrainVoiceLlm {
    client: tokio::sync::Mutex<DaemonClient>,
    conv_id: String,
    poll_interval: Duration,
    timeout: Duration,
}

impl DaemonBrainVoiceLlm {
    fn new(client: DaemonClient, conv_id: String) -> Self {
        Self {
            client: tokio::sync::Mutex::new(client),
            conv_id,
            poll_interval: DEFAULT_POLL_INTERVAL,
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

#[async_trait]
impl VoiceLlm for DaemonBrainVoiceLlm {
    async fn complete(&self, _req: &VoiceTurnRequest) -> Result<String, VoiceError> {
        let mut client = self.client.lock().await;

        let baseline = {
            let graph = fetch_graph(&mut client, &self.conv_id, None).await?;
            max_committed_assistant_seq(nodes_of(&graph))
        };

        let deadline = Instant::now() + self.timeout;
        loop {
            tokio::time::sleep(self.poll_interval).await;
            let graph = fetch_graph(&mut client, &self.conv_id, Some(baseline + 1)).await?;
            if let Some((_, text)) = select_new_committed_reply(nodes_of(&graph), baseline) {
                return Ok(text);
            }
            if Instant::now() >= deadline {
                return Err(VoiceError::Transport(format!(
                    "daemon brain: no reply on conv `{}` after {:?} — the daemon may be \
                     unreachable, or `[kernel.agent].voice_loop` may be disabled",
                    self.conv_id, self.timeout
                )));
            }
        }
    }
}

/// Probe for a daemon-brain adapter: connect to the kernel daemon and
/// confirm it actually serves `conversation.graph` for `conv_id` (a daemon
/// built without the `ecc` feature, or simply unreachable, means
/// daemon-brain mode can't work). Does NOT confirm `[kernel.agent].voice_loop`
/// is enabled — see [`resolve_brain`]'s printed caveat.
async fn connect_daemon_brain(conv_id: &str) -> Option<DaemonBrainVoiceLlm> {
    let mut client = crate::commands::daemon_conn::connect_opt().await?;
    let probe = Request::with_params(
        "conversation.graph",
        serde_json::json!({ "conv_id": conv_id }),
    );
    client.call(probe).await.ok()?.into_result().ok()?;
    Some(DaemonBrainVoiceLlm::new(client, conv_id.to_string()))
}

/// Resolve which brain speaks the reply for a `weft voice talk` session per
/// `--brain`, and whether the turn recorder should also mirror the
/// assistant's reply via `agent.turn.record`.
///
/// Returns the `VoiceLlm` override for
/// [`clawft_voice_talk::live::run_live_observed_with_llm`] (`None` keeps the
/// native stack's default local-Hermes binding), and the mirror flag
/// (`false` in daemon-brain mode — the daemon's own voice loop already
/// recorded the reply through its sink; mirroring it again would
/// double-anchor the same turn).
pub async fn resolve_brain(
    mode: BrainMode,
    conv_id: &str,
) -> anyhow::Result<(Option<Arc<dyn VoiceLlm>>, bool)> {
    match mode {
        BrainMode::Local => Ok((None, true)),
        BrainMode::Daemon => {
            let adapter = connect_daemon_brain(conv_id).await.ok_or_else(|| {
                anyhow::anyhow!(
                    "--brain daemon requires a reachable kernel daemon serving \
                     conversation.graph — start one with `weaver kernel start`"
                )
            })?;
            println!(
                "Brain: DAEMON (conv `{conv_id}` — speaking the kernel's own voice-loop \
                 reply). If `[kernel.agent].voice_loop` is disabled on the daemon, every \
                 turn will stall ~{}s before failing — that is NOT auto-detected.",
                DEFAULT_TIMEOUT.as_secs()
            );
            Ok((Some(Arc::new(adapter) as Arc<dyn VoiceLlm>), false))
        }
        BrainMode::Auto => match connect_daemon_brain(conv_id).await {
            Some(adapter) => {
                println!(
                    "Brain: DAEMON (auto — reachable daemon serving conv `{conv_id}`). If \
                     `[kernel.agent].voice_loop` is disabled on the daemon, every turn will \
                     stall ~{}s before failing — falling back to local is NOT automatic; \
                     restart with `--brain local` if replies stall.",
                    DEFAULT_TIMEOUT.as_secs()
                );
                Ok((Some(Arc::new(adapter) as Arc<dyn VoiceLlm>), false))
            }
            None => {
                println!("Brain: LOCAL (auto — no kernel daemon reachable)");
                Ok((None, true))
            }
        },
    }
}

/// One `conversation.graph` round-trip, reconnecting once on transport error
/// (same one-retry contract as the `agent.turn.record` poster and `weft
/// voice watch`).
async fn fetch_graph(
    client: &mut DaemonClient,
    conv_id: &str,
    since: Option<u64>,
) -> Result<Value, VoiceError> {
    let request = Request::with_params("conversation.graph", graph_params(conv_id, since));
    match client.call(request).await {
        Ok(resp) => resp
            .into_result()
            .map_err(|e| VoiceError::Transport(format!("conversation.graph rejected: {e}"))),
        Err(e) => match crate::commands::daemon_conn::connect_retry().await {
            Some(reconnected) => {
                *client = reconnected;
                let request =
                    Request::with_params("conversation.graph", graph_params(conv_id, since));
                client
                    .call(request)
                    .await
                    .map_err(|e2| VoiceError::Transport(format!("{e} (reconnect: {e2})")))?
                    .into_result()
                    .map_err(|e| VoiceError::Transport(format!("conversation.graph rejected: {e}")))
            }
            None => Err(VoiceError::Transport(format!(
                "daemon brain: transport error and reconnect failed: {e}"
            ))),
        },
    }
}

fn graph_params(conv_id: &str, since: Option<u64>) -> Value {
    match since {
        Some(s) => serde_json::json!({ "conv_id": conv_id, "since": s }),
        None => serde_json::json!({ "conv_id": conv_id }),
    }
}

fn nodes_of(graph: &Value) -> &[Value] {
    graph
        .get("nodes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// A node is a candidate committed reply iff it's a non-attempt, committed,
/// assistant turn (mirrors the `is_attempt` / `role` / `state` conventions
/// `voice_watch/render.rs` already renders from).
fn is_committed_reply(node: &Value) -> bool {
    node.get("role").and_then(Value::as_str) == Some("assistant")
        && node.get("state").and_then(Value::as_str) == Some("committed")
        && !node
            .get("attempt")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

/// The highest `chain_seq` among committed, non-attempt assistant nodes —
/// the baseline a fresh reply must exceed. `0` when there are none (real
/// chain seqs are 1-based).
fn max_committed_assistant_seq(nodes: &[Value]) -> u64 {
    nodes
        .iter()
        .filter(|n| is_committed_reply(n))
        .filter_map(|n| n.get("chain_seq").and_then(Value::as_u64))
        .max()
        .unwrap_or(0)
}

/// The lowest-`chain_seq` committed, non-attempt assistant node strictly
/// newer than `baseline_seq` with non-empty text — the daemon's next reply
/// after the turn this adapter is waiting on.
fn select_new_committed_reply(nodes: &[Value], baseline_seq: u64) -> Option<(u64, String)> {
    nodes
        .iter()
        .filter(|n| is_committed_reply(n))
        .filter_map(|n| {
            let seq = n.get("chain_seq").and_then(Value::as_u64)?;
            let text = n.get("text").and_then(Value::as_str)?;
            (seq > baseline_seq && !text.is_empty()).then(|| (seq, text.to_string()))
        })
        .min_by_key(|(seq, _)| *seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn committed_assistant(seq: u64, text: &str) -> Value {
        json!({ "role": "assistant", "state": "committed", "chain_seq": seq, "text": text })
    }

    #[test]
    fn max_seq_is_zero_when_no_committed_assistant_nodes() {
        let nodes = vec![json!({ "role": "user", "state": "committed", "chain_seq": 5 })];
        assert_eq!(max_committed_assistant_seq(&nodes), 0);
    }

    #[test]
    fn max_seq_ignores_attempts_and_user_and_tool_nodes() {
        let nodes = vec![
            committed_assistant(3, "hi"),
            json!({ "role": "assistant", "state": "frontier", "attempt": true, "chain_seq": 9 }),
            json!({ "role": "user", "state": "committed", "chain_seq": 7, "text": "hey" }),
            json!({ "role": "tool", "state": "committed", "chain_seq": 8, "text": "result" }),
        ];
        assert_eq!(max_committed_assistant_seq(&nodes), 3);
    }

    #[test]
    fn select_finds_new_committed_assistant_reply() {
        let nodes = vec![committed_assistant(5, "the answer")];
        let found = select_new_committed_reply(&nodes, 3);
        assert_eq!(found, Some((5, "the answer".to_string())));
    }

    #[test]
    fn select_ignores_attempt_nodes() {
        let nodes = vec![
            json!({
                "role": "assistant", "state": "frontier", "attempt": true,
                "chain_seq": 6, "text": "in-flight draft"
            }),
        ];
        assert_eq!(select_new_committed_reply(&nodes, 3), None);
    }

    #[test]
    fn select_ignores_user_and_tool_nodes() {
        let nodes = vec![
            json!({ "role": "user", "state": "committed", "chain_seq": 6, "text": "hey" }),
            json!({ "role": "tool", "state": "committed", "chain_seq": 7, "text": "42" }),
        ];
        assert_eq!(select_new_committed_reply(&nodes, 3), None);
    }

    #[test]
    fn select_ignores_seqs_at_or_below_baseline() {
        let nodes = vec![committed_assistant(3, "old"), committed_assistant(2, "older")];
        assert_eq!(select_new_committed_reply(&nodes, 3), None);
    }

    #[test]
    fn select_ignores_empty_text() {
        let nodes = vec![committed_assistant(5, "")];
        assert_eq!(select_new_committed_reply(&nodes, 3), None);
    }

    #[test]
    fn select_picks_lowest_new_seq_when_several() {
        let nodes = vec![
            committed_assistant(9, "third"),
            committed_assistant(5, "first new"),
            committed_assistant(7, "second"),
        ];
        assert_eq!(
            select_new_committed_reply(&nodes, 3),
            Some((5, "first new".to_string()))
        );
    }

    #[test]
    fn nodes_of_defaults_to_empty_slice_when_missing() {
        assert!(nodes_of(&json!({})).is_empty());
        assert!(nodes_of(&json!({ "nodes": [] })).is_empty());
    }
}
