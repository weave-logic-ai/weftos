//! Daemon-first routing for `weft agent` (M1 / ADR-061, WEFT-606).
//!
//! When a kernel daemon is reachable, `weft agent` routes every turn
//! through the daemon's long-lived `AgentService` via the `agent.chat`
//! RPC, making the daemon-hosted loop THE single conversation engine
//! that text uses today and voice will share later. When no daemon is
//! running, [`crate::commands::agent`] falls back to the in-process
//! `AgentLoop` with no behaviour change.
//!
//! The daemon owns conversation continuity: the service reads only the
//! latest `user` message from the request and loads prior history
//! server-side keyed by `conv_id` (see
//! `clawft_service_agent::AgentService::dispatch`). So the CLI sends
//! just the current turn under a single stable [`CLI_CONV_ID`] and the
//! daemon threads the history across turns and restarts. One-shot `-m`
//! turns use their own conversation instead (fresh per call, or the
//! `--session` name), so they never inherit the REPL's history.

use std::time::Duration;

use clawft_core::agent::skills_v2::SkillRegistry;
use clawft_rpc::{DaemonClient, Request};
use clawft_types::agent_chat::{AgentChatMessage, AgentChatParams, AgentChatResult};
use tokio::io::AsyncBufReadExt;
use tracing::info;

use crate::interactive::builtins::{QUIT_SENTINEL, register_builtins, register_skill_commands};
use crate::interactive::registry::{InteractiveContext, SlashCommandRegistry};

/// Stable conversation id for CLI agent turns.
///
/// The daemon keys conversation continuity on this id, so a single
/// stable value maps every CLI turn — across turns and restarts — to
/// one canonical daemon session. This deliberately mirrors the legacy
/// in-process `cli-session` chat_id (namespaced with a `cli:` prefix so
/// the two histories are distinguishable on the substrate). When both a
/// daemon and the in-process fallback have run, the daemon session is
/// the canonical one; the in-process `cli-session` history only applies
/// when no daemon was reachable.
pub const CLI_CONV_ID: &str = "cli:cli-session";

/// Daemon conversation id for a one-shot chat id (`cli:` namespace, like
/// [`CLI_CONV_ID`]).
pub fn one_shot_conv_id(chat_id: &str) -> String {
    format!("cli:{chat_id}")
}

/// Short probe timeout for daemon detection.
///
/// A local Unix-socket connect is effectively instant when the daemon
/// is up; the timeout only bounds the pathological case (stale socket,
/// filesystem stall) so `weft agent` never hangs on detection.
const PROBE_TIMEOUT: Duration = Duration::from_millis(750);

/// Try to connect to a running kernel daemon within a short timeout.
///
/// Returns a connected [`DaemonClient`] when the socket accepts a
/// connection, or `None` when no daemon is reachable (the caller then
/// falls back to the in-process loop).
pub async fn detect() -> Option<DaemonClient> {
    // A probe timeout (Err) collapses to `None` — same as "no daemon".
    let mut client = tokio::time::timeout(PROBE_TIMEOUT, crate::commands::daemon_conn::connect_opt())
        .await
        .unwrap_or_default()?;
    // Surface a stale-binary/daemon pairing before the session starts
    // (once per process, non-fatal).
    crate::commands::daemon_guard::warn_on_build_mismatch(&mut client).await;
    Some(client)
}

/// Send one user turn through the daemon's `agent.chat` RPC.
///
/// Returns the assistant text on success. A transport failure or an
/// error response from the daemon (`ok: false`, e.g. the agent service
/// failed to wire at boot) is returned as `Err` so callers map it to
/// the right exit code (one-shot) or REPL notice (interactive).
async fn daemon_turn(
    client: &mut DaemonClient,
    conv_id: &str,
    user_input: &str,
) -> anyhow::Result<AgentChatResult> {
    let params = AgentChatParams {
        messages: vec![AgentChatMessage {
            role: "user".into(),
            content: user_input.to_owned(),
                    audio: None,
        }],
        temperature: None,
        max_tokens: None,
        conv_id: conv_id.to_owned(),
        metadata: None,
        caller_id: None,
    };
    let payload = serde_json::to_value(&params)
        .map_err(|e| anyhow::anyhow!("failed to encode agent.chat params: {e}"))?;
    let response = client.call(Request::with_params("agent.chat", payload)).await?;
    // `into_result` bails with the daemon's error string on `ok: false`.
    let value = response.into_result()?;
    let result: AgentChatResult = serde_json::from_value(value)
        .map_err(|e| anyhow::anyhow!("failed to decode agent.chat result: {e}"))?;
    Ok(result)
}

/// One-shot (`-m`) turn routed through the daemon.
///
/// Prints the assistant reply to stdout on success. On failure the
/// error goes to stderr and the function bails so the process exits
/// non-zero — matching the in-process one-shot's fix-C contract where
/// an error reply is an exit-1 condition, not a normal print.
/// `chat_id` is the one-shot conversation (see
/// `agent::one_shot_chat_id`): fresh per call unless `--session` named one,
/// so a daemon-routed `-m` never continues the REPL's [`CLI_CONV_ID`].
pub async fn run_single_message(
    mut client: DaemonClient,
    message: &str,
    chat_id: &str,
) -> anyhow::Result<()> {
    // Engine line to stderr so `-m` stdout stays clean for scripting.
    eprintln!("Engine: daemon (shared)");
    match daemon_turn(&mut client, &one_shot_conv_id(chat_id), message).await {
        // A turn the loop flagged as failed (finish_reason "error") is an
        // exit-1 condition with the text on stderr, same as in-process.
        Ok(result) if result.is_error() => {
            eprintln!("{}", result.assistant_text);
            anyhow::bail!("{}", result.assistant_text)
        }
        Ok(result) => {
            println!("{}", result.assistant_text);
            Ok(())
        }
        Err(e) => {
            eprintln!("{e}");
            anyhow::bail!("{e}")
        }
    }
}

