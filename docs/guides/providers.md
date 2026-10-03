# Providers Guide

## Overview

clawft uses a pluggable provider system for LLM access. All providers use the
OpenAI-compatible chat completions API format (`POST /v1/chat/completions`),
which has become the de facto standard across 19+ hosted and self-hosted LLM
services. The provider layer lives in the `clawft-llm` crate and has no
dependencies on other clawft crates, making it reusable in isolation.

Key design principles:

- **Single protocol**: every provider speaks the same OpenAI-compatible
  request/response format.
- **Prefix-based routing**: model identifiers like `openai/gpt-4o` encode both
  the target provider and the model name.
- **Environment-driven keys**: API keys are resolved from environment variables
  at request time, never persisted in configuration files.
- **Zero boilerplate for built-in providers**: nine providers are pre-configured
  and ready to use with a single environment variable each.


## How Provider Routing Works

When clawft receives a model identifier (e.g. `anthropic/claude-sonnet-4-5-20250514`),
it passes through the `ProviderRouter`, which performs these steps:

```
  User specifies model
         |
         v
  "anthropic/claude-sonnet-4-5-20250514"
         |
         v
  ProviderRouter.route()
         |
         +--> Scan registered prefixes (longest match first)
         |      Match: "anthropic/" -> provider "anthropic"
         |
         +--> Strip prefix
         |      "claude-sonnet-4-5-20250514"
         |
         +--> Return (provider, stripped_model_name)
         |
         v
  provider.complete(ChatRequest { model: "claude-sonnet-4-5-20250514", ... })
         |
         v
  POST https://api.anthropic.com/v1/chat/completions
```

If no prefix matches any registered provider, the router falls back to the
default provider (the first one in the configuration list) and passes the
model string through unchanged.

The router uses longest-prefix-first matching. This means a prefix like
`openai/o1/` will be tried before `openai/`, enabling sub-provider routing
if needed.


## Built-in Providers

`ProviderRouter::with_builtins()` registers seven providers out of the box.
Each requires only its corresponding environment variable to be set.

| Provider | Base URL | API Key Env Var | Default Model | Notes |
|----------|----------|-----------------|---------------|-------|
| `openai` | `https://api.openai.com/v1` | `OPENAI_API_KEY` | `gpt-4o` | Default provider (first in list). |
| `anthropic` | `https://api.anthropic.com/v1` | `ANTHROPIC_API_KEY` | `claude-sonnet-4-5-20250514` | Adds `anthropic-version: 2023-06-01` header automatically. |
| `groq` | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` | `llama-3.1-70b-versatile` | Fast inference on open-source models. |
| `deepseek` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` | `deepseek-chat` | |
| `mistral` | `https://api.mistral.ai/v1` | `MISTRAL_API_KEY` | `mistral-large-latest` | |
| `together` | `https://api.together.xyz/v1` | `TOGETHER_API_KEY` | (none) | Open-source model hosting. Specify model explicitly. |
| `openrouter` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | (none) | Multi-provider gateway. Specify model explicitly. |
| `gemini` | `https://generativelanguage.googleapis.com/v1beta/openai` | `GOOGLE_GEMINI_API_KEY` | `gemini-2.5-flash` | Google Gemini via OpenAI-compatible endpoint. |
| `xai` | `https://api.x.ai/v1` | `XAI_API_KEY` | `grok-3-mini` | xAI Grok models. |

All built-in providers have a model prefix of `<name>/` (e.g. `openai/`,
`anthropic/`, `groq/`). Every prefix ends with a forward slash.


## Configuration

### Minimal Setup

Set an environment variable for any provider you want to use, then reference
the model with its provider prefix:

```sh
export OPENAI_API_KEY="sk-..."
```

No configuration file is needed for built-in providers. The model identifier
in `agents.defaults.model` handles routing:

```json
{
  "agents": {
    "defaults": {
      "model": "openai/gpt-4o"
    }
  }
}
```

### Provider Overrides

The application configuration file supports overrides for five built-in
providers: `openai`, `anthropic`, `groq`, `deepseek`, and `openrouter`.
Each provider section accepts `api_base` and `extra_headers`:

