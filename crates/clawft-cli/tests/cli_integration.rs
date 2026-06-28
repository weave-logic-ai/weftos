//! CLI integration tests for the `weft` binary.
//!
//! These tests run the actual compiled binary via `std::process::Command`
//! to verify end-to-end CLI behavior. Each test spawns a fresh process
//! with `CLAWFT_CONFIG` pointing at a nonexistent path so the config
//! loader falls back to defaults (empty JSON object).

use std::process::Command;

/// Build a `Command` pointing at the compiled `weft` binary.
///
/// Sets `CLAWFT_CONFIG` to a nonexistent path so the config loader
/// detects that the file does not exist and falls back to defaults.
/// This prevents tests from accidentally loading a real user config
/// from `~/.clawft/config.json` or `~/.nanobot/config.json`.
fn weft_bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_weft"));
    cmd.env("CLAWFT_CONFIG", "/tmp/.clawft-test-nonexistent-config.json");
    // Suppress tracing output so test assertions only match program output.
    cmd.env("RUST_LOG", "off");
    cmd
}

// ── 1. Version and help ─────────────────────────────────────────────────

#[test]
fn version_output() {
    let output = weft_bin()
        .arg("--version")
        .output()
        .expect("failed to run weft");

    assert!(output.status.success(), "exit code should be 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("weft") && stdout.contains(env!("CARGO_PKG_VERSION")),
        "version output should contain 'weft' and '{}', got: {stdout}",
        env!("CARGO_PKG_VERSION")
    );
}

#[test]
fn help_output() {
    let output = weft_bin()
        .arg("--help")
        .output()
        .expect("failed to run weft");

    assert!(output.status.success(), "exit code should be 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("clawft AI assistant CLI"),
        "help output should contain the CLI description, got: {stdout}"
    );
}

#[test]
fn agent_help_output() {
    let output = weft_bin()
        .args(["agent", "--help"])
        .output()
        .expect("failed to run weft");

    assert!(output.status.success(), "exit code should be 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("agent") || stdout.contains("Agent"),
        "agent help should mention 'agent', got: {stdout}"
    );
}

#[test]
fn unknown_subcommand_fails() {
    let output = weft_bin()
        .arg("this-subcommand-does-not-exist")
        .output()
        .expect("failed to run weft");

    assert!(
        !output.status.success(),
        "unknown subcommand should return non-zero exit code"
    );
}

// ── 2. Status command ───────────────────────────────────────────────────

#[test]
fn status_succeeds() {
    let output = weft_bin()
        .arg("status")
        .output()
        .expect("failed to run weft");

    assert!(output.status.success(), "weft status should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("weft status"),
        "status output should contain header, got: {stdout}"
    );
}

