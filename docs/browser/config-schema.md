# Browser Config Schema

Annotated reference for the JSON object passed to clawft-wasm
`init(config_json[, env_json])`.

The schema is the same root [`Config`](../../crates/clawft-types/src/config/mod.rs)
type used by the native CLI. Serde accepts **both** `snake_case` and
`camelCase` field names; unknown fields are ignored. Every section has a
default, so a minimal browser config only needs `providers` + a model.

Browser-specific behavior lives on each provider entry:

| Field | Alias | Browser meaning |
|-------|-------|-----------------|
| `browser_direct` | `browserDirect` | Call the provider origin without a proxy; Anthropic gets the direct-access header |
| `cors_proxy` | `corsProxy` | When set, rewrite requests to `{corsProxy}/{base}/{path}` |
| `api_key` | `apiKey` | **Required** in browser (no process env for keys) |
| `api_base` | `apiBase`, `baseUrl` | Override built-in base URL |
| `extra_headers` | `extraHeaders` | Merged into every request |

See [cors-provider-setup.md](./cors-provider-setup.md) for per-provider CORS
recipes. Native-only sections (`channels`, most of `gateway`, MCP exec, etc.)
may still appear in JSON; they are parsed but largely unused in WASM.

---

## Minimal browser config

```json
{
  "providers": {
    "anthropic": {
      "apiKey": "sk-ant-…",
      "browserDirect": true
    }
  },
  "agents": {
    "defaults": {
      "model": "anthropic/claude-sonnet-4-5-20250514",
      "maxTokens": 4096
    }
  }
}
```

Harness default (from `crates/clawft-wasm/www/index.html`):

```json
{
  "providers": {
    "openrouter": {
      "apiKey": "YOUR_OPENROUTER_KEY"
    },
    "anthropic": {
      "apiKey": "YOUR_ANTHROPIC_KEY",
      "browserDirect": true
    }
  },
  "agents": {
    "defaults": {
      "model": "openrouter/meta-llama/llama-3.1-8b-instruct:free",
      "maxTokens": 4096
    }
  }
}
```

---

## Root object

```json
{
  "agents": { },
  "channels": { },
  "providers": { },
  "gateway": { },
  "tools": { },
  "delegation": { },
  "routing": { },
  "agent_routing": { },
  "voice": { },
  "kernel": { },
  "pipeline": { },
  "plugins": { },
  "skills": { }
}
```

| Key | Browser relevance | Notes |
|-----|-------------------|--------|
| `agents` | **High** | Default model, tokens, workspace path |
| `providers` | **High** | Keys, CORS, base URLs — required for LLM calls |
| `routing` | Medium | Static / tiered routing if configured |
| `tools` | Medium | Web search keys; `exec` / MCP largely N/A in browser |
| `pipeline` | Low | Scorer / learner backend names (`noop` default) |
| `skills` | Low | Discovery settings when skill tools are present |
| `voice` | Low | Voice pipeline; browser voice is a separate UI path |
| `kernel` | Low | Brand / WeftOS kernel knobs |
| `plugins` | Low | Plugin grants (native-oriented) |
| `delegation` | Low | Task delegation rules |
| `agent_routing` | Low | Multi-agent doctor table (WEFT-197) |
| `channels` | None in WASM | Telegram/Slack/Discord — native daemon only |
| `gateway` | None for pure WASM | HTTP server / UI API CORS for the **native** gateway |

---

## `agents`

```json
{
  "agents": {
    "workspaceRoot": null,
    "defaults": {
      "workspace": "~/.nanobot/workspace",
      "model": "anthropic/claude-sonnet-4-5-20250514",
      "maxTokens": 8192,
      "temperature": 0.7,
      "maxToolIterations": 20,
      "memoryWindow": 50
    },
    "costBudget": {
      "maxTokensPerConv": 200000,
      "maxUsdPerConv": 1.0,
      "maxIterationsPerConv": 30
    },
    "cowMemory": {
      "enabled": false,
      "path": "~/.clawft/workspace/cow_memory",
      "ingestTurns": true,
      "cadence": "turn"
    },
    "bindingThreadMode": "deny"
  }
}
```

### `agents` fields

| Field | Type | Default | Browser notes |
|-------|------|---------|---------------|
| `workspaceRoot` / `workspaceRoot` | path \| null | `null` | Native daemon identity root (WEFT-83). Browser FS uses virtual `/clawft/workspace` when OPFS is enabled. |
| `defaults` | object | see below | **Primary** browser knobs. |
| `costBudget` / `costBudget` | object | free-tier caps | Per-conversation circuit breaker (WEFT-322). Useful if you wire full agent loop in browser. |
| `cowMemory` / `cowMemory` | object | disabled | COW memory checkpointing; requires native-ish storage backends. |
| `bindingThreadMode` / `bindingThreadMode` | `"deny"` \| `"warn_only"` | `"deny"` | SOUL.md binding-thread policy (WEFT-342). |

