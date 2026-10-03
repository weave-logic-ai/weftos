//! Ollama is driven through its API: it owns its process and decides when
//! weights are resident, so the adapter asks it to load or unload a model
//! and reports what `/api/ps` says. It never starts, stops or signals the
//! Ollama server itself.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::probe::ServerClient;

/// Progress of the background load request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadState {
    /// The request is in flight.
    Loading,
    /// The request returned.
    Done,
    /// The request failed.
    Failed(String),
}

/// Shared handle on a [`LoadState`].
pub type SharedLoad = Arc<Mutex<LoadState>>;

fn keep_alive_value(k: &str) -> Value {
    k.parse::<i64>()
        .map(Value::from)
        .unwrap_or_else(|_| json!(k))
}

/// Ask Ollama to bring `tag` into memory (an empty prompt generates
/// nothing). Blocks until it answers, so run it on a task.
pub async fn load(client: &ServerClient, tag: &str, keep_alive: &str) -> Result<(), String> {
    let body = json!({"model": tag, "prompt": "", "stream": false,
        "keep_alive": keep_alive_value(keep_alive)});
    match client.post("/api/generate", &body).await? {
        (200, _) => Ok(()),
        (c, b) => Err(format!(
            "ollama load returned HTTP {c}: {}",
            b.chars().take(200).collect::<String>()
        )),
    }
}

/// Whether `tag` is in memory now (`/api/ps`); false when Ollama does not
/// answer, so a load is attempted and reports the failure itself.
pub async fn resident(client: &ServerClient, tag: &str) -> bool {
    let Ok((200, body)) = client.get("/api/ps").await else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<Value>(&body) else {
        return false;
    };
    let same = |n: &str| n == tag || n.strip_suffix(":latest") == Some(tag) || tag.strip_suffix(":latest") == Some(n);
    v.get("models").and_then(Value::as_array).is_some_and(|ms| {
        ms.iter()
            .any(|m| ["name", "model"].iter().any(|k| m.get(k).and_then(Value::as_str).is_some_and(same)))
    })
}

/// Ask Ollama to drop `tag` from memory now (`keep_alive: 0`).
pub async fn unload(client: &ServerClient, tag: &str) -> Result<(), String> {
    let body = json!({"model": tag, "keep_alive": 0});
    match client.post("/api/generate", &body).await? {
        (200, _) => Ok(()),
        (c, _) => Err(format!("ollama unload returned HTTP {c}")),
    }
}
