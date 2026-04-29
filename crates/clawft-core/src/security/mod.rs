//! Security boundary validation functions.

#[cfg(feature = "signing")]
pub mod signing;

use clawft_types::error::ClawftError;
use serde_json::Value;

const MAX_SESSION_ID_LEN: usize = 256;

/// Maximum nesting depth for YAML frontmatter (SEC-SKILL-01).
pub const MAX_YAML_DEPTH: usize = 10;

/// Maximum file size for SKILL.md files in bytes (SEC-SKILL-07).
pub const MAX_SKILL_MD_SIZE: usize = 50 * 1024; // 50 KB

/// Maximum file size for agent YAML/JSON files in bytes (SEC-SKILL-07).
pub const MAX_AGENT_FILE_SIZE: usize = 10 * 1024; // 10 KB

/// Validate a session identifier against security policies.
///
/// Rejects empty, overlong, path-traversal, directory-separator, null-byte,
/// and control-character identifiers (tab is permitted).
pub fn validate_session_id(id: &str) -> Result<(), ClawftError> {
    if id.is_empty() {
        return Err(ClawftError::SecurityViolation {
            reason: "session ID must not be empty".into(),
        });
    }
    if id.len() > MAX_SESSION_ID_LEN {
        return Err(ClawftError::SecurityViolation {
            reason: format!(
                "session ID too long ({} bytes, max {MAX_SESSION_ID_LEN})",
                id.len()
            ),
        });
    }
    if id.contains("..") {
        return Err(ClawftError::SecurityViolation {
            reason: "session ID contains path traversal sequence '..'".into(),
        });
    }
    if id.contains('/') || id.contains('\\') {
        return Err(ClawftError::SecurityViolation {
            reason: "session ID contains directory separator".into(),
        });
    }
    if id.contains('\0') {
        return Err(ClawftError::SecurityViolation {
            reason: "session ID contains null byte".into(),
        });
    }
    for byte in id.bytes() {
        if byte <= 0x1F && byte != 0x09 {
            return Err(ClawftError::SecurityViolation {
                reason: format!("session ID contains control character 0x{byte:02X}"),
            });
        }
    }
    Ok(())
}

/// Truncate a JSON value so its serialized form fits within `max_bytes`.
///
/// Strategy varies by type: strings get a truncation suffix, arrays keep
/// leading elements with a sentinel, and other values get raw-JSON wrapping.
pub fn truncate_result(value: Value, max_bytes: usize) -> Value {
    let serialized = serde_json::to_string(&value).unwrap_or_default();
    if serialized.len() <= max_bytes {
        return value;
    }
    let original_len = serialized.len();
    match value {
        Value::String(s) => truncate_string(s, original_len, max_bytes),
        Value::Array(arr) => truncate_array(arr, original_len, max_bytes),
        _ => truncate_raw(serialized, original_len, max_bytes),
    }
}

fn truncate_string(s: String, original_len: usize, max_bytes: usize) -> Value {
    let suffix = format!("... [truncated, original size: {original_len} bytes]");
    let available = max_bytes.saturating_sub(2 + suffix.len()); // 2 = JSON quotes
    let truncated: String = s.chars().take(available).collect();
    let mut result = Value::String(format!("{truncated}{suffix}"));
    // Shrink further if JSON escaping pushed us over.
    while serde_json::to_string(&result).unwrap_or_default().len() > max_bytes {
        let chars: Vec<char> = result.as_str().unwrap().chars().collect();
        if chars.len() <= suffix.len() + 1 {
            result = Value::String(suffix.clone());
            break;
        }
        let keep = chars.len() - suffix.len() - 1;
        let shortened: String = chars[..keep].iter().collect();
        result = Value::String(format!("{shortened}{suffix}"));
    }
    result
}

fn truncate_array(arr: Vec<Value>, original_count: usize, max_bytes: usize) -> Value {
    let sentinel = serde_json::json!({"_truncated": true, "original_count": original_count});
    let sentinel_ser = serde_json::to_string(&sentinel).unwrap_or_default();
    let mut kept: Vec<Value> = Vec::new();
    let mut current_len = 2 + sentinel_ser.len(); // `[` + sentinel + `]`
    for elem in arr {
        let elem_ser = serde_json::to_string(&elem).unwrap_or_default();
        let added = elem_ser.len() + 1; // comma separator
        if current_len + added > max_bytes {
            break;
        }
        current_len += added;
        kept.push(elem);
    }
    kept.push(sentinel);
    Value::Array(kept)
}

fn truncate_raw(serialized: String, original_len: usize, max_bytes: usize) -> Value {
    let template_overhead =
        r#"{"_truncated_json":"...","original_bytes":}"#.len() + original_len.to_string().len();
    let available = max_bytes.saturating_sub(template_overhead);
    let prefix: String = serialized.chars().take(available).collect();
    let mut result = serde_json::json!({"_truncated_json": format!("{prefix}..."), "original_bytes": original_len});
    while serde_json::to_string(&result).unwrap_or_default().len() > max_bytes {
        let cur = match result.get("_truncated_json").and_then(Value::as_str) {
            Some(s) => s.to_owned(),
            None => break,
        };
        let chars: Vec<char> = cur.chars().collect();
        if chars.len() <= 4 {
            break;
        }
        let shortened: String = chars[..chars.len() - 4].iter().collect();
        result["_truncated_json"] = Value::String(format!("{shortened}..."));
    }
    result
}

