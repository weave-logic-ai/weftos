//! WeftOS LLM service — HTTP client to an OpenAI-compatible chat
//! completions endpoint (typically a local `llama-server`).
//!
//! # Why this crate exists
//!
//! The whisper service (sibling crate `clawft-service-whisper`) proved
//! the pattern: an external local model gets a thin HTTP wrapper, the
//! daemon hosts the wrapper as a tokio task, the wrapper exposes a
//! single in-process client object, and the daemon publishes one RPC
//! that delegates to it. This crate is the same pattern for chat
//! completions against a locally-hosted Qwen3 (or any other) model
//! served by `llama-server`'s OpenAI-compat API.
//!
//! # Why not reuse `clawft-llm`?
//!
//! `clawft-llm` is a *general* provider abstraction (OpenAI, Anthropic,
//! local, with routing + failover + retry + SSE). It targets browser +
//! native, has its own router/config story, and brings in
//! `clawft-types`, `eml-core`, `uuid`, and a futures stack. For the
//! daemon's "POST one prompt to a single localhost endpoint and return
//! the completion" use case, that surface is overkill and the
//! dependency edge would couple the daemon-only service to the
//! browser-targeted abstraction.
//!
//! `clawft-service-llm` is intentionally narrow: one client struct,
//! one method (`complete`), one error enum, one in-flight semaphore
//! to match `llama-server`'s single-batch backpressure model. If the
//! chat window later wants streaming, it lands here as a sibling
//! `complete_stream` method, not a reach into the general provider
//! crate.
//!
//! # Crate layout
//!
//! - [`client`] — [`LlmClient`], the HTTP consumer of
//!   `/v1/chat/completions`.
//!
//! The service-side glue (substrate publish, control flag, RPC handler
//! wiring) lives in `clawft-weave`'s `daemon.rs`; this crate has no
//! substrate or kernel knowledge so it tests cleanly against
//! `wiremock`.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

pub mod client;

pub use client::{
    ChatChoice, ChatMessage, ChatRequest, ChatResponse, ChatTimings, ChatUsage,
    ChatUsagePromptDetails, LlmClient, LlmConfig, LlmError, Tool, ToolCall, ToolCallFunction,
    ToolChoice, ToolFunction,
};

/// Environment variable read by [`LlmConfig::from_env`].
pub const LLM_SERVICE_URL_ENV: &str = "LLM_SERVICE_URL";

/// Environment variable that overrides the default model name, e.g.
/// for picking a specific OpenRouter slug. Honoured by the daemon's
/// LLM-client construction in `clawft-weave`.
pub const LLM_MODEL_ENV: &str = "LLM_MODEL";

/// Environment variable carrying an OpenRouter API key. When set,
/// the daemon defaults the LLM service URL to OpenRouter and attaches
/// `Authorization: Bearer <key>` to chat-completion requests.
pub const OPENROUTER_API_KEY_ENV: &str = "OPENROUTER_API_KEY";

/// Default LLM service URL if the env var is unset. Matches the
/// `llama-server` instance the user already runs locally for Qwen3.
pub const DEFAULT_LLM_SERVICE_URL: &str = "http://127.0.0.1:8111";

/// Default OpenRouter API base. The client appends `/v1/chat/completions`
/// (and tolerates a trailing `/v1`), so this points at the API root,
/// not the v1 root.
pub const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api";

/// Default model name. `llama-server` accepts any string and routes to
/// its single loaded model, so the default is purely cosmetic — it
/// shows up in the request body for traceability and is echoed back in
/// the response.
pub const DEFAULT_LLM_MODEL: &str = "local";

/// Default OpenRouter model — Nvidia's free Nemotron 120b. Used when
/// `OPENROUTER_API_KEY` is set and `LLM_MODEL` is unset.
pub const DEFAULT_OPENROUTER_MODEL: &str = "nvidia/nemotron-3-super-120b-a12b:free";
