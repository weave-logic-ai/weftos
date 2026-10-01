//! `weft voice watch` — the Wave 1 §W1.4 live process surface (terminal-native).
//!
//! Polls `conversation.graph` at a ~1 Hz cadence and renders each committed turn
//! node as it lands: role + text, its 4-axis classification (intent / topic /
//! emotion tier badge), and — when the turn carries a `voice_analysis` record —
//! the per-utterance decomposition rows: speaker + cosine score, the endpoint
//! fire (prob + source), a prosody summary, a moving **arousal bar** with the
//! emotion label + `source` badge, per-token confidence, and paralinguistics.
//! `--json` dumps each new node's full record (one line per node) for
//! inspection / piping.
//!
//! This is the committed-state half of the §W1.4 surface — its source is the
//! `conversation.graph` poll (ADR-067 D2), which serves `voice_analysis`
//! verbatim (Wave 1 §W1.2 wire). Interleaved with it is the **decision
//! trace** (`voice.trace` poll, `render::render_trace_events`): the voice
//! loop's gate verdicts (non-lexical / unclear, with the metrics that
//! decided), router actions with their input signals, and each brain call's
//! accounting — model, wall time, tool calls, token counts, and captured
//! reasoning — plus client-posted process events (`voice.trace.append` from
//! the talk client: cue tones, TTS render time, barge-ins). The graph shows
//! WHAT the conversation said; the trace shows HOW the agent got there.
//! The **live per-frame half** ([`LiveRenderObserver`])
//! layers on top: attached to the observe-only `weft voice listen` loop, it
//! renders the partial transcript, the endpoint fire the instant it happens, and
//! the finalized turn's full decomposition as events arrive — no graph round-trip.
//! (A live capture level-meter awaits a per-frame `CaptureLevel` event; until
//! then the finalized-line + arousal bar is the fallback, plan riskiest-call #4.)
//!
//! Wave 2 §W2.6 extends the committed-state half with the register-early/
//! commit-late reply lifecycle: an in-flight reply is an ATTEMPT node
//! (`role=assistant`, `state=frontier`, `attempt: true`, carrying the
//! submitted `goal`); it renders once as "agent busy", then transitions
//! frontier→pruned on interrupt or frontier→committed on finalize. A Refine
//! draws a `Contradicts` edge from the new attempt to the superseded one.
//! `render::render_graph_updates` tracks each node's last-rendered state
//! across polls so a transition — not just a brand-new node — gets its own
//! line. The rendering internals live in [`render`], a sibling module split
//! out once this surface grew a second node lifecycle to track.

mod render;

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use clawft_rpc::{DaemonClient, Request};
use clawft_voice_talk::{ConversationEvent, ConversationObserver};

use render::{fmt_ms, render_graph_updates, render_trace_events, render_voice_analysis, WatchState};