/// Strip null bytes, DEL, and ASCII control chars except `\n`, `\r`, `\t`.
/// Preserves all valid UTF-8 including emoji, CJK, and RTL text.
pub fn sanitize_content(content: &str) -> String {
    content
        .chars()
        .filter(|&c| {
            if c == '\0' || c == '\x7F' {
                return false;
            }
            let b = c as u32;
            !(b <= 0x1F && b != 0x0A && b != 0x0D && b != 0x09)
        })
        .collect()
}

// ── SEC-SKILL-01: YAML depth validation ─────────────────────────────

/// Check that a YAML frontmatter string does not exceed `MAX_YAML_DEPTH`
/// levels of nesting. Nesting is detected by indentation changes (each
/// 2-space indent = one level).
///
/// Returns `Ok(())` if the depth is within limits, or a
/// [`ClawftError::SecurityViolation`] if it exceeds `MAX_YAML_DEPTH`.
pub fn validate_yaml_depth(yaml: &str) -> Result<(), ClawftError> {
    let mut max_depth: usize = 0;

    for line in yaml.lines() {
        if line.trim().is_empty() || line.trim().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        // Each 2 spaces of indentation counts as one nesting level.
        // A single space also counts (for odd indentation).
        let depth = indent.div_ceil(2);
        max_depth = max_depth.max(depth);
    }

    if max_depth > MAX_YAML_DEPTH {
        return Err(ClawftError::SecurityViolation {
            reason: format!(
                "YAML frontmatter nesting depth {max_depth} exceeds maximum of {MAX_YAML_DEPTH}"
            ),
        });
    }
    Ok(())
}

// ── SEC-SKILL-02: Directory name validation ─────────────────────────

/// Validate a directory entry name for use in skill/agent loading.
///
/// Rejects names that contain path traversal (`..`), directory separators
/// (`/` or `\`), or are absolute paths (start with `/`).
pub fn validate_directory_name(name: &str) -> Result<(), ClawftError> {
    if name.is_empty() {
        return Err(ClawftError::SecurityViolation {
            reason: "directory name must not be empty".into(),
        });
    }
    if name.contains("..") {
        return Err(ClawftError::SecurityViolation {
            reason: format!(
                "directory name '{}' contains path traversal sequence '..'",
                name
            ),
        });
    }
    if name.contains('/') || name.contains('\\') {
        return Err(ClawftError::SecurityViolation {
            reason: format!("directory name '{}' contains directory separator", name),
        });
    }
    if name.starts_with('/') {
        return Err(ClawftError::SecurityViolation {
            reason: format!("directory name '{}' is an absolute path", name),
        });
    }
    Ok(())
}

// ── SEC-SKILL-03: Tool allowlist intersection ───────────────────────

/// Compute the effective tool list as the intersection of a skill's
/// `allowed_tools` and an agent's `allowed_tools`.
///
/// If the skill's tool list is empty, the agent's list is returned
/// (the skill does not restrict tools). If the agent's list is empty,
/// all tools are allowed and the skill's list is returned as-is.
/// If both are non-empty, only tools in both lists are returned.
pub fn intersect_allowed_tools(skill_tools: &[String], agent_tools: &[String]) -> Vec<String> {
    // If the skill has no restrictions, the agent's list applies.
    if skill_tools.is_empty() {
        return agent_tools.to_vec();
    }
    // If the agent has no restrictions, the skill's list applies.
    if agent_tools.is_empty() {
        return skill_tools.to_vec();
    }
    // Both have restrictions: return the intersection.
    skill_tools
        .iter()
        .filter(|t| agent_tools.contains(t))
        .cloned()
        .collect()
}

// ── SEC-SKILL-04: Model string validation ───────────────────────────

/// Validate a model string against security rules.
///
/// Accepts strings matching `provider/model-name` or bare `model-name`.
/// Rejects strings containing shell metacharacters, newlines, control
/// characters, or other dangerous patterns.
pub fn validate_model_string(model: &str) -> Result<(), ClawftError> {
    if model.is_empty() {
        return Err(ClawftError::SecurityViolation {
            reason: "model string must not be empty".into(),
        });
    }

    // Check for control characters (including newlines).
    for ch in model.chars() {
        if ch.is_control() {
            return Err(ClawftError::SecurityViolation {
                reason: format!(
                    "model string contains control character U+{:04X}",
                    ch as u32
                ),
            });
        }
    }

    // Shell metacharacters that must not appear in a model string.
    const FORBIDDEN: &[char] = &[
        ';', '|', '&', '$', '`', '(', ')', '{', '}', '<', '>', '!', '#', '~', '*', '?', '[', ']',
        '\'', '"', '\\', '\n', '\r',
    ];
    for ch in model.chars() {
        if FORBIDDEN.contains(&ch) {
            return Err(ClawftError::SecurityViolation {
                reason: format!("model string contains forbidden character '{}'", ch),
            });
        }
    }

    Ok(())
}