### `agents.defaults`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `workspace` | string | `"~/.nanobot/workspace"` | File-tool working directory. In browser, prefer absolute virtual paths under `/clawft` when using OPFS (`browser-opfs`). |
| `model` | string | local Hermes route on native; set explicitly in browser | **`provider/model`** form (e.g. `openai/gpt-4o`). Selects which `providers.*` entry is used. |
| `maxTokens` / `max_tokens` | integer | `8192` | Max completion tokens. |
| `temperature` | float | `0.7` | Sampling temperature. |
| `maxToolIterations` / `max_tool_iterations` | integer | `20` | Tool loop ceiling per turn. |
| `memoryWindow` / `memory_window` | integer | `50` | Recent messages kept in context. |

After a successful `init`, the browser runtime **strips the provider prefix**
from `agents.defaults.model` and stamps the bare model name for the static
router (see `clawft-wasm` `init`).

### `agents.costBudget`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `maxTokensPerConv` | u64 | `200000` | Cumulative tokens before circuit opens |
| `maxUsdPerConv` | f64 | `1.0` | Cumulative USD spend cap |
| `maxIterationsPerConv` | u32 | `30` | LLM round-trips per conversation |

### `agents.cowMemory`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Opt-in checkpoint bracket |
| `path` | string | `~/.clawft/workspace/cow_memory` | Lineage storage path |
| `ingestTurns` | bool | `true` | Embed turn exchange into working node |
| `cadence` | `"turn"` \| `"tool"` | `"turn"` | Checkpoint frequency |

---

## `providers`

Named map of provider credentials. Keys that matter for browser WASM:

```json
{
  "providers": {
    "anthropic": {
      "apiKey": "sk-ant-…",
      "apiBase": null,
      "extraHeaders": null,
      "browserDirect": true,
      "corsProxy": null
    },
    "openai": {
      "apiKey": "sk-…",
      "browserDirect": false,
      "corsProxy": "https://cors-proxy.example.com"
    },
    "openrouter": { "apiKey": "", "corsProxy": null },
    "deepseek": { "apiKey": "" },
    "groq": { "apiKey": "" },
    "gemini": { "apiKey": "" },
    "xai": { "apiKey": "" },
    "custom": {
      "apiKey": "",
      "apiBase": "https://api.example.com/v1",
      "corsProxy": "https://cors-proxy.example.com"
    },
    "ollama": {
      "apiKey": "",
      "apiBase": "http://localhost:11434/v1",
      "browserDirect": true
    },
    "local": {
      "apiKey": "",
      "apiBase": null
    },
    "zhipu": { "apiKey": "" },
    "dashscope": { "apiKey": "" },
    "vllm": { "apiKey": "" },
    "moonshot": { "apiKey": "" },
    "minimax": { "apiKey": "" },
    "aihubmix": { "apiKey": "" },
    "openai_codex": { "apiKey": "" },
    "elevenlabs": { "apiKey": "" }
  }
}
```

### Provider entry shape (`ProviderConfig`)

| Field | Aliases | Type | Default | Description |
|-------|---------|------|---------|-------------|
| `apiKey` | `api_key` | string (secret) | `""` | API key. **Must be non-empty** for the provider selected by `agents.defaults.model` or `init` fails. |
| `apiBase` | `api_base`, `baseUrl` | string \| null | `null` | Override built-in base URL (proxies, self-host, Azure-style endpoints). |
| `extraHeaders` | `extra_headers` | object \| null | `null` | Extra HTTP headers merged into every request. |
| `browserDirect` | `browser_direct` | bool | `false` | Direct browser access; Anthropic gets `anthropic-dangerous-direct-browser-access: true`. |
| `corsProxy` | `cors_proxy` | string \| null | `null` | CORS proxy origin; requests become `{proxy}/{absolute-target-url}`. |

### Built-in bases (when `apiBase` is null)

From `clawft_llm::config::builtin_providers()`:

| Name | Base URL | Model prefix | Default model |
|------|----------|--------------|---------------|
| `openai` | `https://api.openai.com/v1` | `openai/` | `gpt-4o` |
| `anthropic` | `https://api.anthropic.com/v1` | `anthropic/` | `claude-sonnet-4-5-20250514` |
| `groq` | `https://api.groq.com/openai/v1` | `groq/` | `llama-3.1-70b-versatile` |
| `deepseek` | `https://api.deepseek.com/v1` | `deepseek/` | `deepseek-chat` |
| `openrouter` | `https://openrouter.ai/api/v1` | `openrouter/` | *(none)* |
| `gemini` | `https://generativelanguage.googleapis.com/v1beta/openai` | `gemini/` | `gemini-2.5-flash` |
| `xai` | `https://api.x.ai/v1` | `xai/` | `grok-3-mini` |
| `ollama` | `http://localhost:11434/v1` | `ollama/` | `llama3.2` |
| `local` | Hermes default (`DEFAULT_LOCAL_LLM_API_BASE`) | `local/` | Hermes default model |
| `mistral` | `https://api.mistral.ai/v1` | `mistral/` | `mistral-large-latest` |
| `together` | `https://api.together.xyz/v1` | `together/` | *(none)* |