#[test]
fn status_detailed_succeeds() {
    let output = weft_bin()
        .args(["status", "--detailed"])
        .output()
        .expect("failed to run weft");

    assert!(
        output.status.success(),
        "weft status --detailed should exit 0"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Detailed mode prints extra sections like Gateway and Channels.
    assert!(
        stdout.contains("Agent defaults"),
        "detailed status should contain 'Agent defaults', got: {stdout}"
    );
}

#[test]
fn status_rejects_unknown_flag() {
    // The status subcommand does not accept --config; clap should reject it.
    let output = weft_bin()
        .args(["status", "--bogus-flag-that-does-not-exist"])
        .output()
        .expect("failed to run weft");

    assert!(
        !output.status.success(),
        "unknown flag should cause a non-zero exit code"
    );
}

// ── 3. Config command ───────────────────────────────────────────────────

#[test]
fn config_show_outputs_json() {
    let output = weft_bin()
        .args(["config", "show"])
        .output()
        .expect("failed to run weft");

    assert!(output.status.success(), "weft config show should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The output should be valid JSON (default config serialized).
    assert!(
        stdout.contains('{') && stdout.contains('}'),
        "config show should output JSON, got: {stdout}"
    );
}

#[test]
fn config_section_agents() {
    let output = weft_bin()
        .args(["config", "section", "agents"])
        .output()
        .expect("failed to run weft");

    assert!(
        output.status.success(),
        "weft config section agents should exit 0"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The agents section contains a "defaults" key with model, workspace, etc.
    assert!(
        stdout.contains("defaults") || stdout.contains("model"),
        "agents section should contain defaults or model, got: {stdout}"
    );
}

#[test]
fn config_section_nonexistent_handles_gracefully() {
    let output = weft_bin()
        .args(["config", "section", "nonexistent_section_xyz"])
        .output()
        .expect("failed to run weft");

    // The command itself exits 0 but prints an error to stderr.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown section"),
        "nonexistent section should produce error on stderr, got: {stderr}"
    );
}

// ── 4. Completions ──────────────────────────────────────────────────────

#[test]
fn completions_bash() {
    let output = weft_bin()
        .args(["completions", "bash"])
        .output()
        .expect("failed to run weft");

    assert!(
        output.status.success(),
        "weft completions bash should exit 0"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("bash") || stdout.contains("complete") || stdout.contains("COMPREPLY"),
        "bash completions should contain shell-specific keywords, got: {stdout}"
    );
}

#[test]
fn completions_zsh() {
    let output = weft_bin()
        .args(["completions", "zsh"])
        .output()
        .expect("failed to run weft");

    assert!(
        output.status.success(),
        "weft completions zsh should exit 0"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("compdef") || stdout.contains("_describe"),
        "zsh completions should contain zsh-specific keywords, got: {stdout}"
    );
}

#[test]
fn completions_fish() {
    let output = weft_bin()
        .args(["completions", "fish"])
        .output()
        .expect("failed to run weft");

    assert!(
        output.status.success(),
        "weft completions fish should exit 0"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("complete -c"),
        "fish completions should contain 'complete -c', got: {stdout}"
    );
}

#[test]
fn completions_invalid_shell() {
    let output = weft_bin()
        .args(["completions", "notashell"])
        .output()
        .expect("failed to run weft");

    assert!(
        !output.status.success(),
        "unsupported shell should return non-zero exit code"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unsupported shell"),
        "error should mention 'unsupported shell', got: {stderr}"
    );
}

// ── 5. Sessions and memory ──────────────────────────────────────────────

#[test]
fn sessions_list_succeeds() {
    let output = weft_bin()
        .args(["sessions", "list"])
        .output()
        .expect("failed to run weft");

    assert!(output.status.success(), "weft sessions list should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // With no config, it may show "No sessions found." or list an empty table.
    assert!(
        stdout.contains("session") || stdout.contains("Session") || stdout.contains("No sessions"),
        "sessions list should produce session-related output, got: {stdout}"
    );
}

#[test]
fn memory_show_succeeds() {
    let output = weft_bin()
        .args(["memory", "show"])
        .output()
        .expect("failed to run weft");

    assert!(output.status.success(), "weft memory show should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // With no real memory file, expect either the content or "no memory entries".
    assert!(
        stdout.contains("Memory file") || stdout.contains("no memory") || stdout.contains("memory"),
        "memory show should produce memory-related output, got: {stdout}"
    );
}

// ── 6. Channels status ─────────────────────────────────────────────────

#[test]
fn channels_status_succeeds() {
    let output = weft_bin()
        .args(["channels", "status"])
        .output()
        .expect("failed to run weft");

    assert!(
        output.status.success(),
        "weft channels status should exit 0"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("CHANNEL") || stdout.contains("telegram") || stdout.contains("slack"),
        "channels status should show channel information, got: {stdout}"
    );
}

// ── 7. Cron list ────────────────────────────────────────────────────────

#[test]
fn cron_list_succeeds() {
    let output = weft_bin()
        .args(["cron", "list"])
        .output()
        .expect("failed to run weft");

    assert!(output.status.success(), "weft cron list should exit 0");
}

// ── 8. Gateway help ─────────────────────────────────────────────────────

#[test]
fn gateway_help_output() {
    let output = weft_bin()
        .args(["gateway", "--help"])
        .output()
        .expect("failed to run weft");

    assert!(output.status.success(), "exit code should be 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("gateway") || stdout.contains("Gateway"),
        "gateway help should mention 'gateway', got: {stdout}"
    );
}

// ── 9. Subcommand --help for every top-level command ────────────────

#[test]
fn config_help_output() {
    let output = weft_bin()
        .args(["config", "--help"])
        .output()
        .expect("failed to run weft");

    assert!(output.status.success(), "weft config --help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("config") || stdout.contains("Config") || stdout.contains("configuration"),
        "config help should mention 'config', got: {stdout}"
    );
}

#[test]
fn memory_help_output() {
    let output = weft_bin()
        .args(["memory", "--help"])
        .output()
        .expect("failed to run weft");

    assert!(output.status.success(), "weft memory --help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("memory") || stdout.contains("Memory"),
        "memory help should mention 'memory', got: {stdout}"
    );
}

#[test]
fn sessions_help_output() {
    let output = weft_bin()
        .args(["sessions", "--help"])
        .output()
        .expect("failed to run weft");

    assert!(
        output.status.success(),
        "weft sessions --help should exit 0"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("sessions") || stdout.contains("Sessions") || stdout.contains("session"),
        "sessions help should mention 'sessions', got: {stdout}"
    );
}

#[test]
fn channels_help_output() {
    let output = weft_bin()
        .args(["channels", "--help"])
        .output()
        .expect("failed to run weft");

    assert!(
        output.status.success(),
        "weft channels --help should exit 0"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("channels") || stdout.contains("Channels") || stdout.contains("channel"),
        "channels help should mention 'channels', got: {stdout}"
    );
}

#[test]
fn cron_help_output() {
    let output = weft_bin()
        .args(["cron", "--help"])
        .output()
        .expect("failed to run weft");

    assert!(output.status.success(), "weft cron --help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("cron") || stdout.contains("Cron") || stdout.contains("scheduled"),
        "cron help should mention 'cron', got: {stdout}"
    );
}

// ── 10. Invalid flag handling ───────────────────────────────────────

#[test]
fn invalid_top_level_flag_fails() {
    let output = weft_bin()
        .arg("--nonexistent")
        .output()
        .expect("failed to run weft");

    assert!(
        !output.status.success(),
        "unknown top-level flag should return non-zero exit code"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unexpected argument") || stderr.contains("error"),
        "stderr should indicate the flag is unrecognized, got: {stderr}"
    );
}

// ── 11. Agent message without valid config ──────────────────────────

#[test]
fn agent_message_without_config_does_not_panic() {
    // Sending a message with no valid config may block waiting for the LLM,
    // so we spawn the process and give it a short window to check that it
    // at least starts without panicking. We use `spawn` + `wait_with_output`
    // with a timeout rather than `.output()` which blocks indefinitely.
    use std::time::{Duration, Instant};

    let mut child = weft_bin()
        .args(["agent", "-m", "test message"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn weft");

    let deadline = Instant::now() + Duration::from_secs(5);
    let exited = loop {
        if Instant::now() >= deadline {
            break false;
        }
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break false,
        }
    };

    if exited {
        // Process exited within the timeout -- verify no panic in output.
        let output = child.wait_with_output().expect("failed to read output");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{stdout}{stderr}");
        assert!(
            !combined.contains("panicked at") && !combined.contains("RUST_BACKTRACE"),
            "agent -m should not panic without config, got: {combined}"
        );
    } else {
        // Process is still running (blocking on LLM call). That's acceptable
        // -- it means it didn't panic during bootstrap. Kill it cleanly.
        let _ = child.kill();
        let _ = child.wait();
    }
}

// ── 12. Completions subcommand help ─────────────────────────────────

#[test]
fn completions_help_output() {
    let output = weft_bin()
        .args(["completions", "--help"])
        .output()
        .expect("failed to run weft");

    assert!(
        output.status.success(),
        "weft completions --help should exit 0"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("completions")
            || stdout.contains("Completions")
            || stdout.contains("shell"),
        "completions help should mention 'completions' or 'shell', got: {stdout}"
    );
}

// ── 13. Memory subcommand variants ──────────────────────────────────

#[test]
fn memory_history_succeeds() {
    let output = weft_bin()
        .args(["memory", "history"])
        .output()
        .expect("failed to run weft");

    assert!(output.status.success(), "weft memory history should exit 0");
}

#[test]
fn memory_search_succeeds() {
    let output = weft_bin()
        .args(["memory", "search", "test query"])
        .output()
        .expect("failed to run weft");

    assert!(output.status.success(), "weft memory search should exit 0");
}