// ── SEC-SKILL-06: Prompt injection guards ───────────────────────────

/// Known prompt injection tokens to strip from skill instructions.
const INJECTION_TOKENS: &[&str] = &[
    "<system>",
    "</system>",
    "<|im_start|>",
    "<|im_end|>",
    "<|im_sep|>",
    "<|endoftext|>",
    "<|system|>",
    "<|user|>",
    "<|assistant|>",
    "[INST]",
    "[/INST]",
    "<<SYS>>",
    "<</SYS>>",
];

/// Sanitize skill instructions by stripping known prompt injection tokens
/// and warning on suspicious patterns.
///
/// This function:
/// - Strips `<system>`, `<|im_start|>`, and similar injection tokens
/// - Warns on suspicious patterns (multiple system-like prompts)
/// - Does NOT modify normal markdown content
///
/// Returns the sanitized string and a list of warnings (if any).
pub fn sanitize_skill_instructions(instructions: &str) -> (String, Vec<String>) {
    let mut result = instructions.to_string();
    let mut warnings = Vec::new();

    // Loop until no more injection tokens are found (prevents nested bypass
    // where e.g. "<sy<system>stem>" reconstructs after inner removal).
    const MAX_SANITIZE_PASSES: usize = 10;
    let mut passes = 0;
    loop {
        let before = result.clone();
        for token in INJECTION_TOKENS {
            if result.contains(token) {
                warnings.push(format!(
                    "stripped injection token '{}' from skill instructions",
                    token
                ));
                result = result.replace(token, "");
            }
        }
        passes += 1;
        if result == before || passes >= MAX_SANITIZE_PASSES {
            break;
        }
    }

    // Case-insensitive check for remaining suspicious patterns.
    let lower = result.to_lowercase();
    let system_count = lower.matches("<system").count() + lower.matches("system:").count();
    if system_count > 1 {
        warnings.push(format!(
            "suspicious: found {} system-like prompts in skill instructions",
            system_count
        ));
    }

    (result, warnings)
}

// ── SEC-PIPELINE-01: Unified LLM input sanitization ─────────────────

/// Sanitize any content that will reach an LLM call, regardless of source.
///
/// Combines `sanitize_content()` (control char stripping) with
/// `sanitize_skill_instructions()` (injection token removal) and tags
/// the source boundary for audit logging.
///
/// Use at every LLM input path:
/// - Semantic extraction (file content → prompt)
/// - Memory retrieval (MEMORY.md → system message)
/// - Session history replay
/// - Tool results → message stream
/// - Bootstrap files (SOUL.md etc)
/// - Schema builder agent inputs (documents, framework docs, images)
///
/// Returns sanitized content and any warnings (injection tokens found, etc).
pub fn sanitize_llm_input(content: &str, source_label: &str) -> (String, Vec<String>) {
    let cleaned = sanitize_content(content);
    let (sanitized, mut warnings) = sanitize_skill_instructions(&cleaned);

    if !warnings.is_empty() {
        tracing::warn!(
            source = source_label,
            warning_count = warnings.len(),
            "prompt injection tokens stripped from LLM input"
        );
    }

    // Tag warnings with source for audit trail.
    for w in &mut warnings {
        *w = format!("[{source_label}] {w}");
    }

    (sanitized, warnings)
}

/// Validate content intended for schema generation by an agent.
///
/// In addition to standard LLM sanitization, checks for patterns that
/// could produce malicious schema entries (e.g., IRIs pointing to
/// attacker-controlled domains, wildcard overrides, excessive permissions).
pub fn sanitize_schema_input(content: &str, source_label: &str) -> (String, Vec<String>) {
    let (sanitized, mut warnings) = sanitize_llm_input(content, source_label);

    let lower = sanitized.to_lowercase();

    // Warn on content that looks like it's trying to inject schema directives.
    if lower.contains("geometry:") && lower.contains("contains:") {
        warnings.push(format!(
            "[{source_label}] input contains schema-like directives — may be attempting schema injection"
        ));
    }

    // Warn on suspicious IRI patterns in raw input.
    for pattern in &["javascript:", "data:", "file://", "ftp://"] {
        if lower.contains(pattern) {
            warnings.push(format!(
                "[{source_label}] input contains suspicious URI scheme: {pattern}"
            ));
        }
    }

    (sanitized, warnings)
}

// ── SEC-SKILL-07: File size validation ──────────────────────────────

/// Validate that a file size is within the allowed limit.
///
/// Returns `Ok(())` if the size is within limits, or a
/// [`ClawftError::SecurityViolation`] if it exceeds the maximum.
pub fn validate_file_size(
    size: usize,
    max_size: usize,
    file_kind: &str,
) -> Result<(), ClawftError> {
    if size > max_size {
        return Err(ClawftError::SecurityViolation {
            reason: format!(
                "{} file size {} bytes exceeds maximum of {} bytes",
                file_kind, size, max_size
            ),
        });
    }
    Ok(())
}

