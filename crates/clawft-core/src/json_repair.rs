//! JSON repair for malformed LLM output.
//!
//! LLMs frequently return JSON that is almost-but-not-quite valid:
//!
//! - Wrapped in markdown fences (`` ```json ... ``` ``)
//! - Trailing commas after the last element in objects/arrays
//! - Unquoted keys (`{name: "value"}` instead of `{"name": "value"}`)
//! - Truncated output (missing closing brackets/braces)
//!
//! This module provides a lightweight repair pass (~100 lines of logic) that
//! attempts to fix these common issues before handing the string to
//! `serde_json::from_str`. It is **not** a full JSON parser -- it handles
//! the most common failure modes observed in tool-call responses from GPT-4,
//! Claude, and similar models.
//!
//! # Usage
//!
//! ```ignore
//! use clawft_core::json_repair::repair_json;
//!
//! let malformed = r#"```json
//! {"name": "test", "value": 42,}
//! ```"#;
//!
//! let repaired = repair_json(malformed);
//! let value: serde_json::Value = serde_json::from_str(&repaired).unwrap();
//! ```

/// Attempt to repair malformed JSON from LLM output.
///
/// Applies the following transformations in order:
///
/// 1. **Strip markdown fences**: Removes leading/trailing `` ```json `` / `` ``` `` markers.
/// 2. **Fix trailing commas**: Removes commas before `]` and `}`.
/// 3. **Fix unquoted keys**: Adds quotes around bare object keys.
/// 4. **Close truncated structures**: Appends missing `]` and `}` to balance brackets.
///
/// Returns the repaired string. If the input is already valid JSON, it is
/// returned unchanged (modulo whitespace from fence stripping).
pub fn repair_json(input: &str) -> String {
    let stripped = strip_markdown_fences(input);
    let no_trailing = fix_trailing_commas(&stripped);
    let quoted_keys = fix_unquoted_keys(&no_trailing);
    close_truncated(&quoted_keys)
}

/// Try to parse JSON, falling back to repair if initial parse fails.
///
/// This is the recommended entry point for tool-call argument parsing.
/// It avoids the repair overhead when the JSON is already valid.
pub fn parse_with_repair(input: &str) -> Result<serde_json::Value, serde_json::Error> {
    // Fast path: try parsing as-is first.
    if let Ok(val) = serde_json::from_str(input) {
        return Ok(val);
    }

    // Slow path: repair and retry.
    let repaired = repair_json(input);
    serde_json::from_str(&repaired)
}

// ---------------------------------------------------------------------------
// Step 1: Strip markdown fences
// ---------------------------------------------------------------------------

/// Remove markdown code fences from around JSON content.
///
/// Handles:
/// - `` ```json\n...\n``` ``
/// - `` ```\n...\n``` ``
/// - Leading/trailing whitespace
fn strip_markdown_fences(input: &str) -> String {
    let trimmed = input.trim();

    // Check for opening fence: ```json or ```
    let after_open = if let Some(stripped) = trimmed.strip_prefix("```json") {
        stripped
    } else if let Some(stripped) = trimmed.strip_prefix("```") {
        stripped
    } else {
        return trimmed.to_string();
    };

    // Strip leading newline after fence
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);

    // Check for closing fence
    let before_close = if let Some(stripped) = after_open.trim_end().strip_suffix("```") {
        stripped.trim_end()
    } else {
        after_open
    };

    before_close.to_string()
}

// ---------------------------------------------------------------------------
// Step 2: Fix trailing commas
// ---------------------------------------------------------------------------

/// Remove trailing commas before `]` and `}`.
///
/// Handles nested structures and whitespace between the comma and closing
/// bracket. Respects string literals (does not modify commas inside strings).
fn fix_trailing_commas(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(len);
    let mut in_string = false;
    let mut escape_next = false;
    let mut i = 0;

    while i < len {
        let c = chars[i];

        if escape_next {
            result.push(c);
            escape_next = false;
            i += 1;
            continue;
        }

        if c == '\\' && in_string {
            result.push(c);
            escape_next = true;
            i += 1;
            continue;
        }

        if c == '"' {
            in_string = !in_string;
            result.push(c);
            i += 1;
            continue;
        }

        if in_string {
            result.push(c);
            i += 1;
            continue;
        }

        // Outside a string, check if this comma is trailing.
        if c == ',' {
            // Look ahead past whitespace for `]` or `}`.
            let mut j = i + 1;
            while j < len && chars[j].is_whitespace() {
                j += 1;
            }
            if j < len && (chars[j] == ']' || chars[j] == '}') {
                // Skip the trailing comma; push the whitespace and closer.
                i += 1;
                continue;
            }
        }

        result.push(c);
        i += 1;
    }

    result
}

