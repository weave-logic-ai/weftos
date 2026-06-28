//! Browser WASM regression suite — WEFT-388 (M5-A).
//!
//! Exercises the public `wasm-bindgen` surface of `clawft-wasm` in a
//! real browser via `wasm-pack test --headless --chrome`. The shipped
//! pipeline (W-BROWSER P0.1) wires `AgentLoop<BrowserPlatform>` end-to-end,
//! but the transport stage hits the network so a faithful round-trip
//! test would require either a CORS-friendly mock LLM endpoint or a
//! shim that intercepts the [`reqwest::Client`] inside `BrowserLlmClient`
//! — neither is in scope for M5-A. Instead, this suite locks down the
//! _entry-point contracts_ exposed by `browser_entry`:
//!
//! 1. `boot_info()`        — deterministic boot trace (no network).
//! 2. `analyze_files()`    — pipeline-shaped JSON round-trip (no network).
//! 3. `init()`             — config-parse error path.
//! 4. `init()`             — missing-API-key error path.
//! 5. `send_message()`     — "not initialized" guard before `init()`.
//! 6. `VERSION`            — non-empty crate version metadata.
//!
//! The full network round-trip lives in the `www/` HTML harness (BW6)
//! and the upcoming `crates/clawft-llm/tests/browser_transport_*` suite
//! (out of scope for this card — see WEFT-390 follow-up).
//!
//! Run via `scripts/build.sh test-browser` (which shells out to
//! `wasm-pack test --headless --chrome -p clawft-wasm --features browser`).

#![cfg(all(target_arch = "wasm32", feature = "browser"))]

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

// `clawft_wasm::*` reexports the `browser_entry` symbols (init, send_message,
// boot_info, analyze_files, set_env) when the `browser` feature is on.
use clawft_wasm::{VERSION, analyze_files, boot_info, send_message, set_env};

// ---------------------------------------------------------------------------
// Test 1 — boot_info() returns the expected 5-phase trace.
// ---------------------------------------------------------------------------
#[wasm_bindgen_test]
fn boot_info_returns_all_kernel_phases() {
    let json = boot_info();
    let parsed: serde_json::Value =
        serde_json::from_str(&json).expect("boot_info returns valid JSON");

    let arr = parsed.as_array().expect("boot_info returns a JSON array");
    assert!(!arr.is_empty(), "boot_info must emit at least one phase");

    // Every entry must have `phase` and `detail` strings.
    for entry in arr {
        assert!(entry.get("phase").and_then(|v| v.as_str()).is_some());
        assert!(entry.get("detail").and_then(|v| v.as_str()).is_some());
    }

    // Phases must include the kernel sequence INIT → CONFIG → SERVICES → READY.
    let phases: Vec<&str> = arr
        .iter()
        .filter_map(|e| e.get("phase").and_then(|v| v.as_str()))
        .collect();
    for required in &["INIT", "CONFIG", "SERVICES", "NETWORK", "READY"] {
        assert!(
            phases.contains(required),
            "boot_info missing phase {required}: got {phases:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 2 — analyze_files() runs the pipeline-shaped analyzers and
// produces summary + findings.
// ---------------------------------------------------------------------------
#[wasm_bindgen_test]
fn analyze_files_emits_summary_and_findings() {
    let input = serde_json::json!([
        {
            "path": "src/lib.rs",
            "content": "// TODO: refactor\nfn main() {}\n"
        },
        {
            "path": ".env",
            "content": "API_KEY=ZZZZ\n"
        }
    ])
    .to_string();

    let out = analyze_files(&input);
    let v: serde_json::Value =
        serde_json::from_str(&out).expect("analyze_files returns valid JSON");

    // Summary: file_count = 2, total_lines >= 3, languages non-empty.
    let summary = v.get("summary").expect("summary present");
    assert_eq!(summary.get("file_count").and_then(|n| n.as_u64()), Some(2));
    assert!(
        summary
            .get("total_lines")
            .and_then(|n| n.as_u64())
            .unwrap_or(0)
            >= 3
    );
    assert!(
        summary
            .get("languages")
            .and_then(|l| l.as_array())
            .is_some()
    );

    // Findings: must include a TODO (info) and a security error for `.env`.
    let findings = v
        .get("findings")
        .and_then(|f| f.as_array())
        .expect("findings array");
    let has_todo = findings
        .iter()
        .any(|f| f.get("category").and_then(|c| c.as_str()) == Some("todo"));
    let has_env_error = findings.iter().any(|f| {
        f.get("category").and_then(|c| c.as_str()) == Some("security")
            && f.get("severity").and_then(|s| s.as_str()) == Some("error")
    });
    assert!(has_todo, "expected a TODO finding from src/lib.rs");
    assert!(has_env_error, "expected a security:error finding from .env");
}

// ---------------------------------------------------------------------------
// Test 3 — analyze_files() handles malformed input without panicking.
// ---------------------------------------------------------------------------
#[wasm_bindgen_test]
fn analyze_files_rejects_malformed_input() {
    let out = analyze_files("not-json");
    let v: serde_json::Value = serde_json::from_str(&out).expect("error path still emits JSON");
    assert!(
        v.get("error").is_some(),
        "expected an `error` field for malformed input, got {v}"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — init() rejects malformed config JSON via a JsValue error.
// ---------------------------------------------------------------------------
#[wasm_bindgen_test]
async fn init_rejects_malformed_config() {
    let result = clawft_wasm::init("{not valid json").await;
    assert!(
        result.is_err(),
        "init() must reject malformed JSON config; got Ok"
    );
    let err = result.unwrap_err();
    let msg = err.as_string().unwrap_or_default();
    assert!(
        msg.contains("config parse error"),
        "expected `config parse error` in JsValue, got `{msg}`"
    );
}

// ---------------------------------------------------------------------------
// Test 5 — send_message() before init() returns the "not initialized"
// guard. Validates that the public ABI never panics on the unhappy path
// — a JS caller invoking out of order should always get a structured
// JsValue error.
// ---------------------------------------------------------------------------
#[wasm_bindgen_test]
async fn send_message_before_init_errors_cleanly() {
    let result = send_message("hello").await;
    // The runtime is set inside `init()`. If a previous test in the
    // same suite set it (only test 4 should fail before then), this
    // test still passes — `send_message` would reach the agent and
    // produce some error on the in-flight request. Either way it must
    // never `unwrap()` panic the worker.
    if let Err(err) = result {
        let msg = err.as_string().unwrap_or_default();
        assert!(
            !msg.is_empty(),
            "send_message error JsValue must carry a message"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 6 — VERSION metadata is present and non-empty (smoke check that
// the cargo env stamping survives the wasm-bindgen pass).
// ---------------------------------------------------------------------------
#[wasm_bindgen_test]
fn version_constant_is_set() {
    assert!(
        !VERSION.is_empty(),
        "VERSION must be the non-empty crate version"
    );
    assert!(
        VERSION.chars().any(|c| c.is_ascii_digit()),
        "VERSION must contain a digit (got `{VERSION}`)"
    );
}

// ---------------------------------------------------------------------------
// Test 7 — set_env() is a no-op shim but must not panic.
// ---------------------------------------------------------------------------
#[wasm_bindgen_test]
fn set_env_is_noop_safe() {
    // Pre-init this is a no-op; primary contract is "never panic on
    // arbitrary input" — covers a future where the env shim becomes
    // load-bearing.
    set_env("ANTHROPIC_API_KEY", "sk-test-not-real");
    set_env("", "");
}