```json
{
  "providers": {
    "anthropic": {
      "api_base": "https://my-proxy.example.com/v1",
      "extra_headers": {
        "X-Org-Id": "my-org-123"
      }
    },
    "openai": {
      "api_base": null
    }
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `api_key` | string | API key for this provider. Prefer environment variables over inline keys. |
| `api_base` | string or null | Overrides the built-in base URL. Set to `null` or omit to keep the default. |
| `extra_headers` | object | Additional HTTP headers merged into every request to this provider. |

When `api_base` is set to a non-empty string, it replaces the provider's
built-in base URL entirely. When `extra_headers` are provided, they are
merged with any headers already defined on the provider (e.g. the Anthropic
version header is preserved).

### Custom Providers

To add a provider not in the built-in list, define it under the `providers`
key in the configuration file:

```json
{
  "providers": {
    "custom_provider": {
      "base_url": "http://localhost:11434/v1",
      "api_key_env": "OLLAMA_API_KEY",
      "default_model": "llama3"
    }
  }
}
```

### Config File Location

The config file is resolved in order:

| Priority | Source |
|----------|--------|
| 1 | `CLAWFT_CONFIG` environment variable (absolute path) |
| 2 | `~/.clawft/config.json` |
| 3 | `~/.nanobot/config.json` (legacy fallback) |

See the [Configuration Guide](configuration.md) for the full config reference.


## Model Identifier Format

Model identifiers use the format `provider/model-name`. The slash separates
the routing prefix from the model name sent to the API.

Examples:

| Identifier | Provider | Model sent to API |
|------------|----------|-------------------|
| `openai/gpt-4o` | openai | `gpt-4o` |
| `anthropic/claude-sonnet-4-5-20250514` | anthropic | `claude-sonnet-4-5-20250514` |
| `groq/llama-3.1-70b-versatile` | groq | `llama-3.1-70b-versatile` |
| `deepseek/deepseek-chat` | deepseek | `deepseek-chat` |
| `mistral/mistral-large-latest` | mistral | `mistral-large-latest` |
| `together/meta-llama/Meta-Llama-3-70B` | together | `meta-llama/Meta-Llama-3-70B` |
| `openrouter/meta/llama-3-70b` | openrouter | `meta/llama-3-70b` |
| `gpt-4o` | (default) | `gpt-4o` |

Note that only the first slash is used as the separator. This means models
with slashes in their names (common on Together and OpenRouter) work
correctly: `openrouter/meta/llama-3-70b` routes to the `openrouter` provider
with model `meta/llama-3-70b`.

When no prefix is present, the identifier is passed unchanged to the default
provider (the first provider in the configuration list -- `openai` for built-in
configurations).

### Programmatic Access

The `ProviderRouter` exposes a static helper for prefix splitting:

```rust
let (prefix, model) = ProviderRouter::strip_prefix("openai/gpt-4o");
// prefix = Some("openai"), model = "gpt-4o"

let (prefix, model) = ProviderRouter::strip_prefix("gpt-4o");
// prefix = None, model = "gpt-4o"
```


## Error Handling

All provider operations return `Result<T, ProviderError>`. The error type
maps directly to common HTTP failure modes:

```rust
pub enum ProviderError {
    /// HTTP request failed (network error, DNS failure, etc.).
    RequestFailed(String),

    /// Authentication rejected (HTTP 401 or 403).
    AuthFailed(String),

    /// Rate limited (HTTP 429). Includes a suggested retry delay.
    RateLimited { retry_after_ms: u64 },

    /// Model not found on the provider (HTTP 404).
    ModelNotFound(String),

    /// Provider not configured (e.g. missing API key env var).
    NotConfigured(String),

    /// Response could not be parsed as valid ChatResponse JSON.
    InvalidResponse(String),

    /// Request exceeded the timeout duration.
    Timeout,

    /// Low-level HTTP error (from reqwest).
    Http(reqwest::Error),

