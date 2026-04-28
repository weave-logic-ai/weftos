//! Diagnostic probe — not a real test. Loads the live config from the
//! current working directory using the production loader and prints
//! the merged `routing.permissions.channels` block. Useful for
//! verifying that a workspace `.clawft/config.json` overlay is being
//! applied correctly. Run with:
//!
//!   cargo test -p clawft-platform --test overlay_probe -- --nocapture
//!
//! Marked `#[ignore]` so it doesn't fire on normal `cargo test` runs.

use clawft_platform::config_loader::load_config_raw;

#[tokio::test]
#[ignore]
async fn probe_merged_channels_block() {
    // Force cwd to the workspace root so the Layer 3 overlay path
    // (`./.clawft/config.json` resolved cwd-relative) sees the same
    // file the daemon would.
    std::env::set_current_dir("/home/aepod/dev/clawft").unwrap();
    let fs = clawft_platform::fs::NativeFileSystem;
    let env = clawft_platform::env::NativeEnvironment;
    let raw = load_config_raw(&fs, &env).await.unwrap();

    println!("\n>>> cwd: {}", std::env::current_dir().unwrap().display());

    let channels = raw.pointer("/routing/permissions/channels");
    println!("\n>>> merged routing.permissions.channels:");
    println!(
        "{}",
        serde_json::to_string_pretty(channels.unwrap_or(&serde_json::Value::Null)).unwrap()
    );

    let agent_chat = raw.pointer("/routing/permissions/channels/agent.chat");
    println!("\n>>> agent.chat resolved (raw json): {agent_chat:?}");

    // Now do the typed deserialization the daemon would do, then
    // inspect the typed Config struct to see if `agent.chat` survived.
    let typed: clawft_types::config::Config = serde_json::from_value(raw).unwrap();
    println!(
        "\n>>> typed routing.permissions.channels keys: {:?}",
        typed.routing.permissions.channels.keys().collect::<Vec<_>>()
    );
    println!(
        ">>> typed agent.chat: {:?}",
        typed.routing.permissions.channels.get("agent.chat")
    );
}