### Browser provider resolution

1. Match `agents.defaults.model` against builtin `model_prefix` values.
2. Load the matching `providers.<name>` for keys / CORS / base override.
3. If no prefix matches, walk `providers.provider_fallback_order`
   (`providerFallbackOrder`) and pick the first with a non-empty `apiKey`
   (model string sent as-is). **Default** (WEFT-404 back-compat):
   `openrouter` → `openai` → `anthropic` → `groq` → `deepseek` → `gemini` → `xai`.

```json
{
  "providers": {
    "providerFallbackOrder": ["openai", "anthropic", "openrouter"],
    "openai": { "apiKey": "sk-…" }
  }
}
```

Names not in the explicit match table fall through to `providers.custom`.

---

## `gateway` (native / UI API — optional in browser JSON)

Parsed but not started by clawft-wasm. Documented so shared `config.json`
files remain valid.

```json
{
  "gateway": {
    "host": "127.0.0.1",
    "port": 18790,
    "heartbeatIntervalMinutes": 0,
    "heartbeatPrompt": "heartbeat",
    "apiPort": 18789,
    "corsOrigins": ["http://localhost:5173"],
    "apiEnabled": false
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `host` | string | `"127.0.0.1"` | Gateway bind address |
| `port` | u16 | `18790` | Gateway listen port |
| `heartbeatIntervalMinutes` | u64 | `0` | Heartbeat interval (0 = off) |
| `heartbeatPrompt` | string | `"heartbeat"` | Heartbeat prompt text |
| `apiPort` | u16 | `18789` | Separate REST API port for the UI |
| `corsOrigins` | string[] | `["http://localhost:5173"]` | Allowed origins for the **native** UI API — **not** the LLM CORS proxy |
| `apiEnabled` | bool | `false` | Enable REST/WS API |

> **Do not confuse** `gateway.corsOrigins` (daemon UI API) with
> `providers.*.corsProxy` (browser → LLM provider relay).

---

## `tools`

```json
{
  "tools": {
    "web": {
      "search": {
        "apiKey": "",
        "maxResults": 5
      }
    },
    "exec": {
      "timeout": 60
    },
    "restrictToWorkspace": false,
    "mcpServers": {
      "example": {
        "command": "npx",
        "args": ["-y", "some-mcp-server"],
        "env": {},
        "internalOnly": false
      }
    }
  }
}
```

| Field | Browser notes |
|-------|---------------|
| `web.search` | Web-search tool may run if compiled into the browser tool set and a key is present. |
| `exec` | Shell exec is **native-only**; excluded from browser feature builds. |
| `mcpServers` | MCP stdio servers require process spawn — not available in pure WASM. |
| `restrictToWorkspace` | Honored by file tools when present. |

---

## `routing`

Tiered routing / permissions (shared with native). Browser `init` builds a
pipeline with a static router pinned after provider resolution.

```json
{
  "routing": {
    "strategy": "static"
  }
}
```

The dashboard setup form (`browser-config.tsx`) writes
`routing: { strategy: "static" }` by default. Additional routing fields
follow the native `RoutingConfig` schema in `clawft-types` — omit unless you
know you need them.

---

## `pipeline`

```json
{
  "pipeline": {
    "scorer": "noop",
    "learner": "noop"
  }
}
```

| Field | Default | Values |
|-------|---------|--------|
| `scorer` | `"noop"` | `"noop"`, `"fitness"` |
| `learner` | `"noop"` | `"noop"`, `"trajectory"` |

---

## `voice`, `kernel`, `plugins`, `skills`, `delegation`, `agent_routing`, `channels`

These sections use the same types as native config. For browser playgrounds
they can be omitted entirely.

| Section | When to set in browser |
|---------|------------------------|
| `voice` | Only if integrating browser voice UI with shared config files |
| `kernel` | Brand / subsystem toggles; optional |
| `plugins` | Capability grants for plugin hosts (usually native) |
| `skills` | Skill discovery / autogen settings |
| `delegation` | Multi-agent delegation rules |
| `agent_routing` | Doctor / specialist routing table |
| `channels` | Never meaningful inside pure WASM (no Telegram/Slack gateways) |

Full native field tables: [docs/reference/config.md](../reference/config.md).

---

## Dashboard-produced config

The clawft-ui WASM setup screen builds a slightly flattened object:

```json
{
  "defaults": {
    "model": "claude-sonnet-4-5-20250929",
    "max_tokens": 4096
  },
  "providers": {
    "anthropic": {
      "api_key": "…",
      "base_url": undefined,
      "browser_direct": true,
      "cors_proxy": undefined
    }
  },
  "routing": { "strategy": "static" }
}
```

Notes:

- UI uses top-level `defaults` in the object it hands to the host; the WASM
  `Config` type expects `agents.defaults`. The adapter layer that calls
  `init` must nest defaults under `agents` (or pass a hand-authored schema-
  complete JSON as in the `www` harness).
- Encrypted keys in IndexedDB are **not** the `init` payload; plaintext is
  only held in memory for the Authorization header.
- CORS proxy field is validated with `validateCorsProxyUrl` (HTTPS or
  loopback HTTP only).

---

## Optional `env_json` (second `init` argument)

Not part of the config schema, but part of browser init (WEFT-391):

```javascript
await init(
  JSON.stringify(config),
  JSON.stringify({
    "CLAWFT_DEBUG": "1",
    "ANTHROPIC_API_KEY": "sk-ant-…"  // optional seed; prefer apiKey in config
  })
);
```

| Rule | Detail |
|------|--------|
| Type | JSON object of string → string |
| Timing | Applied before `AgentLoop` takes the platform |
| Live updates | `set_env(key, value)` after init mutates the same `Arc` |
| Keys in browser | `std::env` is not available; LLM keys should still come from `providers.*.apiKey` |

---

## Full annotated example (browser playground)

```json
{
  "agents": {
    "defaults": {
      "workspace": "/clawft/workspace",
      "model": "anthropic/claude-sonnet-4-5-20250514",
      "maxTokens": 4096,
      "temperature": 0.7,
      "maxToolIterations": 20,
      "memoryWindow": 50
    },
    "costBudget": {
      "maxTokensPerConv": 200000,
      "maxUsdPerConv": 1.0,
      "maxIterationsPerConv": 30
    }
  },
  "providers": {
    "anthropic": {
      "apiKey": "sk-ant-…",
      "browserDirect": true
    },
    "openai": {
      "apiKey": "sk-…",
      "browserDirect": false,
      "corsProxy": "https://cors-proxy.example.com"
    },
    "openrouter": {
      "apiKey": "sk-or-…",
      "apiBase": "https://openrouter.ai/api/v1",
      "corsProxy": "https://cors-proxy.example.com"
    },
    "groq": {
      "apiKey": "gsk_…",
      "corsProxy": "https://cors-proxy.example.com"
    },
    "deepseek": {
      "apiKey": "sk-…",
      "corsProxy": "https://cors-proxy.example.com"
    },
    "gemini": {
      "apiKey": "AIza…",
      "corsProxy": "https://cors-proxy.example.com"
    },
    "xai": {
      "apiKey": "xai-…",
      "corsProxy": "https://cors-proxy.example.com"
    },
    "ollama": {
      "apiBase": "http://localhost:11434/v1",
      "browserDirect": true
    },
    "custom": {
      "apiKey": "…",
      "apiBase": "https://api.example.com/v1",
      "corsProxy": "https://cors-proxy.example.com",
      "extraHeaders": {
        "X-Custom-Header": "value"
      }
    }
  },
  "routing": {
    "strategy": "static"
  },
  "pipeline": {
    "scorer": "noop",
    "learner": "noop"
  },
  "tools": {
    "web": {
      "search": {
        "apiKey": "",
        "maxResults": 5
      }
    }
  }
}
```

---

## Validation rules summary

| Check | Where enforced |
|-------|----------------|
| Valid JSON + Config shape | `init` → serde parse error |
| Non-empty `apiKey` for resolved provider | `init` after `resolve_provider` |
| Known model prefix or fallback key | `resolve_provider` |
| CORS proxy HTTPS or loopback HTTP | UI `validateCorsProxyUrl` (WEFT-310) |
| Secure context for OPFS / SW | Browser runtime (HTTPS or localhost) |

---

## Related docs

- [cors-provider-setup.md](./cors-provider-setup.md) — per-provider CORS matrix and recipes
- [quickstart.md](./quickstart.md) — minimal harness
- [api-reference.md](./api-reference.md) — `init` / `send_message` / env helpers
- [deployment.md](./deployment.md) — static hosting and sample proxy Worker
- [docs/reference/config.md](../reference/config.md) — full native configuration reference
- [docs/guides/providers.md](../guides/providers.md) — provider routing on native
- [ADR-083](../adr/adr-083-browser-wasm-support.md) — browser WASM architecture
