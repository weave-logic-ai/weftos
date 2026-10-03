//! What a peer may ask of a local model server over the mesh.
//!
//! Narrower than the loopback proxy: peers get the OpenAI inference
//! endpoints only, never Ollama's management or introspection endpoints,
//! and cannot choose the model or how long it stays resident.

use super::table::MeshLocal;
use super::types::ProxyError;

/// Paths served to peers.
pub fn mesh_path_allowed(path: &str) -> bool {
    matches!(
        path.split('?').next().unwrap_or(""),
        "/v1/chat/completions" | "/v1/completions" | "/v1/embeddings" | "/v1/models"
    )
}

/// Keys a peer may send, per path. Everything else is dropped: servers
/// read more from a request body than the OpenAI schema (`mlx_lm.server`
/// loads `adapters` and `draft_model`, Ollama reads `keep_alive` and
/// `options`, llama-server takes `grammar`, `cache_prompt` and slot
/// controls), and a peer must not reach any of that.
const SAMPLING: &[&str] = &[
    "max_tokens",
    "max_completion_tokens",
    "temperature",
    "top_p",
    "top_k",
    "min_p",
    "stream",
    "stream_options",
    "stop",
    "n",
    "seed",
    "presence_penalty",
    "frequency_penalty",
    "repetition_penalty",
    "logit_bias",
    "logprobs",
    "top_logprobs",
    "response_format",
    "user",
];

fn allowed_keys(route: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match route {
        "/v1/chat/completions" => (SAMPLING, &["messages", "tools", "tool_choice", "parallel_tool_calls"]),
        "/v1/completions" => (SAMPLING, &["prompt"]),
        "/v1/embeddings" => (&[], &["input", "encoding_format", "dimensions", "user"]),
        _ => (&[], &[]),
    }
}

/// Pin a forwarded JSON body to the instance this node serves: only the
/// allowlisted keys for the path survive, and `model` is set to what the
/// server knows the instance as (a peer must not make a server load another
/// model: Ollama loads any installed one, and `mlx_lm.server` fetches any
/// repo named in `model`, so MLX gets its `default_model` placeholder). A
/// POST body that is not a JSON object is refused.
pub fn pin_body(path: &str, body: &[u8], local: &MeshLocal) -> Result<Vec<u8>, ProxyError> {
    if body.is_empty() {
        return Ok(Vec::new());
    }
    let bad = || ProxyError::BadRequest("request body must be a JSON object".into());
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| bad())?;
    let src = v.as_object().ok_or_else(bad)?;
    let route = path.split('?').next().unwrap_or("");
    let (common, own) = allowed_keys(route);
    let mut out = serde_json::Map::new();
    for (k, val) in src {
        if common.contains(&k.as_str()) || own.contains(&k.as_str()) {
            out.insert(k.clone(), val.clone());
        }
    }
    let pinned = if local.runtime == "MlxLm" {
        Some("default_model".to_string())
    } else {
        local.model.clone()
    };
    if let Some(m) = pinned {
        out.insert("model".into(), serde_json::Value::String(m));
    }
    serde_json::to_vec(&serde_json::Value::Object(out)).map_err(|_| bad())
}