/// Poll `conversation.graph` for `conv_id` and stream each newly-committed turn
/// node to the terminal until Ctrl+C. `json` switches to one-record-per-line
/// JSON; `interval_ms` is the committed-state poll cadence.
pub async fn handle_watch(conv_id: String, json: bool, interval_ms: u64) -> anyhow::Result<()> {
    let mut client = crate::commands::daemon_conn::connect_opt().await.ok_or_else(|| {
        anyhow::anyhow!(
            "no kernel daemon reachable — start one with `weaver kernel start` \
             (and enable [kernel.agent.classification] mode = \"keyword\")"
        )
    })?;
    // Warn once if this binary and the daemon were built from different trees.
    super::daemon_guard::warn_on_build_mismatch(&mut client).await;

    if !json {
        println!(
            "=== weft voice watch — conversation `{conv_id}` \
             (poll {interval_ms}ms · Ctrl+C to exit) ===\n"
        );
        println!("Watching committed turns + the agent's decision trace: gates (and why),");
        println!("router verdicts, brain calls (model · wall time · tools · reasoning),");
        println!("TTS render timing and cue tones. Speak via `weft voice talk`.\n");
    }

    // Nodes already rendered + their last-known state, so a re-poll only
    // prints turns that newly landed or transitioned (§W2.6).
    let mut watch = WatchState::default();
    // Incremental cursor into the daemon's voice trace (the decision/process
    // feed) — `None` until the first read returns a cursor. A daemon built
    // before `voice.trace` degrades gracefully: warn once, keep the graph.
    let mut trace_cursor: Option<u64> = None;
    let mut trace_supported = true;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                if !json {
                    println!("\nwatch ended.");
                }
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_millis(interval_ms)) => {
                // The decision/process trace first (route → gate → brain
                // lines), so a turn's "how" prints just before the graph's
                // committed "what" for the same poll window.
                if trace_supported {
                    let mut trace_params = serde_json::json!({ "conv_id": conv_id });
                    if let Some(cursor) = trace_cursor {
                        trace_params["since"] = serde_json::json!(cursor);
                    }
                    let req = Request::with_params("voice.trace", trace_params);
                    match client.call(req).await {
                        Ok(resp) => match resp.into_result() {
                            Ok(val) => {
                                if let Some(next) = val.get("next").and_then(|v| v.as_u64()) {
                                    trace_cursor = Some(next);
                                }
                                render_trace_events(&val, json);
                            }
                            Err(e) => {
                                eprintln!(
                                    "voice.trace unavailable ({e}) — daemon predates the \
                                     decision trace; showing committed turns only"
                                );
                                trace_supported = false;
                            }
                        },
                        Err(e) => {
                            eprintln!("daemon transport error: {e}; reconnecting…");
                            match crate::commands::daemon_conn::connect_retry().await {
                                Some(c) => { client = c; continue; }
                                None => {
                                    eprintln!("kernel daemon gone — watch stopped.");
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
                let req = Request::with_params(
                    "conversation.graph",
                    serde_json::json!({ "conv_id": conv_id }),
                );
                match client.call(req).await {
                    Ok(resp) => match resp.into_result() {
                        Ok(val) => render_graph_updates(&val, &mut watch, json),
                        Err(e) => eprintln!("conversation.graph error: {e}"),
                    },
                    Err(e) => {
                        // One reconnect attempt; a stopped daemon ends the watch.
                        eprintln!("daemon transport error: {e}; reconnecting…");
                        match crate::commands::daemon_conn::connect_retry().await {
                            Some(c) => client = c,
                            None => {
                                eprintln!("kernel daemon gone — watch stopped.");
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── Live per-frame surface (§W1.4) ─────────────────────────────────────────
//
// The observe-side counterpart to the graph poll above: a `ConversationObserver`
// attached to the `weft voice listen` loop that renders the process live as
// events fire, reusing the same decomposition rows the committed-state view uses.

/// Renders the live process stream to the terminal as [`ConversationEvent`]s
/// arrive: the running partial ("listening…"), the endpoint fire the instant it
/// decides (probability, source, trailing silence), the finalized turn with its
/// full voice decomposition (speaker, prosody, arousal bar, paralinguistics), and
/// speaker enrollment plus any reply. Non-blocking — plain terminal writes.
pub struct LiveRenderObserver;

impl ConversationObserver for LiveRenderObserver {
    fn observe(&self, event: ConversationEvent) {
        match event {
            ConversationEvent::PartialTranscript { text } => {
                let line = if text.trim().is_empty() {
                    "  … listening".to_string()
                } else {
                    format!("  … {text}")
                };
                // Overwrite in place (no newline) so the partial updates live.
                print!("\r{line}\x1b[K");
                let _ = std::io::stdout().flush();
            }
            ConversationEvent::EndpointFired {
                completion_prob,
                source,
                silence_ms,
            } => {
                println!("\r  ⏎ endpoint p={completion_prob:.2} ({source}) @ {silence_ms}ms silence\x1b[K");
            }
            ConversationEvent::UserTurn {
                text,
                speaker_name,
                voice_analysis,
                ..
            } => {
                let who = speaker_name.as_deref().unwrap_or("speaker");
                println!("┌─ {who}: \u{201c}{text}\u{201d}");
                // Reuse the committed-state decomposition rows by serializing the
                // record to the same flat JSON the graph view renders from.
                if let Some(va) = voice_analysis.as_deref()
                    && let Ok(v) = serde_json::to_value(va)
                {
                    render_voice_analysis(&v);
                }
                println!("└─\n");
            }
            ConversationEvent::SpeakerEnrolled { name, .. } => {
                println!("  ✦ enrolled new speaker: {name}");
            }
            ConversationEvent::CommittedReply { answer } => {
                println!("  ↳ reply: {answer}");
            }
            // Live level meter (§W1.4 row 1). Placeholder to keep the render
            // exhaustive — coder-B owns the visual (a moving dBFS-vs-floor bar,
            // overwrite-in-place like the partial). Fields are ready to render.
            ConversationEvent::CaptureLevel {
                rms_dbfs: _,
                floor_dbfs: _,
            } => {}
            // The "thinking" filler spoken when the answer is slow to land
            // (WEFT-658) — surfaced the same way the partial transcript is.
            ConversationEvent::SpeculativeFiller { text } => {
                println!("  … {text}");
            }
            // A light cue tone (replaced the spoken ack/filler): "ack" the
            // moment a turn engages the brain, "working" while the answer
            // is still generating.
            ConversationEvent::CueTone { kind } => {
                println!("  ♪ cue ({})", kind.label());
            }
            // Speech-synthesis timing for the grounded answer.
            ConversationEvent::TtsRendered { ms, chunks } => {
                println!("  ♬ tts rendered: {chunks} chunk(s) in {}", fmt_ms(ms));
            }
            // A turn was gated before the reply slot ran (WEFT-659) — no ack,
            // no filler, no brain reply. Surfaced so the watch view shows why.
            ConversationEvent::TurnGated { reason } => {
                println!("  ⊘ turn gated ({reason})");
            }
            // Listen-only never fires these; a full talk loop supersedes the ack
            // with the committed reply and prunes on barge-in.
            ConversationEvent::SpeculativeReply { .. } | ConversationEvent::Interrupted => {}
        }
    }
}

/// Fan one event out to several observers. `run_live_observed` accepts a single
/// `extra_observer`, so the listen surface combines the daemon turn-recorder and
/// the [`LiveRenderObserver`] through this. Events are `Clone`, so each observer
/// gets its own copy.
pub struct FanOutObserver(pub Vec<Arc<dyn ConversationObserver>>);

impl ConversationObserver for FanOutObserver {
    fn observe(&self, event: ConversationEvent) {
        for observer in &self.0 {
            observer.observe(event.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fan_out_delivers_to_every_observer() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Counter(Arc<AtomicUsize>);
        impl ConversationObserver for Counter {
            fn observe(&self, _event: ConversationEvent) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let a = Arc::new(AtomicUsize::new(0));
        let b = Arc::new(AtomicUsize::new(0));
        let fan = FanOutObserver(vec![
            Arc::new(Counter(a.clone())),
            Arc::new(Counter(b.clone())),
        ]);
        fan.observe(ConversationEvent::EndpointFired {
            completion_prob: 0.9,
            source: "smart-turn-v3".into(),
            silence_ms: 300,
        });
        assert_eq!(a.load(Ordering::SeqCst), 1, "first observer received the event");
        assert_eq!(b.load(Ordering::SeqCst), 1, "second observer received the event");
    }
}