    /// JSON serialization/deserialization error.
    Json(serde_json::Error),
}
```

Error mapping by HTTP status code:

| Status | Error Variant | Typical Cause |
|--------|---------------|---------------|
| 401, 403 | `AuthFailed` | Invalid, expired, or missing API key. |
| 404 | `ModelNotFound` | Model name does not exist on the provider. |
| 429 | `RateLimited` | Too many requests. The `retry_after_ms` field is parsed from the response body when available, defaulting to 1000 ms. |
| Other 4xx/5xx | `RequestFailed` | Includes the status code and response body in the message. |

Before any HTTP call is made, the provider resolves the API key from the
environment. If the environment variable is not set, a `NotConfigured` error
is returned immediately, without making a network request.


## Automatic Retry Behavior

Every provider is wrapped in a `RetryPolicy` that automatically retries
transient failures with exponential backoff. The default configuration is:

| Parameter | Default | Description |
|-----------|---------|-------------|
| `max_retries` | 3 | Maximum number of retry attempts after the initial call. |
| `base_delay` | 1 second | Initial delay before the first retry. |
| `max_delay` | 30 seconds | Upper bound on the delay between retries. |
| `jitter_fraction` | 0.25 | Random jitter added as a fraction of the computed delay. |

The delay for attempt `n` is `min(base_delay * 2^n, max_delay)` plus
random jitter of `0..jitter_fraction * delay`.

The following errors are considered retryable:

- `RateLimited` (HTTP 429) -- except for permanent quota/credit exhaustion
- `ServerError` (HTTP 500-599)
- `Timeout` -- request exceeded the configured timeout
- `Http` -- network-level failures (connection refused, DNS, etc.)

Non-retryable errors (authentication, model not found, invalid response,
permanent billing issues) fail immediately without retry.

For rate-limited responses, the provider parses the `Retry-After` HTTP
header (numeric seconds) or `retry_after_ms` / `retry_after` fields from
the JSON error body. If neither is present, a default of 1000 ms is used.

Some providers (e.g. xAI, OpenAI) return HTTP 429 for permanent credit
or spending limit exhaustion. These are detected by keyword matching
(e.g. "exhausted", "billing", "insufficient_quota") and are **not**
retried -- they produce a `RequestFailed` error immediately.


## Using Local LLMs

Any local server that exposes an OpenAI-compatible `/v1/chat/completions`
endpoint works as a clawft provider. Ollama and llama.cpp are the most
common choices.

### Ollama

1. Install Ollama and pull a model:

   ```sh
   curl -fsSL https://ollama.com/install.sh | sh
   ollama pull llama3
   ```

2. Ollama runs on `http://localhost:11434` by default and exposes an
   OpenAI-compatible endpoint at `/v1/chat/completions`.

3. Configure clawft to use it. You can override the OpenAI provider's base
   URL, or add a separate custom provider:

   **Option A -- Override OpenAI base URL:**

   ```json
   {
     "providers": {
       "openai": {
         "api_base": "http://localhost:11434/v1"
       }
     },
     "agents": {
       "defaults": {
         "model": "openai/llama3"
       }
     }
   }
   ```

   Set a dummy key since Ollama does not require authentication:

   ```sh
   export OPENAI_API_KEY="not-needed"
   ```

   **Option B -- Programmatic custom provider:**

   ```rust
   use clawft_llm::{ProviderConfig, ProviderRouter};
   use std::collections::HashMap;

   let configs = vec![ProviderConfig {
       name: "ollama".into(),
       base_url: "http://localhost:11434/v1".into(),
       api_key_env: "OLLAMA_API_KEY".into(),
       model_prefix: Some("ollama/".into()),
       default_model: Some("llama3".into()),
       headers: HashMap::new(),
   }];

   let router = ProviderRouter::from_configs(configs);
   let (provider, model) = router.route("ollama/llama3").unwrap();
   ```

### llama.cpp Server

1. Start the llama.cpp server:

   ```sh
   ./llama-server -m model.gguf --port 8080
   ```

2. Point a provider at it:

   ```json
   {
     "providers": {
       "openai": {
         "api_base": "http://localhost:8080/v1"
       }
     }
   }
   ```

### vLLM

vLLM natively serves an OpenAI-compatible API:

```sh
python -m vllm.entrypoints.openai.api_server --model meta-llama/Llama-3-8B
```

Point the provider's `api_base` to `http://localhost:8000/v1`.


## Adding Custom Providers

### Using ProviderConfig

For any service that speaks the OpenAI chat completions protocol, create a
`ProviderConfig` and include it in the router:

```rust
use clawft_llm::{ProviderConfig, ProviderRouter};
use std::collections::HashMap;

let mut configs = clawft_llm::config::builtin_providers();

configs.push(ProviderConfig {
    name: "my-service".into(),
    base_url: "https://my-llm-service.example.com/v1".into(),
    api_key_env: "MY_SERVICE_API_KEY".into(),
    model_prefix: Some("my-service/".into()),
    default_model: Some("my-model-v2".into()),
    headers: HashMap::from([
        ("X-Org-Id".into(), "org-123".into()),
    ]),
});

let router = ProviderRouter::from_configs(configs);
```

### ProviderConfig Fields

```rust
pub struct ProviderConfig {
    /// Human-readable provider name (e.g. "openai", "my-service").
    pub name: String,

    /// Base URL for the API. The /chat/completions path is appended automatically.
    pub base_url: String,

    /// Environment variable name that holds the API key.
    pub api_key_env: String,

    /// Prefix for model routing (e.g. "my-service/"). Must end with "/".
    pub model_prefix: Option<String>,

    /// Default model when none is specified in the request.
    pub default_model: Option<String>,

    /// Extra HTTP headers included in every request (e.g. version headers).
    pub headers: HashMap<String, String>,
}
```

### Implementing the Provider Trait

For services that do not follow the OpenAI request format, implement the
`Provider` trait directly:

```rust
use async_trait::async_trait;
use clawft_llm::{Provider, ChatRequest, ChatResponse, Result};

pub struct MyCustomProvider {
    // your fields
}

#[async_trait]
impl Provider for MyCustomProvider {
    fn name(&self) -> &str {
        "my-custom"
    }

    async fn complete(&self, request: &ChatRequest) -> Result<ChatResponse> {
        // Transform the ChatRequest into your provider's native format,
        // make the HTTP call, and transform the response back into
        // a ChatResponse.
        todo!()
    }
}
```

The `ChatRequest` and `ChatResponse` types follow the OpenAI schema:

- `ChatRequest` contains `model`, `messages`, optional `max_tokens`,
  `temperature`, `tools`, and `stream`.
- `ChatResponse` contains `id`, `choices` (each with a `message` and
  `finish_reason`), optional `usage` statistics, and the `model` name.

A direct `Provider` implementation can be used alongside `OpenAiCompatProvider`
instances by wrapping both in `Box<dyn Provider>` and registering them with a
`ProviderRouter` (or using them directly without routing).


## Placed local inference

By default `local/` goes to the address in `[kernel.llm]` or the ADR-060
default (`127.0.0.1:8090`). The daemon can instead keep that address stable
while the model server moves, and let other nodes on the mesh use it. This is
off unless `<runtime>/inference.json` exists.

```json
{ "roles": [ { "role": "hermes", "flavor": "llamacpp",
               "instance_port": 18090, "proxy_port": 8090,
               "on_occupied": "refuse", "provider": "local" } ],
  "mesh": { "expose": ["hermes"],
            "serve_peers":  { "hermes": ["<node id>"] },
            "remote_nodes": { "hermes": ["<node id>"] } } }
```

- A role watches a model server that somebody else runs on
  `127.0.0.1:<instance_port>` (`llamacpp`, `mlx-lm` or `ollama`). The daemon
  only reads from it; it never starts, stops or signals it. A server that is
  not up yet is picked up when it appears.
- `proxy_port` makes the daemon listen on that loopback port and forward
  OpenAI-compatible requests to wherever the role is served, so existing
  clients keep their address. The proxy forwards only GET and POST on the
  inference paths, to the instance the placement table names, never to a host
  the request names. If something already answers on `proxy_port` it is not
  bound over: `refuse` (default) fails, `adopt` leaves the existing server as
  the address.
- `provider: "local"` makes in-process agents resolve `local/` through the
  role. Precedence is unchanged: `LLM_SERVICE_URL`, then `[kernel.llm]` and
  `[providers.local]`, then placement, then the ADR-060 constants. Any
  explicit setting wins, and if placement has no answer the configured
  address is used.
