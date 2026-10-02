//! The `POST /api/v1/store/ingest` request contract and its validation.
//!
//! Body: `{"vectors":[[id,[8 floats]],...],"dedup":true}`. Anything else is
//! rejected: wrong shape, wrong dimensionality, non-finite floats, an empty
//! or oversize batch, unknown keys.

use serde_json::Value;

/// Dimensionality of an ingested vector (the ADR-069 feature packet).
pub const DIMS: usize = 8;
/// Most vectors in one request.
pub const MAX_VECTORS_PER_BATCH: usize = 256;
/// Largest request body the bridge reads.
pub const MAX_BODY_BYTES: usize = 64 * 1024;
/// Path every cog posts to.
pub const INGEST_PATH: &str = "/api/v1/store/ingest";

/// One vector of a batch.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IngestVector {
    /// Caller-chosen id.
    pub id: u64,
    /// The 8 values.
    pub values: [f32; DIMS],
}

/// A validated batch.
#[derive(Debug, Clone, PartialEq)]
pub struct IngestBatch {
    /// Vectors (1..=[`MAX_VECTORS_PER_BATCH`]).
    pub vectors: Vec<IngestVector>,
    /// Skip vectors the store already holds (same id, or bit-identical values).
    pub dedup: bool,
}

/// Why a request was refused. Maps to an HTTP status.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IngestError {
    /// Malformed request (400).
    #[error("malformed request: {0}")]
    Malformed(String),
    /// Body or batch over a limit (413).
    #[error("too large: {0}")]
    TooLarge(String),
    /// Missing or unknown token (401).
    #[error("unauthorized")]
    Unauthorized,
    /// Token belongs to another instance (403).
    #[error("forbidden")]
    Forbidden,
    /// Over the per-instance rate (429).
    #[error("rate limited")]
    RateLimited,
    /// The store owner could not take the batch (502).
    #[error("store unavailable: {0}")]
    Unavailable(String),
}

impl IngestError {
    /// HTTP status code.
    pub fn status(&self) -> u16 {
        match self {
            Self::Malformed(_) => 400,
            Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::TooLarge(_) => 413,
            Self::RateLimited => 429,
            Self::Unavailable(_) => 502,
        }
    }
}

fn bad(m: impl Into<String>) -> IngestError {
    IngestError::Malformed(m.into())
}

/// Parse and validate a request body.
pub fn parse_batch(body: &[u8]) -> Result<IngestBatch, IngestError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(IngestError::TooLarge(format!(
            "body over {MAX_BODY_BYTES} bytes"
        )));
    }
    let v: Value = serde_json::from_slice(body).map_err(|e| bad(format!("not JSON: {e}")))?;
    let obj = v.as_object().ok_or_else(|| bad("body must be an object"))?;
    if let Some(k) = obj.keys().find(|k| *k != "vectors" && *k != "dedup") {
        return Err(bad(format!("unknown key `{k}`")));
    }
    let dedup = match obj.get("dedup") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("`dedup` must be a boolean")),
    };
    let arr = obj
        .get("vectors")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("`vectors` must be an array"))?;
    if arr.is_empty() {
        return Err(bad("`vectors` is empty"));
    }
    if arr.len() > MAX_VECTORS_PER_BATCH {
        return Err(IngestError::TooLarge(format!(
            "{} vectors, at most {MAX_VECTORS_PER_BATCH}",
            arr.len()
        )));
    }
    let vectors = arr
        .iter()
        .enumerate()
        .map(|(i, e)| parse_vector(e).map_err(|m| bad(format!("vectors[{i}]: {m}"))))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(IngestBatch { vectors, dedup })
}

fn parse_vector(e: &Value) -> Result<IngestVector, String> {
    let pair = e.as_array().filter(|p| p.len() == 2);
    let [id, vals] = pair.map(|p| [&p[0], &p[1]]).ok_or("expected [id, [values]]")?;
    let id = id.as_u64().ok_or("id must be a non-negative integer")?;
    let vals = vals.as_array().ok_or("values must be an array")?;
    if vals.len() != DIMS {
        return Err(format!("{} values, expected {DIMS}", vals.len()));
    }
    let mut values = [0f32; DIMS];
    for (slot, x) in values.iter_mut().zip(vals) {
        let f = x.as_f64().ok_or("values must be numbers")? as f32;
        if !f.is_finite() {
            return Err("values must be finite f32".into());
        }
        *slot = f;
    }
    Ok(IngestVector { id, values })
}

/// True for a project id: 26 upper-case alphanumeric characters (a ULID).
pub fn valid_project_id(s: &str) -> bool {
    s.len() == 26 && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}
