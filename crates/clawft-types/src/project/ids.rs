//! ULID generation and validation.

use super::ProjectError;

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Mint a new project id (26-char uppercase Crockford ULID).
pub fn new_id() -> String {
    ulid::Ulid::new().to_string()
}

/// Validate that `id` is a canonical ULID.
///
/// Must be called before any id becomes a path component: the alphabet
/// check rejects `/`, `\`, `.` and every other traversal character.
pub fn validate_id(id: &str) -> Result<(), ProjectError> {
    let b = id.as_bytes();
    let ok = b.len() == 26
        // 128 bits in 26 * 5 = 130: the first char may only carry 3 bits.
        && b[0] <= b'7'
        && b.iter().all(|c| CROCKFORD.contains(c));
    if ok {
        Ok(())
    } else {
        Err(ProjectError::InvalidId(id.to_string()))
    }
}