- Mesh use is deny by default. `expose` lets a role leave loopback;
  `serve_peers` names the peers this node serves it to; `remote_nodes` names
  the nodes this node will use. A peer is served only when admission is
  enforced, it is a full node (not a leaf) and its id was verified on the
  connection. Peers are sent only the OpenAI inference paths, with `model`
  pinned to the served model and everything outside a body-key allowlist
  dropped. The local client's `Authorization` header goes to the local server
  only, never over the mesh; adopting a server is the decision to trust it
  with the credentials local clients send to that address.
- `infer.status` (read) shows roles, proxy state, where each role resolves
  and why the mesh is or is not available. `infer.expose` and `infer.allow`
  (admin) change exposure and the allowlists until restart; each change is
  recorded on the chain. The file holds the persistent settings. Optional
  `sync_secs` (1 to 60, default 5) and `advert_secs` (1 to 30, default 20)
  tune the local re-check and the mesh announcement (`sync_secs` may not
  exceed `advert_secs`, since the announcement rides the sync tick, and the real period,
  `advert_secs` rounded up to a multiple of `sync_secs`, must stay within 30); adverts expire after 60 s
  without a refresh.
- In service mode (ADR-103) the machine mesh service owns the mesh and hands
  the daemon deliveries as an unverified peer, so local placement and the
  proxy work but serving or using a remote node does not; the status says so.

## API Key Management

API keys are resolved at request time, not at configuration time. The
resolution order is:

1. **Explicit key** -- if `OpenAiCompatProvider::with_api_key()` was used,
   that key is returned immediately.
2. **Environment variable** -- the value of the environment variable named
   in `ProviderConfig.api_key_env` is read via `std::env::var()`.

If neither source provides a key, a `ProviderError::NotConfigured` error is
returned before any network request is made.

Best practices:

- **Never commit API keys** to configuration files or source control.
- **Use environment variables** for all keys. Set them in your shell profile,
  `.env` file (excluded from version control), or a secrets manager.
- **Rotate keys regularly** and use scoped/restricted keys where the provider
  supports them.
- The `Debug` implementation for `OpenAiCompatProvider` redacts the API key,
  displaying `***` instead of the actual value.

Example `.env` file (add to `.gitignore`):

```sh
OPENAI_API_KEY=sk-...
ANTHROPIC_API_KEY=sk-ant-...
GROQ_API_KEY=gsk_...
```


## Troubleshooting

### "provider not configured: set OPENAI_API_KEY env var"

The environment variable for the target provider is not set. Export it in
your shell or add it to your `.env` file. The error message names the
specific variable that is missing.

### "authentication failed" (401/403)

The API key is set but the provider rejected it. Verify the key is correct,
not expired, and has sufficient permissions. For Anthropic, ensure the
`anthropic-version` header matches a supported API version (the built-in
configuration sets `2023-06-01`).

### "model not found" (404)

The model name does not exist on the target provider. Check for typos in
the model identifier. Remember that the provider prefix is stripped before
the request: `openai/gpt-4o` sends `gpt-4o` to the OpenAI API.

### "rate limited: retry after Nms"

The provider returned HTTP 429. Wait for the indicated duration before
retrying. If this happens frequently, consider using a different provider
or requesting a rate limit increase.

### Requests go to the wrong provider

Model identifiers are matched by longest prefix first. If your model string
does not start with a registered prefix, it falls through to the default
provider. Verify the prefix with:

```rust
let (prefix, model) = ProviderRouter::strip_prefix("your/model-string");
println!("prefix: {:?}, model: {}", prefix, model);
```

### Local LLM returns parse errors

Ensure the local server returns responses in the exact OpenAI chat completion
JSON format. The `ChatResponse` struct requires at minimum: `id` (string),
`choices` (array with `index`, `message`, and `finish_reason`), `usage`
(object or null), and `model` (string). Some local servers omit fields or
use different key names.

### Timeout errors

The default HTTP client does not set an explicit timeout. If you need one,
configure the `reqwest::Client` with a timeout when constructing a custom
`OpenAiCompatProvider`. For long-running inference on local hardware, ensure
the server's own timeout is set high enough.

### Verifying provider connectivity

Enable debug logging to see the full request/response cycle:

```sh
RUST_LOG=clawft_llm=debug weft chat
```

This logs the provider name, model, message count, and response metadata
for every chat completion call.