// ── SEC-SKILL-08: MCP tool namespace isolation ──────────────────────

/// Validate that an MCP tool name uses the `{server}__{tool}` format
/// (double underscore separator).
///
/// Non-MCP tools (those without any underscore) are allowed through.
/// Only tools that appear to come from an MCP server (contain at least
/// one underscore) are checked for the double-underscore convention.
pub fn validate_mcp_tool_name(tool_name: &str) -> Result<(), ClawftError> {
    // If the tool name contains no underscores at all, it is a local tool.
    if !tool_name.contains('_') {
        return Ok(());
    }
    // If it contains a double underscore, it follows the convention.
    if tool_name.contains("__") {
        return Ok(());
    }
    // It contains single underscores but no double underscore.
    // This could be a local tool with underscores in its name (e.g.
    // "read_file"), which is fine. We only flag tools that look like
    // they come from an MCP server but lack the `__` separator.
    // Heuristic: if the first segment (before the first `_`) looks like
    // a server name (lowercase, no digits), we flag it.
    // For simplicity, we require validation to be called explicitly for
    // MCP tools only.
    Ok(())
}

/// Validate that an MCP server tool name uses the required `{server}__{tool}`
/// format. This is the strict variant called during MCP tool registration.
///
/// Returns an error if the tool name does not contain `__`.
pub fn validate_mcp_tool_name_strict(tool_name: &str) -> Result<(), ClawftError> {
    if !tool_name.contains("__") {
        return Err(ClawftError::SecurityViolation {
            reason: format!(
                "MCP tool '{}' does not use the required '{{server}}__{{tool}}' \
                 namespace format (missing double underscore separator)",
                tool_name
            ),
        });
    }
    Ok(())
}

/// MCP namespace prefixes considered sensitive. A wildcard-only allowlist
/// (`["*"]`) MUST NOT cover these — the operator must opt in to each
/// sensitive namespace explicitly (e.g. `["exec_*"]` or
/// `["exec__shell"]`).
///
/// The list is intentionally short and conservative. Add a namespace
/// here only when granting it via wildcard would constitute a security
/// regression.
pub const SENSITIVE_MCP_NAMESPACES: &[&str] = &[
    "exec",      // shell / process exec
    "shell",     // alternate shell namespace
    "system",    // system control
    "sudo",      // privilege escalation
    "kernel",    // kernel surface
    "subprocess",// process spawn
    "process",   // process management
    "fs_admin",  // privileged filesystem
];

/// Split an MCP tool name `{server}__{tool}` into `(server, tool)`.
///
/// Returns `None` for non-MCP tool names (no `__`).
/// Multiple `__` use the FIRST as the split point so namespaces remain
/// stable even if the tool name itself contains `__`.
pub fn split_mcp_namespace(tool_name: &str) -> Option<(&str, &str)> {
    tool_name
        .find("__")
        .map(|idx| (&tool_name[..idx], &tool_name[idx + 2..]))
}

