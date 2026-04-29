//! WEFT-248: chat-panel live smoke against a running daemon +
//! llama-server.
//!
//! Gated behind `feature = "live-smoke"` so the default CI run
//! stays hermetic. To run manually:
//!
//! ```bash
//! # 1. Start the daemon and llama-server out-of-band (these are
//! #    operator-managed; the test does NOT spin them up).
//! # 2. Run:
//! cargo test -p clawft-gui-egui --features live-smoke \
//!     --test chat_panel_live_smoke -- --ignored --nocapture
//! ```
//!
//! What this asserts:
//!
//! 1. The daemon socket resolves at the conventional location
//!    (`$WEFTOS_DAEMON_SOCKET` or `~/.weftos/daemon.sock`).
//! 2. `agent.chat` (the chat panel's wire) replies within 60s with
//!    a non-empty response containing the substring "project" or
//!    "WeftOS" or any of the project-name tokens — i.e. the LLM
//!    actually grounded a reply about the workspace.
//!
//! The test is `#[ignore]`-marked so even with the feature on, it
//! only runs under explicit `--ignored`. That's the additional
//! guard against accidental runs in CI environments that do build
//! with `--all-features`.

#![cfg(all(feature = "live-smoke", not(target_arch = "wasm32")))]

use std::time::{Duration, Instant};

#[test]
#[ignore = "live-smoke: requires running daemon + llama-server (set WEFTOS_DAEMON_SOCKET)"]
fn chat_panel_replies_about_workspace() {
    // Resolve the daemon socket. The default mirror's the desktop
    // shell's resolution path — see the vscode panel's
    // `resolveSocketPath`.
    let socket = std::env::var("WEFTOS_DAEMON_SOCKET").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/.weftos/daemon.sock")
    });

    // Bail out early if the socket isn't there — the test message
    // should make the failure mode obvious to the operator.
    assert!(
        std::path::Path::new(&socket).exists(),
        "expected daemon socket at {socket}; start the daemon first \
         or set WEFTOS_DAEMON_SOCKET"
    );

    // Build a Live transport and fire `agent.chat` with the canonical
    // smoke prompt. The wasm bridge in production drives the same
    // `Command::Raw` shape via `__weftPostToHost`; here we go direct
    // through native_live since this test runs on native.
    let live = clawft_gui_egui::live::Live::spawn();
    let (tx, mut rx) = clawft_gui_egui::live::reply_channel();
    live.submit(clawft_gui_egui::live::Command::Raw {
        method: "agent.chat".into(),
        params: serde_json::json!({
            "message": "what is this project",
        }),
        reply: Some(tx),
    });

    // Poll for up to 60s.
    let deadline = Instant::now() + Duration::from_secs(60);
    let reply = loop {
        match clawft_gui_egui::live::try_recv_reply(&mut rx) {
            clawft_gui_egui::live::TryReply::Done(Ok(value)) => break value,
            clawft_gui_egui::live::TryReply::Done(Err(err)) => {
                panic!("agent.chat returned error: {err}");
            }
            clawft_gui_egui::live::TryReply::Closed => {
                panic!("agent.chat reply channel closed without a value");
            }
            clawft_gui_egui::live::TryReply::Empty => {
                if Instant::now() > deadline {
                    panic!("agent.chat did not reply within 60s");
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    };

    // The shape is implementation-defined but conventionally
    // `{ reply: "..." }` or `{ message: { content: "..." } }`. Try
    // both and fall back to the JSON dump.
    let body = reply
        .get("reply")
        .or_else(|| reply.pointer("/message/content"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| reply.to_string());
    let lower = body.to_lowercase();

    // The smoke prompt is "what is this project" — at least one of
    // these tokens should appear in any sensible answer. We're
    // checking that the LLM actually saw the workspace; an empty
    // reply (or a generic "I don't know") would be a regression.
    let expected_tokens =
        ["weftos", "project", "clawft", "agent", "rust"];
    let hit = expected_tokens.iter().any(|t| lower.contains(t));
    assert!(
        hit,
        "agent.chat reply did not mention any expected token \
         (looked for {expected_tokens:?}). Reply: {body:?}"
    );
    assert!(!body.trim().is_empty(), "agent.chat reply was empty");
}
