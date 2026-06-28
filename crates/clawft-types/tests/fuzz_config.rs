//! Fuzz-style tests for configuration deserialization.
//!
//! Generates random and malformed JSON inputs and verifies that
//! `serde_json::from_str` never panics -- it should always return
//! `Ok(...)` or `Err(...)` gracefully.

use clawft_types::config::Config;
use clawft_types::routing::RoutingConfig;

// ── Helpers ──────────────────────────────────────────────────────────────

/// Simple deterministic PRNG (xorshift64) for reproducible fuzz inputs
/// without requiring the `rand` crate as a dev-dep in clawft-types.
struct Xorshift64(u64);

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 { 1 } else { seed })
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn next_usize(&mut self, max: usize) -> usize {
        (self.next_u64() as usize) % max.max(1)
    }

    fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 == 0
    }

    fn next_byte(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }
}

/// Generate a random JSON-like string (may be invalid JSON).
fn random_json(rng: &mut Xorshift64, depth: usize) -> String {
    if depth > 4 {
        return random_json_leaf(rng);
    }

    match rng.next_usize(6) {
        0 => random_json_object(rng, depth),
        1 => random_json_array(rng, depth),
        _ => random_json_leaf(rng),
    }
}

fn random_json_leaf(rng: &mut Xorshift64) -> String {
    match rng.next_usize(7) {
        0 => "null".to_string(),
        1 => "true".to_string(),
        2 => "false".to_string(),
        3 => format!("{}", rng.next_u64() as i64),
        4 => format!("{:.2}", (rng.next_u64() % 10000) as f64 / 100.0),
        5 => {
            // Random string
            let len = rng.next_usize(30);
            let s: String = (0..len)
                .map(|_| {
                    let b = rng.next_byte();
                    // Mix of ASCII and some special chars
                    match b % 10 {
                        0 => '"',
                        1 => '\\',
                        2 => '\n',
                        3 => '{',
                        4 => '}',
                        _ => (b'a' + (b % 26)) as char,
                    }
                })
                .collect();
            format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
        }
        _ => {
            // Known field names to hit real config paths
            let names = [
                "\"agents\"",
                "\"channels\"",
                "\"providers\"",
                "\"gateway\"",
                "\"tools\"",
                "\"routing\"",
                "\"mode\"",
                "\"tiers\"",
            ];
            names[rng.next_usize(names.len())].to_string()
        }
    }
}

fn random_json_object(rng: &mut Xorshift64, depth: usize) -> String {
    let field_count = rng.next_usize(6);
    let fields: Vec<String> = (0..field_count)
        .map(|_| {
            let key = random_field_name(rng);
            let val = random_json(rng, depth + 1);
            format!("\"{}\":{}", key, val)
        })
        .collect();
    format!("{{{}}}", fields.join(","))
}

fn random_json_array(rng: &mut Xorshift64, depth: usize) -> String {
    let elem_count = rng.next_usize(5);
    let elems: Vec<String> = (0..elem_count)
        .map(|_| random_json(rng, depth + 1))
        .collect();
    format!("[{}]", elems.join(","))
}

fn random_field_name(rng: &mut Xorshift64) -> String {
    let known = [
        "agents",
        "channels",
        "providers",
        "gateway",
        "tools",
        "delegation",
        "routing",
        "voice",
        "kernel",
        "pipeline",
        "mode",
        "tiers",
        "scorer",
        "learner",
        "model",
        "max_tokens",
        "name",
        "enabled",
        "selection_strategy",
        "fallback_model",
        "complexity_range",
    ];

    if rng.next_bool() {
        known[rng.next_usize(known.len())].to_string()
    } else {
        let len = rng.next_usize(15) + 1;
        (0..len)
            .map(|_| (b'a' + rng.next_byte() % 26) as char)
            .collect()
    }
}

/// Generate completely malformed inputs (not valid JSON at all).
fn malformed_inputs() -> Vec<&'static str> {
    vec![
        "",
        " ",
        "\n\t\r",
        "{",
        "}",
        "}{",
        "[",
        "]",
        "[]",
        "{}{}",
        "{\"a\":}",
        "{:\"b\"}",
        "{\"a\"::\"b\"}",
        "{\"a\": \"b\",}",
        "[1,2,3,]",
        "undefined",
        "NaN",
        "Infinity",
        "-Infinity",
        "{'single_quotes': 'value'}",
        "{\"nested\": {\"deep\": {\"deeper\": {\"deepest\":",
        &"a]]]]][[[[[",
        "{\"key\": \"\\u0000\\u0001\\u0002\"}",
        "{\"key\": \"\\uDEAD\"}",
        "/*comment*/{}",
        "{\"a\": 1e999}",
        "{\"a\": -1e999}",
        "{\"a\": 0.0000000000000000000000001}",
        &"{\"a\": 9999999999999999999999999999999999999}",
    ]
}