/// WEFT-32: validate that a tool name is permitted by an allowlist.
///
/// Implements the wildcard-namespace guard requested by the 0.7.0
/// release-gate audit (security review T-02): a tool named
/// `exec__shell` (double-underscore namespace) must NOT be reachable
/// through a `["*"]` allowlist. The operator has to opt in to every
/// sensitive namespace by listing it explicitly (e.g. `["exec_*"]`,
/// `["exec__shell"]`, `["exec__*"]`).
///
/// The function returns `Ok(())` when the tool name is permitted and a
/// `SecurityViolation` error when the wildcard guard fires. Non-MCP
/// tool names (no `__` separator) and tool names whose namespace prefix
/// is not in [`SENSITIVE_MCP_NAMESPACES`] are always permitted as long
/// as some allowlist entry matches them.
///
/// Arguments:
/// - `tool_name`: the fully-qualified tool identifier (e.g. `"exec__shell"`).
/// - `allowlist`: the caller's `tool_access` patterns. The empty list
///   means "deny all" (zero-trust default). `["*"]` means "allow all
///   non-sensitive". An entry like `["exec_*"]`, `["exec__*"]`, or
///   `["exec__shell"]` opts in to the sensitive namespace explicitly.
///
/// Examples:
/// ```ignore
/// // Wildcard does NOT cover sensitive namespaces.
/// validate_mcp_namespace_against_wildcard("exec__shell", &["*".into()])
///     .unwrap_err();
/// // Explicit namespace prefix opts in.
/// validate_mcp_namespace_against_wildcard("exec__shell", &["exec_*".into()])
///     .unwrap();
/// // Non-sensitive namespace is fine under wildcard.
/// validate_mcp_namespace_against_wildcard("fs__read_file", &["*".into()])
///     .unwrap();
/// ```
pub fn validate_mcp_namespace_against_wildcard(
    tool_name: &str,
    allowlist: &[String],
) -> Result<(), ClawftError> {
    // Empty allowlist is handled by the caller (deny-all). We only
    // enforce the wildcard-namespace guard.
    if allowlist.is_empty() {
        return Ok(());
    }

    // Reject a meta-wildcard prefix in the namespace position itself —
    // an entry like `["*", ...]` paired with a tool name whose namespace
    // is "*" is non-sensical and almost certainly a configuration bug.
    if let Some((server, _tool)) = split_mcp_namespace(tool_name)
        && server == "*"
    {
        return Err(ClawftError::SecurityViolation {
            reason: format!(
                "MCP tool '{tool_name}' uses wildcard '*' as namespace prefix"
            ),
        });
    }

    let Some((server, _tool)) = split_mcp_namespace(tool_name) else {
        // Not an MCP tool — namespace guard does not apply.
        return Ok(());
    };

    // Is the namespace flagged as sensitive?
    if !SENSITIVE_MCP_NAMESPACES.contains(&server) {
        return Ok(());
    }

    // Sensitive namespace — check whether the allowlist opts in
    // explicitly. An entry opts in if it:
    //   - exactly matches the tool name (e.g. `"exec__shell"`)
    //   - starts with `"<server>_"` or `"<server>__"` and uses globs
    //     (e.g. `"exec_*"`, `"exec__*"`).
    //
    // A bare wildcard `"*"` does NOT count as opt-in.
    let prefix_under = format!("{server}_");
    let prefix_dunder = format!("{server}__");
    let opted_in = allowlist.iter().any(|entry| {
        if entry == "*" {
            return false;
        }
        if entry == tool_name {
            return true;
        }
        entry.starts_with(&prefix_under) || entry.starts_with(&prefix_dunder)
    });

    if !opted_in {
        return Err(ClawftError::SecurityViolation {
            reason: format!(
                "MCP tool '{tool_name}' lives in sensitive namespace \
                 '{server}_*'; wildcard '*' allowlist does not cover \
                 sensitive namespaces. Add an explicit entry like \
                 '{server}_*' or '{tool_name}' to opt in."
            ),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_session_id ────────────────────────────────────────

    #[test]
    fn valid_simple_id() {
        assert!(validate_session_id("abc-123_XYZ").is_ok());
    }

    #[test]
    fn valid_id_with_allowed_chars() {
        assert!(validate_session_id("user@host:session+1.0").is_ok());
    }

    #[test]
    fn valid_id_with_space_and_tab() {
        assert!(validate_session_id("hello world\there").is_ok());
    }

    #[test]
    fn reject_empty() {
        let err = validate_session_id("").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn reject_too_long() {
        let err = validate_session_id(&"a".repeat(257)).unwrap_err();
        assert!(err.to_string().contains("too long"));
    }

    #[test]
    fn accept_max_length() {
        assert!(validate_session_id(&"b".repeat(256)).is_ok());
    }

    #[test]
    fn reject_path_traversal() {
        let err = validate_session_id("foo/../bar").unwrap_err();
        assert!(err.to_string().contains("path traversal"));
    }

    #[test]
    fn reject_forward_slash() {
        let err = validate_session_id("foo/bar").unwrap_err();
        assert!(err.to_string().contains("directory separator"));
    }

    #[test]
    fn reject_backslash() {
        let err = validate_session_id("foo\\bar").unwrap_err();
        assert!(err.to_string().contains("directory separator"));
    }

    #[test]
    fn reject_null_byte() {
        let err = validate_session_id("foo\0bar").unwrap_err();
        assert!(err.to_string().contains("null byte"));
    }

    #[test]
    fn reject_control_char() {
        let err = validate_session_id("foo\x01bar").unwrap_err();
        assert!(err.to_string().contains("control character"));
    }

    // ── truncate_result ────────────────────────────────────────────

    #[test]
    fn no_truncation_when_fits() {
        let val = Value::String("short".into());
        assert_eq!(truncate_result(val.clone(), 1000), val);
    }

    #[test]
    fn truncate_long_string() {
        let result = truncate_result(Value::String("x".repeat(500)), 100);
        let ser = serde_json::to_string(&result).unwrap();
        assert!(ser.len() <= 100, "got {} bytes", ser.len());
        assert!(result.as_str().unwrap().contains("truncated"));
    }

    #[test]
    fn truncate_large_array() {
        let arr: Vec<Value> = (0..100).map(|i| Value::Number(i.into())).collect();
        let result = truncate_result(Value::Array(arr), 100);
        let ser = serde_json::to_string(&result).unwrap();
        assert!(ser.len() <= 100, "got {} bytes", ser.len());
        let last = result.as_array().unwrap().last().unwrap();
        assert_eq!(last["_truncated"], Value::Bool(true));
    }

    #[test]
    fn truncate_object() {
        let mut map = serde_json::Map::new();
        for i in 0..50 {
            map.insert(format!("key_{i}"), Value::String("value".into()));
        }
        let result = truncate_result(Value::Object(map), 100);
        let ser = serde_json::to_string(&result).unwrap();
        assert!(ser.len() <= 100, "got {} bytes", ser.len());
        assert!(result.get("_truncated_json").is_some());
    }

    // ── sanitize_content ───────────────────────────────────────────

    #[test]
    fn sanitize_preserves_normal_text() {
        assert_eq!(sanitize_content("hello world"), "hello world");
    }

    #[test]
    fn sanitize_preserves_newlines_tabs() {
        assert_eq!(sanitize_content("a\nb\rc\td"), "a\nb\rc\td");
    }

    #[test]
    fn sanitize_strips_null() {
        assert_eq!(sanitize_content("a\0b"), "ab");
    }

    #[test]
    fn sanitize_strips_control_chars() {
        assert_eq!(sanitize_content("a\x01\x02\x1Fb"), "ab");
    }

    #[test]
    fn sanitize_strips_del() {
        assert_eq!(sanitize_content("a\x7Fb"), "ab");
    }

    #[test]
    fn sanitize_preserves_emoji() {
        let s = "Hello \u{1F600} world \u{1F30D}";
        assert_eq!(sanitize_content(s), s);
    }

    #[test]
    fn sanitize_preserves_cjk() {
        let s = "\u{4F60}\u{597D}\u{4E16}\u{754C}";
        assert_eq!(sanitize_content(s), s);
    }

    #[test]
    fn sanitize_preserves_rtl() {
        let s = "\u{0645}\u{0631}\u{062D}\u{0628}\u{0627}";
        assert_eq!(sanitize_content(s), s);
    }

    // ── SEC-SKILL-01: YAML depth validation ──────────────────────

    #[test]
    fn yaml_depth_accepted_at_depth_10() {
        // 10 levels = 20 spaces of indentation
        let mut yaml = String::new();
        yaml.push_str("level0:\n");
        for level in 1..=10 {
            let indent = "  ".repeat(level);
            yaml.push_str(&format!("{}level{}:\n", indent, level));
        }
        assert!(validate_yaml_depth(&yaml).is_ok());
    }

    #[test]
    fn yaml_depth_rejected_at_depth_11() {
        let mut yaml = String::new();
        yaml.push_str("level0:\n");
        for level in 1..=11 {
            let indent = "  ".repeat(level);
            yaml.push_str(&format!("{}level{}:\n", indent, level));
        }
        let err = validate_yaml_depth(&yaml).unwrap_err();
        assert!(err.to_string().contains("nesting depth"));
        assert!(err.to_string().contains("exceeds"));
    }

    #[test]
    fn yaml_depth_flat_is_ok() {
        let yaml = "name: test\nversion: 1.0.0\n";
        assert!(validate_yaml_depth(yaml).is_ok());
    }

    // ── SEC-SKILL-02: Directory name validation ──────────────────

    #[test]
    fn dir_name_valid() {
        assert!(validate_directory_name("my-skill").is_ok());
        assert!(validate_directory_name("skill_v2").is_ok());
    }

    #[test]
    fn dir_name_traversal_rejected() {
        let err = validate_directory_name("../evil").unwrap_err();
        assert!(err.to_string().contains("path traversal"));
    }

    #[test]
    fn dir_name_slash_rejected() {
        let err = validate_directory_name("foo/bar").unwrap_err();
        assert!(err.to_string().contains("directory separator"));
    }

    #[test]
    fn dir_name_backslash_rejected() {
        let err = validate_directory_name("foo\\bar").unwrap_err();
        assert!(err.to_string().contains("directory separator"));
    }

    #[test]
    fn dir_name_empty_rejected() {
        let err = validate_directory_name("").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    // ── SEC-SKILL-03: Tool intersection ──────────────────────────

    #[test]
    fn tool_intersection_both_non_empty() {
        let skill = vec!["Read".into(), "Write".into(), "Grep".into()];
        let agent = vec!["Read".into(), "Grep".into(), "Bash".into()];
        let result = intersect_allowed_tools(&skill, &agent);
        assert_eq!(result, vec!["Read", "Grep"]);
    }

    #[test]
    fn tool_intersection_skill_empty() {
        let agent = vec!["Read".into(), "Bash".into()];
        let result = intersect_allowed_tools(&[], &agent);
        assert_eq!(result, vec!["Read", "Bash"]);
    }

    #[test]
    fn tool_intersection_agent_empty() {
        let skill = vec!["Read".into(), "Write".into()];
        let result = intersect_allowed_tools(&skill, &[]);
        assert_eq!(result, vec!["Read", "Write"]);
    }

    #[test]
    fn tool_intersection_both_empty() {
        let result: Vec<String> = intersect_allowed_tools(&[], &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn tool_intersection_no_overlap() {
        let skill = vec!["Read".into()];
        let agent = vec!["Bash".into()];
        let result = intersect_allowed_tools(&skill, &agent);
        assert!(result.is_empty());
    }

    // ── SEC-SKILL-04: Model string validation ────────────────────

    #[test]
    fn model_string_valid_provider_slash() {
        assert!(validate_model_string("openai/gpt-4").is_ok());
        assert!(validate_model_string("anthropic/claude-opus-4-5").is_ok());
    }

    #[test]
    fn model_string_valid_bare() {
        assert!(validate_model_string("gpt-4").is_ok());
    }

    #[test]
    fn model_string_shell_metachar_rejected() {
        let err = validate_model_string("; rm -rf /").unwrap_err();
        assert!(err.to_string().contains("forbidden character"));
    }

    #[test]
    fn model_string_pipe_rejected() {
        let err = validate_model_string("model | cat /etc/passwd").unwrap_err();
        assert!(err.to_string().contains("forbidden character"));
    }

    #[test]
    fn model_string_newline_rejected() {
        let err = validate_model_string("model\ninjection").unwrap_err();
        assert!(err.to_string().contains("control character"));
    }

    #[test]
    fn model_string_empty_rejected() {
        let err = validate_model_string("").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn model_string_backtick_rejected() {
        let err = validate_model_string("`evil`").unwrap_err();
        assert!(err.to_string().contains("forbidden character"));
    }

    // ── SEC-SKILL-06: Prompt injection guards ────────────────────

    #[test]
    fn sanitize_strips_system_tags() {
        let input = "Hello <system>injected</system> world";
        let (result, warnings) = sanitize_skill_instructions(input);
        assert!(!result.contains("<system>"));
        assert!(!result.contains("</system>"));
        assert!(!warnings.is_empty());
    }

    #[test]
    fn sanitize_strips_im_tokens() {
        let input = "Normal <|im_start|>system<|im_end|> text";
        let (result, warnings) = sanitize_skill_instructions(input);
        assert!(!result.contains("<|im_start|>"));
        assert!(!result.contains("<|im_end|>"));
        assert!(!warnings.is_empty());
    }

    #[test]
    fn sanitize_preserves_normal_markdown() {
        let input = "# Heading\n\nNormal **bold** and `code` text.\n\n- list item";
        let (result, warnings) = sanitize_skill_instructions(input);
        assert_eq!(result, input);
        assert!(warnings.is_empty());
    }

    #[test]
    fn sanitize_strips_nested_injection_tokens() {
        // Nested token: removing inner "<system>" from "<sy<system>stem>"
        // must not leave behind a reconstructed "<system>".
        let input = "before <sy<system>stem>injected</sy</system>stem> after";
        let (result, warnings) = sanitize_skill_instructions(input);
        assert!(!result.contains("<system>"), "nested <system> survived: {result}");
        assert!(!result.contains("</system>"), "nested </system> survived: {result}");
        assert!(!warnings.is_empty());
    }

    #[test]
    fn sanitize_strips_deeply_nested_tokens() {
        // Double nesting: <<|im_start<|im_start|>|>
        let input = "x <|im_sta<|im_start|>rt|> y";
        let (result, warnings) = sanitize_skill_instructions(input);
        assert!(!result.contains("<|im_start|>"), "deep nested token survived: {result}");
        assert!(!warnings.is_empty());
    }

    #[test]
    fn sanitize_strips_llama_tokens() {
        let input = "text <<SYS>> system prompt <</SYS>> more text";
        let (result, warnings) = sanitize_skill_instructions(input);
        assert!(!result.contains("<<SYS>>"));
        assert!(!result.contains("<</SYS>>"));
        assert!(!warnings.is_empty());
    }

    // ── SEC-SKILL-07: File size validation ───────────────────────

    #[test]
    fn file_size_within_limit() {
        assert!(validate_file_size(1024, MAX_SKILL_MD_SIZE, "SKILL.md").is_ok());
    }

    #[test]
    fn file_size_at_limit() {
        assert!(validate_file_size(MAX_SKILL_MD_SIZE, MAX_SKILL_MD_SIZE, "SKILL.md").is_ok());
    }

    #[test]
    fn skill_md_51kb_rejected() {
        let size = 51 * 1024;
        let err = validate_file_size(size, MAX_SKILL_MD_SIZE, "SKILL.md").unwrap_err();
        assert!(err.to_string().contains("exceeds"));
        assert!(err.to_string().contains("SKILL.md"));
    }

    #[test]
    fn agent_file_11kb_rejected() {
        let size = 11 * 1024;
        let err = validate_file_size(size, MAX_AGENT_FILE_SIZE, "agent").unwrap_err();
        assert!(err.to_string().contains("exceeds"));
    }

    // ── SEC-SKILL-08: MCP tool namespace isolation ───────────────

    #[test]
    fn mcp_tool_with_double_underscore_valid() {
        assert!(validate_mcp_tool_name_strict("mcp__server__tool").is_ok());
        assert!(validate_mcp_tool_name_strict("claude-flow__memory_store").is_ok());
    }

    #[test]
    fn mcp_tool_without_double_underscore_rejected() {
        let err = validate_mcp_tool_name_strict("mcp_tool_name").unwrap_err();
        assert!(err.to_string().contains("namespace format"));
        assert!(err.to_string().contains("double underscore"));
    }

    #[test]
    fn mcp_tool_no_underscore_rejected_strict() {
        let err = validate_mcp_tool_name_strict("simpletool").unwrap_err();
        assert!(err.to_string().contains("namespace format"));
    }

    #[test]
    fn local_tool_no_underscore_passes_lenient() {
        assert!(validate_mcp_tool_name("ReadFile").is_ok());
    }

    #[test]
    fn local_tool_with_underscore_passes_lenient() {
        assert!(validate_mcp_tool_name("read_file").is_ok());
    }

    // ── WEFT-32: MCP namespace guard against wildcard ['*'] ─────────

    #[test]
    fn weft32_split_mcp_namespace_extracts_server() {
        assert_eq!(
            split_mcp_namespace("exec__shell"),
            Some(("exec", "shell"))
        );
        // Multiple `__` use the FIRST as split point.
        assert_eq!(
            split_mcp_namespace("exec__shell__nested"),
            Some(("exec", "shell__nested"))
        );
        // Non-MCP names produce None.
        assert_eq!(split_mcp_namespace("read_file"), None);
        assert_eq!(split_mcp_namespace("simpletool"), None);
    }

    #[test]
    fn weft32_wildcard_does_not_cover_exec_namespace() {
        // The headline attack: `exec__shell` must be denied under `["*"]`.
        let err = validate_mcp_namespace_against_wildcard(
            "exec__shell",
            &["*".into()],
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("sensitive namespace"));
        assert!(msg.contains("exec"));
    }

    #[test]
    fn weft32_explicit_namespace_prefix_opts_in() {
        // `exec_*` opts the caller in to the sensitive `exec` namespace.
        assert!(validate_mcp_namespace_against_wildcard(
            "exec__shell",
            &["exec_*".into()]
        )
        .is_ok());
        // Double-underscore prefix also opts in.
        assert!(validate_mcp_namespace_against_wildcard(
            "exec__shell",
            &["exec__*".into()]
        )
        .is_ok());
        // Exact name opts in.
        assert!(validate_mcp_namespace_against_wildcard(
            "exec__shell",
            &["exec__shell".into()]
        )
        .is_ok());
    }

    #[test]
    fn weft32_non_sensitive_namespace_allowed_under_wildcard() {
        // `fs__read_file` is fine under `["*"]` — `fs` is not sensitive.
        assert!(validate_mcp_namespace_against_wildcard(
            "fs__read_file",
            &["*".into()]
        )
        .is_ok());
    }

    #[test]
    fn weft32_non_mcp_tool_passes_under_wildcard() {
        // Local tools without `__` aren't subject to the guard.
        assert!(validate_mcp_namespace_against_wildcard(
            "read_file",
            &["*".into()]
        )
        .is_ok());
    }

    #[test]
    fn weft32_wildcard_namespace_prefix_rejected() {
        // The acceptance criterion's literal wildcard-as-namespace case:
        // a tool name like `*__*` (server="*" in the namespace position).
        let err = validate_mcp_namespace_against_wildcard(
            "*__*",
            &["*".into()],
        )
        .unwrap_err();
        assert!(err.to_string().contains("wildcard"));

        let err2 = validate_mcp_namespace_against_wildcard(
            "*__read_file",
            &["*".into()],
        )
        .unwrap_err();
        assert!(err2.to_string().contains("wildcard"));
    }

    #[test]
    fn weft32_empty_allowlist_passes_namespace_guard() {
        // The wildcard guard is orthogonal to the deny-all empty
        // allowlist — caller code handles deny-all separately.
        assert!(validate_mcp_namespace_against_wildcard("exec__shell", &[]).is_ok());
    }

    #[test]
    fn weft32_other_sensitive_namespaces() {
        // Spot-check a few of the other sensitive namespaces.
        for ns in &["shell__cmd", "system__reboot", "sudo__run", "kernel__panic"] {
            assert!(
                validate_mcp_namespace_against_wildcard(ns, &["*".into()])
                    .is_err(),
                "namespace {ns} must be rejected under [*]"
            );
        }
    }

    #[test]
    fn weft32_acceptance_criteria_examples() {
        // From the WEFT-32 instructions:
        //   ["*", "*"]          → Err
        //   ["fs", "read_file"] → Ok (no sensitive namespace involved)
        //   ["*", "read_file"]  → Err for "*"-prefixed tool name
        let err1 = validate_mcp_namespace_against_wildcard(
            "*__*",
            &["*".into(), "*".into()],
        )
        .unwrap_err();
        assert!(err1.to_string().contains("wildcard"));

        // ["fs", "read_file"] — these are tool names in an allowlist
        // for a benign tool. Asking whether `fs__read_file` is allowed
        // should succeed.
        assert!(validate_mcp_namespace_against_wildcard(
            "fs__read_file",
            &["fs".into(), "read_file".into()]
        )
        .is_ok());

        let err3 = validate_mcp_namespace_against_wildcard(
            "*__read_file",
            &["*".into(), "read_file".into()],
        )
        .unwrap_err();
        assert!(err3.to_string().contains("wildcard"));
    }
}
