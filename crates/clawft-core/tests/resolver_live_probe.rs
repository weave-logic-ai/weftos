//! Diagnostic probe — not a real test. Loads the live config from
//! `/home/aepod/dev/clawft` using the production loader, constructs
//! the same `PermissionResolver` the daemon would, and prints what
//! `("panel", "agent.chat")` resolves to. Run with:
//!
//!   cargo test -p clawft-core --test resolver_live_probe -- --ignored --nocapture
//!
//! Marked `#[ignore]` so it doesn't fire on normal `cargo test` runs.

use clawft_core::pipeline::permissions::PermissionResolver;
use clawft_platform::config_loader::load_config_raw;

#[tokio::test]
#[ignore]
async fn probe_chat_resolution_against_live_config() {
    std::env::set_current_dir("/home/aepod/dev/clawft").unwrap();
    let fs = clawft_platform::fs::NativeFileSystem;
    let env = clawft_platform::env::NativeEnvironment;
    let raw = load_config_raw(&fs, &env).await.unwrap();
    let config: clawft_types::config::Config = serde_json::from_value(raw).unwrap();

    println!(
        "\n>>> typed routing.permissions.channels keys: {:?}",
        config.routing.permissions.channels.keys().collect::<Vec<_>>()
    );

    let resolver = PermissionResolver::new(&config.routing, None);
    let perms = resolver.resolve("panel", "agent.chat", false);

    println!("\n>>> resolver resolve('panel', 'agent.chat'):");
    println!("    level         = {}", perms.level);
    println!("    tool_access   = {:?}", perms.tool_access);
    println!("    tool_denylist = {:?}", perms.tool_denylist);

    let cli_perms = resolver.resolve("local", "cli", false);
    println!("\n>>> resolver resolve('local', 'cli'):");
    println!("    level         = {}", cli_perms.level);
    println!("    tool_access   = {:?}", cli_perms.tool_access);
}