// ── Config fuzz tests ────────────────────────────────────────────────────

#[test]
fn fuzz_config_random_json_no_panic() {
    for seed in 0..500 {
        let mut rng = Xorshift64::new(seed + 42);
        let json = random_json_object(&mut rng, 0);

        // This must not panic -- Ok or Err are both fine.
        let _result: Result<Config, _> = serde_json::from_str(&json);
    }
}

#[test]
fn fuzz_config_malformed_json_no_panic() {
    for input in malformed_inputs() {
        let _result: Result<Config, _> = serde_json::from_str(input);
        // Must not panic. Error is expected and fine.
    }
}

#[test]
fn fuzz_config_valid_partial_json() {
    // Valid JSON objects with some known fields and some garbage.
    let cases = vec![
        r#"{}"#,
        r#"{"agents": {}}"#,
        r#"{"agents": null}"#,
        r#"{"agents": 42}"#,
        r#"{"channels": "not_an_object"}"#,
        r#"{"routing": {"mode": "static"}}"#,
        r#"{"routing": {"mode": "tiered", "tiers": []}}"#,
        r#"{"routing": {"mode": 12345}}"#,
        r#"{"pipeline": {"scorer": "fitness", "learner": "trajectory"}}"#,
        r#"{"pipeline": {"scorer": null}}"#,
        r#"{"unknown_field": "should be ignored"}"#,
        r#"{"agents": {}, "channels": {}, "providers": {}, "gateway": {}, "tools": {}}"#,
    ];

    for json in &cases {
        let _result: Result<Config, _> = serde_json::from_str(json);
        // Must not panic.
    }
}

// ── RoutingConfig fuzz tests ─────────────────────────────────────────────

#[test]
fn fuzz_routing_config_random_json_no_panic() {
    for seed in 0..300 {
        let mut rng = Xorshift64::new(seed + 1000);
        let json = random_json_object(&mut rng, 0);

        let _result: Result<RoutingConfig, _> = serde_json::from_str(&json);
    }
}

#[test]
fn fuzz_routing_config_malformed_no_panic() {
    for input in malformed_inputs() {
        let _result: Result<RoutingConfig, _> = serde_json::from_str(input);
    }
}

#[test]
fn fuzz_routing_config_edge_cases() {
    let cases = vec![
        r#"{"mode": ""}"#,
        r#"{"mode": "static", "tiers": null}"#,
        r#"{"tiers": [{"name": "free", "models": [], "complexity_range": [0.0, 0.3]}]}"#,
        r#"{"tiers": [{}]}"#,
        r#"{"selection_strategy": "round_robin"}"#,
        r#"{"selection_strategy": "invalid_strategy"}"#,
        r#"{"fallback_model": "provider/model"}"#,
        r#"{"mode": "tiered", "tiers": [{"name": "x"}], "selection_strategy": "lowest_cost"}"#,
    ];

    for json in &cases {
        let _result: Result<RoutingConfig, _> = serde_json::from_str(json);
        // Must not panic.
    }
}

// ── Random byte sequence tests ───────────────────────────────────────────

#[test]
fn fuzz_config_random_bytes_no_panic() {
    for seed in 0..200 {
        let mut rng = Xorshift64::new(seed + 5000);
        let len = rng.next_usize(256);
        let bytes: Vec<u8> = (0..len).map(|_| rng.next_byte()).collect();

        if let Ok(text) = std::str::from_utf8(&bytes) {
            let _result: Result<Config, _> = serde_json::from_str(text);
        }
    }
}

#[test]
fn fuzz_config_deeply_nested_no_panic() {
    // Test deeply nested JSON to check for stack overflow in serde.
    let depth = 128;
    let mut json = String::new();
    for _ in 0..depth {
        json.push_str("{\"a\":");
    }
    json.push_str("null");
    for _ in 0..depth {
        json.push('}');
    }

    let _result: Result<Config, _> = serde_json::from_str(&json);
}

#[test]
fn fuzz_config_large_arrays_no_panic() {
    // Large array in a field that expects an object.
    let large_array = format!(
        "{{\"tiers\": [{}]}}",
        "null,".repeat(1000).trim_end_matches(',')
    );
    let _result: Result<RoutingConfig, _> = serde_json::from_str(&large_array);
}