// ---------------------------------------------------------------------------
// Step 3: Fix unquoted keys
// ---------------------------------------------------------------------------

/// Add double-quotes around unquoted object keys.
///
/// Detects patterns like `{ key:` or `, key:` where `key` is a bare
/// identifier (alphanumeric + underscore + dollar sign), and wraps it in
/// double-quotes.
fn fix_unquoted_keys(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(len + 32);
    let mut in_string = false;
    let mut escape_next = false;
    let mut i = 0;

    while i < len {
        let c = chars[i];

        if escape_next {
            result.push(c);
            escape_next = false;
            i += 1;
            continue;
        }

        if c == '\\' && in_string {
            result.push(c);
            escape_next = true;
            i += 1;
            continue;
        }

        if c == '"' {
            in_string = !in_string;
            result.push(c);
            i += 1;
            continue;
        }

        if in_string {
            result.push(c);
            i += 1;
            continue;
        }

        // Outside a string: check if we are at the start of an unquoted key.
        // An unquoted key appears after `{`, `,`, or newline (with optional
        // whitespace) and is followed by `:`.
        if is_key_start_char(c) {
            // Collect the identifier.
            let start = i;
            while i < len && is_key_char(chars[i]) {
                i += 1;
            }
            let key = &chars[start..i];

            // Skip whitespace after the identifier.
            let mut j = i;
            while j < len && chars[j].is_whitespace() {
                j += 1;
            }

            if j < len && chars[j] == ':' {
                // This is an unquoted key -- wrap it.
                result.push('"');
                result.extend(key);
                result.push('"');
            } else {
                // Not a key -- emit as-is (likely a value like `true`, `false`, `null`).
                result.extend(key);
            }
            continue;
        }

        result.push(c);
        i += 1;
    }

    result
}

/// Characters that can start an unquoted key: letters, underscore, dollar.
fn is_key_start_char(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '$'
}

/// Characters that can appear in an unquoted key body.
fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

// ---------------------------------------------------------------------------
// Step 4: Close truncated structures
// ---------------------------------------------------------------------------

