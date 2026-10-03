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

/// Pin a forwarded JSON body to the instance this node serves: `model` is
/// rewritten to what the server knows the instance as (a peer must not make
/// a server load another model: Ollama loads any installed one, and
/// `mlx_lm.server` fetches any repo named in `model`, so MLX gets its
/// `default_model` placeholder), and `keep_alive` is removed (it would let
/// a peer pin or evict weights). A POST body that is not a JSON object is
/// refused.
pub fn pin_body(body: &[u8], local: &MeshLocal) -> Result<Vec<u8>, ProxyError> {
    if body.is_empty() {
        return Ok(Vec::new());
    }
    let bad = || ProxyError::BadRequest("request body must be a JSON object".into());
    let mut v: serde_json::Value = serde_json::from_slice(body).map_err(|_| bad())?;
    let obj = v.as_object_mut().ok_or_else(bad)?;
    obj.remove("keep_alive");
    let pinned = if local.runtime == "MlxLm" {
        Some("default_model".to_string())
    } else {
        local.model.clone()
    };
    match pinned {
        Some(m) => {
            obj.insert("model".into(), serde_json::Value::String(m));
        }
        None => {
            obj.remove("model");
        }
    }
    serde_json::to_vec(&v).map_err(|_| bad())
}
