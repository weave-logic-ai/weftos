//! Canonical JSON: recursively sorted object keys, compact separators, UTF-8,
//! no trailing newline, integers only.
//!
//! This matches the Cognitum ADR-154 canonical form (`json.dumps(sort_keys=
//! True, separators=(",", ":"), ensure_ascii=False)`) for the value space
//! both sides accept, and is independent of whether `serde_json` was built
//! with `preserve_order`. Floats are refused so that two runtimes can never
//! disagree about number formatting.

use serde_json::Value;

/// Largest integer magnitude accepted (JavaScript safe-integer range).
const MAX_SAFE_INT: i64 = (1 << 53) - 1;
/// Nesting limit.
const MAX_DEPTH: usize = 32;
/// Upper bound on canonical output size.
pub const MAX_CANONICAL_BYTES: usize = 1024 * 1024;

/// Reasons a value has no canonical encoding.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalError {
    /// Non-integer or out-of-range number.
    #[error("canonical JSON refuses non-integer or out-of-range number {0}")]
    Number(String),
    /// Nesting deeper than the limit.
    #[error("canonical JSON nesting exceeds {MAX_DEPTH}")]
    TooDeep,
    /// Output larger than the limit.
    #[error("canonical JSON exceeds {MAX_CANONICAL_BYTES} bytes")]
    TooLarge,
}

/// Encode `value` canonically.
pub fn canonical_json(value: &Value) -> Result<Vec<u8>, CanonicalError> {
    let mut out = Vec::with_capacity(256);
    write_value(value, &mut out, 0)?;
    if out.len() > MAX_CANONICAL_BYTES {
        return Err(CanonicalError::TooLarge);
    }
    Ok(out)
}

fn write_value(value: &Value, out: &mut Vec<u8>, depth: usize) -> Result<(), CanonicalError> {
    if depth > MAX_DEPTH {
        return Err(CanonicalError::TooDeep);
    }
    if out.len() > MAX_CANONICAL_BYTES {
        return Err(CanonicalError::TooLarge);
    }
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => {
            let ok = n
                .as_i64()
                .map(|i| (-MAX_SAFE_INT..=MAX_SAFE_INT).contains(&i))
                .or_else(|| n.as_u64().map(|u| u <= MAX_SAFE_INT as u64))
                .unwrap_or(false);
            if !ok {
                return Err(CanonicalError::Number(n.to_string()));
            }
            out.extend_from_slice(n.to_string().as_bytes());
        }
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_value(item, out, depth + 1)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            // Byte order of UTF-8 equals code-point order, which is what
            // Python's sort_keys uses.
            keys.sort();
            out.push(b'{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_string(key, out);
                out.push(b':');
                write_value(&map[key.as_str()], out, depth + 1)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

fn write_string(s: &str, out: &mut Vec<u8>) {
    // serde_json escapes exactly `"`, `\` and control characters, and emits
    // non-ASCII as raw UTF-8, which is the ensure_ascii=False behaviour.
    // Serializing a &str cannot fail.
    let encoded = serde_json::to_string(s).unwrap_or_default();
    out.extend_from_slice(encoded.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorts_keys_recursively_and_is_compact() {
        let v = json!({"b": 1, "a": {"z": [true, null], "c": "x"}});
        let out = canonical_json(&v).unwrap();
        assert_eq!(out, br#"{"a":{"c":"x","z":[true,null]},"b":1}"#);
    }

    #[test]
    fn refuses_floats_and_unsafe_integers() {
        assert!(matches!(
            canonical_json(&json!({"x": 1.5})),
            Err(CanonicalError::Number(_))
        ));
        assert!(canonical_json(&json!(9_007_199_254_740_992_u64)).is_err());
        assert!(canonical_json(&json!(9_007_199_254_740_991_u64)).is_ok());
    }

    #[test]
    fn escapes_like_python_ensure_ascii_false() {
        let v = json!({"k": "q\"b\\s\n\u{e9}/"});
        let out = String::from_utf8(canonical_json(&v).unwrap()).unwrap();
        assert_eq!(out, "{\"k\":\"q\\\"b\\\\s\\n\u{e9}/\"}");
    }

    #[test]
    fn refuses_excessive_depth() {
        let mut v = json!(0);
        for _ in 0..40 {
            v = json!([v]);
        }
        assert_eq!(canonical_json(&v), Err(CanonicalError::TooDeep));
    }
}