/// Append missing closing brackets, braces, and string quotes to balance
/// the JSON structure.
///
/// Scans through the input tracking open/close pairs while respecting
/// string literals, then appends any missing closers. Also closes
/// unterminated string literals.
fn close_truncated(input: &str) -> String {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escape_next = false;

    for c in input.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }

        if c == '\\' && in_string {
            escape_next = true;
            continue;
        }

        if c == '"' {
            in_string = !in_string;
            continue;
        }

        if in_string {
            continue;
        }

        match c {
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']'
                if stack.last() == Some(&c) => {
                    stack.pop();
                }
            _ => {}
        }
    }

    if !in_string && stack.is_empty() {
        return input.to_string();
    }

    let mut result = input.to_string();

    // Close unclosed string literal.
    if in_string {
        result.push('"');
    }

    // Append structural closers in reverse order.
    for closer in stack.into_iter().rev() {
        result.push(closer);
    }
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── strip_markdown_fences ──────────────────────────────────────────

    #[test]
    fn strip_json_fence() {
        let input = "```json\n{\"key\": \"value\"}\n```";
        assert_eq!(strip_markdown_fences(input), "{\"key\": \"value\"}");
    }

    #[test]
    fn strip_plain_fence() {
        let input = "```\n{\"a\": 1}\n```";
        assert_eq!(strip_markdown_fences(input), "{\"a\": 1}");
    }

    #[test]
    fn strip_fence_with_whitespace() {
        let input = "  ```json\n{\"x\": true}\n```  ";
        assert_eq!(strip_markdown_fences(input), "{\"x\": true}");
    }

    #[test]
    fn no_fence_unchanged() {
        let input = "{\"a\": 1}";
        assert_eq!(strip_markdown_fences(input), input);
    }

    #[test]
    fn fence_without_closing() {
        let input = "```json\n{\"a\": 1}";
        assert_eq!(strip_markdown_fences(input), "{\"a\": 1}");
    }

    // ── fix_trailing_commas ────────────────────────────────────────────

    #[test]
    fn trailing_comma_object() {
        let input = r#"{"a": 1, "b": 2,}"#;
        let fixed = fix_trailing_commas(input);
        assert_eq!(fixed, r#"{"a": 1, "b": 2}"#);
    }

    #[test]
    fn trailing_comma_array() {
        let input = "[1, 2, 3,]";
        let fixed = fix_trailing_commas(input);
        assert_eq!(fixed, "[1, 2, 3]");
    }

    #[test]
    fn trailing_comma_nested() {
        let input = r#"{"a": [1, 2,], "b": {"c": 3,},}"#;
        let fixed = fix_trailing_commas(input);
        assert_eq!(fixed, r#"{"a": [1, 2], "b": {"c": 3}}"#);
    }

    #[test]
    fn trailing_comma_with_whitespace() {
        let input = "{\n  \"a\": 1,\n}";
        let fixed = fix_trailing_commas(input);
        assert_eq!(fixed, "{\n  \"a\": 1\n}");
    }

    #[test]
    fn no_trailing_comma_unchanged() {
        let input = r#"{"a": 1, "b": 2}"#;
        let fixed = fix_trailing_commas(input);
        assert_eq!(fixed, input);
    }

    #[test]
    fn comma_in_string_preserved() {
        let input = r#"{"a": "hello,}"#;
        let fixed = fix_trailing_commas(input);
        // The comma is inside a string -- should not be removed.
        assert_eq!(fixed, r#"{"a": "hello,}"#);
    }

    // ── fix_unquoted_keys ──────────────────────────────────────────────

    #[test]
    fn unquoted_simple_key() {
        let input = r#"{name: "test"}"#;
        let fixed = fix_unquoted_keys(input);
        assert_eq!(fixed, r#"{"name": "test"}"#);
    }

    #[test]
    fn unquoted_multiple_keys() {
        let input = r#"{name: "test", value: 42}"#;
        let fixed = fix_unquoted_keys(input);
        assert_eq!(fixed, r#"{"name": "test", "value": 42}"#);
    }

    #[test]
    fn unquoted_nested_keys() {
        let input = r#"{outer: {inner: "val"}}"#;
        let fixed = fix_unquoted_keys(input);
        assert_eq!(fixed, r#"{"outer": {"inner": "val"}}"#);
    }

    #[test]
    fn quoted_keys_unchanged() {
        let input = r#"{"name": "test", "value": 42}"#;
        let fixed = fix_unquoted_keys(input);
        assert_eq!(fixed, input);
    }

    #[test]
    fn boolean_values_not_quoted() {
        let input = r#"{"a": true, "b": false, "c": null}"#;
        let fixed = fix_unquoted_keys(input);
        assert_eq!(fixed, input);
    }

    #[test]
    fn unquoted_key_with_underscore() {
        let input = r#"{my_key: "val"}"#;
        let fixed = fix_unquoted_keys(input);
        assert_eq!(fixed, r#"{"my_key": "val"}"#);
    }

    // ── close_truncated ────────────────────────────────────────────────

    #[test]
    fn missing_close_brace() {
        let input = r#"{"a": 1"#;
        let fixed = close_truncated(input);
        assert_eq!(fixed, r#"{"a": 1}"#);
    }

    #[test]
    fn missing_close_bracket() {
        let input = "[1, 2, 3";
        let fixed = close_truncated(input);
        assert_eq!(fixed, "[1, 2, 3]");
    }

    #[test]
    fn missing_nested_closers() {
        let input = r#"{"a": [1, 2, {"b": 3"#;
        let fixed = close_truncated(input);
        assert_eq!(fixed, r#"{"a": [1, 2, {"b": 3}]}"#);
    }

    #[test]
    fn balanced_unchanged() {
        let input = r#"{"a": [1, 2]}"#;
        let fixed = close_truncated(input);
        assert_eq!(fixed, input);
    }

    #[test]
    fn truncated_in_string() {
        // Brace inside a string should not be counted.
        // The string is unclosed, so we close the string first, then the object.
        let input = r#"{"a": "hello {world"#;
        let fixed = close_truncated(input);
        assert_eq!(fixed, r#"{"a": "hello {world"}"#);
    }

    // ── repair_json (integration) ──────────────────────────────────────

    #[test]
    fn repair_fenced_with_trailing_comma() {
        let input = "```json\n{\"key\": \"value\",}\n```";
        let repaired = repair_json(input);
        let val: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(val["key"], "value");
    }

    #[test]
    fn repair_truncated_with_unquoted_keys() {
        let input = r#"{name: "test", count: 42"#;
        let repaired = repair_json(input);
        let val: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(val["name"], "test");
        assert_eq!(val["count"], 42);
    }

    #[test]
    fn repair_all_issues_combined() {
        let input = "```json\n{name: \"test\", items: [1, 2,],}\n```";
        let repaired = repair_json(input);
        let val: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(val["name"], "test");
        assert_eq!(val["items"], serde_json::json!([1, 2]));
    }

    #[test]
    fn repair_valid_json_unchanged() {
        let input = r#"{"key": "value"}"#;
        let repaired = repair_json(input);
        assert_eq!(repaired, input);
    }

    #[test]
    fn repair_deeply_truncated() {
        let input = r#"{"tools": [{"name": "read_file", "args": {"path": "/tmp"#;
        let repaired = repair_json(input);
        let val: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(val["tools"][0]["name"], "read_file");
    }

    // ── parse_with_repair ──────────────────────────────────────────────

    #[test]
    fn parse_valid_json_fast_path() {
        let input = r#"{"a": 1}"#;
        let val = parse_with_repair(input).unwrap();
        assert_eq!(val["a"], 1);
    }

    #[test]
    fn parse_malformed_json_repair_path() {
        let input = "```json\n{\"a\": 1,}\n```";
        let val = parse_with_repair(input).unwrap();
        assert_eq!(val["a"], 1);
    }

    #[test]
    fn parse_truly_broken_json_fails() {
        let input = "this is not json at all";
        assert!(parse_with_repair(input).is_err());
    }

    #[test]
    fn parse_empty_object() {
        let input = "{}";
        let val = parse_with_repair(input).unwrap();
        assert!(val.is_object());
    }

    #[test]
    fn parse_fenced_array() {
        let input = "```\n[1, 2, 3]\n```";
        let val = parse_with_repair(input).unwrap();
        assert_eq!(val, serde_json::json!([1, 2, 3]));
    }

    // ── Edge cases ─────────────────────────────────────────────────────

    #[test]
    fn escaped_quotes_in_string() {
        let input = r#"{"msg": "he said \"hello\""}"#;
        let repaired = repair_json(input);
        let val: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(val["msg"], r#"he said "hello""#);
    }

    #[test]
    fn empty_input() {
        let repaired = repair_json("");
        assert_eq!(repaired, "");
    }

    #[test]
    fn just_fences_no_content() {
        let input = "```json\n```";
        let repaired = repair_json(input);
        assert_eq!(repaired, "");
    }

    #[test]
    fn multiple_trailing_commas() {
        let input = r#"{"a": 1,, "b": 2,}"#;
        let fixed = fix_trailing_commas(input);
        // First comma is not trailing (there's another comma after it, not a closer).
        // Second comma before } is trailing.
        assert_eq!(fixed, r#"{"a": 1,, "b": 2}"#);
    }

    #[test]
    fn repair_real_llm_tool_call() {
        // Simulate a real LLM output with markdown fences and trailing comma.
        let input = r#"```json
{
  "path": "/home/user/project/src/main.rs",
  "content": "fn main() {\n    println!(\"Hello, world!\");\n}",
}
```"#;
        let val = parse_with_repair(input).unwrap();
        assert_eq!(val["path"], "/home/user/project/src/main.rs");
        assert!(val["content"].as_str().unwrap().contains("Hello, world!"));
    }

    #[test]
    fn repair_truncated_tool_call() {
        // Simulate an LLM output truncated mid-stream.
        let input = r#"{"query": "rust programming", "num_results": 5"#;
        let val = parse_with_repair(input).unwrap();
        assert_eq!(val["query"], "rust programming");
        assert_eq!(val["num_results"], 5);
    }
}