/// Interactive REPL routed through the daemon.
///
/// Slash commands (`/help`, `/skills`, `/use`, `/tools`, `/status`,
/// `/quit`, and skill-contributed commands) are dispatched locally
/// exactly as in the in-process REPL. Every non-slash line is sent to
/// the daemon as one `agent.chat` turn under [`CLI_CONV_ID`].
///
/// `/use` still selects a skill locally, but the `agent.chat` wire
/// format has no per-turn metadata slot, so skill instructions cannot
/// be threaded to daemon turns yet; the loop emits a one-time notice
/// when a skill is active and sends the turn without skill context.
pub async fn run_interactive(
    mut client: DaemonClient,
    skill_registry: &SkillRegistry,
) -> anyhow::Result<()> {
    println!("weft agent -- interactive mode (type /help for commands)");
    println!("Engine: daemon (shared)");
    println!("Model: daemon-managed");

    // Local slash-command registry (identical to the in-process REPL).
    let mut cmd_registry = SlashCommandRegistry::new();
    register_builtins(&mut cmd_registry);

    let skill_entries: Vec<(String, String)> = skill_registry
        .list()
        .iter()
        .filter(|s| s.user_invocable)
        .map(|s| (s.name.clone(), s.description.clone()))
        .collect();
    let skill_count = register_skill_commands(&mut cmd_registry, &skill_entries);
    if skill_count > 0 {
        println!("Skills: {skill_count} registered");
    }
    // WEFT-74 / ADR-080: non-blocking notice so pending autogen skills do
    // not accumulate unseen. Review remains CLI (`weft skills pending|…`).
    if let Some(notice) = clawft_core::agent::skill_autogen::pending_review_start_notice(
        None,
        clawft_core::agent::skill_autogen::PendingReviewTiming::DEFAULT_POLICY,
    ) {
        println!("{notice}");
    }
    println!();

    let mut ctx = InteractiveContext::new("daemon-managed".to_string());
    ctx.skill_names = skill_registry.names().iter().map(|s| s.to_string()).collect();

    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin).lines();
    let mut warned_skill = false;

    loop {
        eprint!("> ");
        use std::io::Write;
        std::io::stderr().flush().ok();

        let line = match reader.next_line().await? {
            Some(l) => l,
            None => break, // EOF
        };
        let input = line.trim();
        if input.is_empty() {
            continue;
        }

        // Local slash commands (skill activation, help, status, quit …).
        if input.starts_with('/')
            && let Some(result) = cmd_registry.dispatch(input, &mut ctx)
        {
            match result {
                Ok(output) => {
                    if output == QUIT_SENTINEL {
                        break;
                    }
                    println!("{output}");
                    println!();
                }
                Err(e) => eprintln!("error: {e}"),
            }
            continue;
            // Unknown slash command falls through and is sent to the daemon.
        }

        // Skills cannot ride on `agent.chat` yet (no per-turn metadata);
        // notify once per session and send the raw turn.
        if !ctx.active_skill.is_empty() && !warned_skill {
            eprintln!(
                "note: skill '{}' is selected, but daemon turns do not carry \
                 skill instructions yet — sending without skill context.",
                ctx.active_skill
            );
            warned_skill = true;
        }

        match daemon_turn(&mut client, CLI_CONV_ID, input).await {
            // Interactive chat keeps showing a failed turn's text as a reply.
            Ok(result) => {
                println!("{}", result.assistant_text);
                println!();
            }
            Err(e) => {
                eprintln!("error: {e}");
                // One reconnect attempt: a dropped socket costs this turn
                // but keeps the session alive. If the daemon is truly gone
                // (connect returns None) we end the session rather than
                // spin printing errors.
                match crate::commands::daemon_conn::connect_retry().await {
                    Some(fresh) => {
                        client = fresh;
                        eprintln!("(reconnected to daemon)");
                    }
                    None => {
                        eprintln!("daemon unreachable; ending interactive session.");
                        break;
                    }
                }
            }
        }
    }

    info!("daemon interactive session ended");
    println!("Goodbye.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shot_conv_ids_are_namespaced_and_distinct_from_the_repl() {
        let a = one_shot_conv_id("oneshot-1");
        assert_eq!(a, "cli:oneshot-1");
        assert_ne!(a, CLI_CONV_ID);
        assert_eq!(one_shot_conv_id("review"), one_shot_conv_id("review"));
    }

    #[test]
    fn cli_conv_id_is_stable_and_namespaced() {
        // The daemon keys conversation continuity on this constant, so it
        // must be a single stable value distinguishable from the legacy
        // in-process `cli-session` chat_id.
        assert_eq!(CLI_CONV_ID, "cli:cli-session");
        assert!(CLI_CONV_ID.starts_with("cli:"));
    }

    #[test]
    fn agent_chat_params_encode_carries_conv_id_and_user_turn() {
        // daemon_turn builds exactly this payload; assert the wire shape
        // the daemon's `agent.chat` handler deserializes.
        let params = AgentChatParams {
            messages: vec![AgentChatMessage {
                role: "user".into(),
                content: "hello".into(),
                        audio: None,
        }],
            temperature: None,
            max_tokens: None,
            conv_id: CLI_CONV_ID.to_owned(),
            metadata: None,
            caller_id: None,
        };
        let v = serde_json::to_value(&params).unwrap();
        assert_eq!(v["conv_id"], "cli:cli-session");
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"], "hello");
    }
}
