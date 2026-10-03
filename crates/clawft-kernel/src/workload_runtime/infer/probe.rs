//! Loopback HTTP probes of model servers: health, listed models, and an
//! optional tiny completion. Never follows redirects, ignores proxy
//! environment, caps response size, and only ever talks to the address the
//! caller built from a validated loopback spec.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::spec::InferFlavor;
use crate::workload_runtime::types::RuntimeError;

/// Largest response body read from a server.
const MAX_BODY: usize = 1024 * 1024;

/// What a server reported.
#[derive(Debug, Clone, PartialEq)]
pub enum Health {
    /// Reachable and serving.
    Up,
    /// Reachable, still loading the model (HTTP 503).
    Loading,
    /// Reachable but unhealthy.
    Unhealthy(String),
    /// Nothing answered.
    Unreachable(String),
}

/// One probe of a server.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerReport {
    /// Health.
    pub health: Health,
    /// Model ids the server lists (`/v1/models`, or Ollama `/api/tags`).
    pub models: Vec<String>,
    /// Models resident in memory (Ollama `/api/ps`); empty elsewhere.
    pub loaded: Vec<String>,
    /// Server version when it says (Ollama).
    pub version: Option<String>,
    /// Round trip of the health request.
    pub latency: Duration,
}

impl ServerReport {
    fn down(why: String, latency: Duration) -> Self {
        Self {
            health: Health::Unreachable(why),
            models: Vec::new(),
            loaded: Vec::new(),
            version: None,
            latency,
        }
    }

    /// Whether `model` is listed (exact id, or the id's `:latest`-less form).
    pub fn lists(&self, model: &str) -> bool {
        self.models.iter().any(|m| same_model(m, model))
    }

    /// Whether `model` is resident in memory.
    pub fn resident(&self, model: &str) -> bool {
        self.loaded.iter().any(|m| same_model(m, model))
    }
}

/// Servers name models differently: Ollama adds `:latest` to an untagged
/// pull, llama-server lists the GGUF path, mlx-lm the HF repo id. A listed
/// id matches when it is the wanted name, or its last path segment (minus
/// `.gguf`) contains it.
fn same_model(listed: &str, wanted: &str) -> bool {
    if listed == wanted
        || listed.strip_suffix(":latest") == Some(wanted)
        || wanted.strip_suffix(":latest") == Some(listed)
    {
        return true;
    }
    let base = listed.rsplit('/').next().unwrap_or(listed);
    let base = base.strip_suffix(".gguf").unwrap_or(base);
    base.to_ascii_lowercase()
        .contains(&wanted.to_ascii_lowercase())
}

/// A client bound to one server.
#[derive(Clone)]
pub struct ServerClient {
    http: reqwest::Client,
    base: String,
}

impl ServerClient {
    /// Client for `base` (`http://127.0.0.1:PORT`) with a per-request timeout.
    pub fn new(base: String, timeout: Duration) -> Result<Self, RuntimeError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            // A probe must see the server as it is now, not a pooled socket.
            .pool_max_idle_per_host(0)
            .connect_timeout(timeout)
            .timeout(timeout)
            .build()
            .map_err(|e| RuntimeError::Backend(format!("http client: {e}")))?;
        Ok(Self { http, base })
    }

    /// Base URL.
    pub fn base(&self) -> &str {
        &self.base
    }

    async fn read(resp: reqwest::Response) -> Result<(u16, String), String> {
        let status = resp.status().as_u16();
        let mut resp = resp;
        let mut buf = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            buf.extend_from_slice(&chunk);
            if buf.len() > MAX_BODY {
                return Err("response too large".into());
            }
        }
        Ok((status, String::from_utf8_lossy(&buf).into_owned()))
    }

    /// GET `path`.
    pub async fn get(&self, path: &str) -> Result<(u16, String), String> {
        let r = self
            .http
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::read(r).await
    }

    /// POST JSON to `path`.
    pub async fn post(&self, path: &str, body: &Value) -> Result<(u16, String), String> {
        let r = self
            .http
            .post(format!("{}{path}", self.base))
            .json(body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::read(r).await
    }

    /// Probe the server the way `flavor` is probed: `/health` then
    /// `/v1/models` for llama.cpp and mlx-lm (a 404 on `/health` falls back
    /// to `/v1/models` alone), `/api/version`, `/api/tags` and `/api/ps`
    /// for Ollama.
    pub async fn probe(&self, flavor: InferFlavor) -> ServerReport {
        match flavor {
            InferFlavor::Ollama => self.probe_ollama().await,
            _ => self.probe_openai().await,
        }
    }

    async fn probe_openai(&self) -> ServerReport {
        let t = Instant::now();
        let health = match self.get("/health").await {
            Ok((200, _)) => Health::Up,
            Ok((503, _)) => Health::Loading,
            Ok((404, _)) => match self.get("/v1/models").await {
                Ok((200, _)) => Health::Up,
                Ok((503, _)) => Health::Loading,
                Ok((c, _)) => Health::Unhealthy(format!("/v1/models returned HTTP {c}")),
                Err(e) => return ServerReport::down(e, t.elapsed()),
            },
            Ok((c, _)) => Health::Unhealthy(format!("/health returned HTTP {c}")),
            Err(e) => return ServerReport::down(e, t.elapsed()),
        };
        let latency = t.elapsed();
        let models = if health == Health::Up {
            match self.get("/v1/models").await {
                Ok((200, body)) => parse_ids(&body, "data", "id"),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        ServerReport {
            health,
            models,
            loaded: Vec::new(),
            version: None,
            latency,
        }
    }

    async fn probe_ollama(&self) -> ServerReport {
        let t = Instant::now();
        let (health, version) = match self.get("/api/version").await {
            Ok((200, body)) => (
                Health::Up,
                serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| v["version"].as_str().map(str::to_string)),
            ),
            Ok((503, _)) => (Health::Loading, None),
            Ok((c, _)) => (
                Health::Unhealthy(format!("/api/version returned HTTP {c}")),
                None,
            ),
            Err(e) => return ServerReport::down(e, t.elapsed()),
        };
        let latency = t.elapsed();
        let (mut models, mut loaded) = (Vec::new(), Vec::new());
        if health == Health::Up {
            if let Ok((200, body)) = self.get("/api/tags").await {
                models = parse_ids(&body, "models", "name");
            }
            if let Ok((200, body)) = self.get("/api/ps").await {
                loaded = parse_ids(&body, "models", "name");
            }
        }
        ServerReport {
            health,
            models,
            loaded,
            version,
            latency,
        }
    }

    /// Ask for one token. Costs real compute, so callers opt in.
    pub async fn tiny_completion(&self, model: &str) -> Result<(), String> {
        let body = json!({
            "model": model,
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 1,
            "stream": false,
        });
        match self.post("/v1/chat/completions", &body).await? {
            (200, _) => Ok(()),
            (c, _) => Err(format!("completion returned HTTP {c}")),
        }
    }
}

fn parse_ids(body: &str, list: &str, key: &str) -> Vec<String> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v[list].as_array().map(|a| {
                a.iter()
                    .filter_map(|m| m[key].as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default()
}
