# API Reference

> Axum REST, WebSocket, and SSE endpoints for the ClawFT backend gateway.

This document covers every HTTP endpoint, the WebSocket pub/sub protocol, and SSE
streaming provided by the `clawft-services` API layer. The implementation lives
in `crates/clawft-services/src/api/`.

**Base URL:** `http://localhost:18789` (default port)

**REST prefix:** All REST endpoints are nested under `/api`.

**Total endpoints:** 45 REST + 1 WebSocket

---

## Table of Contents

- [Getting Started](#getting-started)
- [Architecture Overview](#architecture-overview)
- [Authentication](#authentication)
- [Health](#health)
- [Agents](#agents)
- [Sessions](#sessions)
- [Chat and SSE Streaming](#chat-and-sse-streaming)
- [Tools](#tools)
- [Skills](#skills)
- [Memory](#memory)
- [Config](#config)
- [Cron (Stub)](#cron-stub)
- [Channels](#channels)
- [Delegation](#delegation)
- [Monitoring](#monitoring)
- [Voice](#voice)
- [WebSocket](#websocket)
- [CORS Configuration](#cors-configuration)
- [SPA Fallback](#spa-fallback)
- [Error Handling](#error-handling)
- [Endpoint Summary Table](#endpoint-summary-table)
- [TypeScript Types](#typescript-types)

---

## Getting Started

### Starting the API Server

The API server starts automatically when you launch ClawFT with the gateway
enabled. The default port is `18789`.

```bash
# Start ClawFT (gateway enabled by default)
cargo run --bin clawft

# Or with a custom port and UI directory
cargo run --bin clawft -- --api-port 3100 --ui-dir ./clawft-ui/dist
```

### Quick Smoke Test

```bash
# Health check
curl http://localhost:18789/api/health

# List agents
curl http://localhost:18789/api/agents

# List sessions
curl http://localhost:18789/api/sessions

# Create a bearer token (for future auth)
curl -X POST http://localhost:18789/api/auth/token
```

### Connecting via WebSocket

```javascript
const ws = new WebSocket("ws://localhost:18789/ws");

ws.onopen = () => {
  ws.send(JSON.stringify({ type: "subscribe", topic: "agents" }));
};

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  console.log(msg.type, msg);
};
```

### Connecting via SSE

```javascript
const source = new EventSource("http://localhost:18789/api/sessions/my-session/stream");

source.onmessage = (event) => {
  const data = JSON.parse(event.data);
  console.log("SSE event:", data);
};

source.onerror = () => {
  console.log("SSE connection closed or errored");
};
```

---

## Architecture Overview

The API layer is structured as follows:

| Module | File | Purpose |
|--------|------|---------|
| Router + State | `mod.rs` | `ApiState`, access traits, `build_router()`, `serve()` |
| Bridge | `bridge.rs` | Connects core services to API trait objects (erases `Platform` generic) |
| Handlers | `handlers.rs` | Core REST handlers (agents, sessions, tools, health, auth) |
| WebSocket | `ws.rs` | Topic-based pub/sub over WebSocket |
| Broadcaster | `broadcaster.rs` | `TopicBroadcaster` using `tokio::sync::broadcast` channels |
| Chat / SSE | `chat.rs` | Chat session messaging, create, export, SSE streaming |
| Auth | `auth.rs` | In-memory `TokenStore` + auth middleware (not yet wired) |
| Skills | `skills.rs` | Skills CRUD + registry search |
| Memory | `memory_api.rs` | Memory CRUD + search |
| Config | `config_api.rs` | Configuration get/put |
| Cron | `cron_api.rs` | Cron job management (stub) |
| Channels | `channels_api.rs` | Channel status listing |
| Delegation | `delegation.rs` | Delegation active/rules/history |
| Monitoring | `monitoring.rs` | Token usage, costs, pipeline runs |
| Voice | `voice_api.rs` | Voice settings and device testing (stub) |

### ApiState

All handlers receive `ApiState` via Axum's `State` extractor. It holds `Arc`
references to trait objects that abstract away the underlying platform:

```rust
pub struct ApiState {
    pub tools: Arc<dyn ToolRegistryAccess>,
    pub sessions: Arc<dyn SessionAccess>,
    pub agents: Arc<dyn AgentAccess>,
    pub bus: Arc<dyn BusAccess>,
    pub auth: Arc<auth::TokenStore>,
    pub skills: Arc<dyn SkillAccess>,
    pub memory: Arc<dyn MemoryAccess>,
    pub config: Arc<dyn ConfigAccess>,
    pub channels: Arc<dyn ChannelAccess>,
    pub broadcaster: Arc<broadcaster::TopicBroadcaster>,
}
```

### Implementation Status

Each endpoint is annotated with its implementation status:

- **Live** -- fully wired to core services via bridge traits
- **Mock** -- returns realistic mock data; will be wired in a future phase
- **Stub** -- returns placeholder responses; backend service not yet built

---

## Authentication

Authentication uses in-memory bearer tokens managed by `TokenStore`. Tokens are
UUID v4 strings with a configurable TTL (default: 24 hours / 86400 seconds).

**Important:** The auth middleware exists in `auth.rs` but is **intentionally
disabled** for the development workflow. When enabled, it checks the
`Authorization: Bearer <token>` header on all `/api/*` routes except
`/api/auth/token` and `/api/health`.

### Create Token

```
POST /api/auth/token
```

Generates a new bearer token with a 24-hour TTL. No authentication required.

**Status:** Live

**Request body:** None

**Response:**

```json
{
  "token": "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
}
```

**Example:**

```bash
curl -X POST http://localhost:18789/api/auth/token
```

**Notes:**

- The token is a UUID v4 string (not prefixed).
- Tokens are stored in-memory and are lost on server restart.
- When auth middleware is enabled, include the token as:
  `Authorization: Bearer a1b2c3d4-e5f6-7890-abcd-ef1234567890`

---

## Health

### Health Check

```
GET /api/health
```

Returns basic server health information including uptime and crate version.

**Status:** Live

**Response:**

```json
{
  "status": "ok",
  "version": "0.1.0",
  "uptime_secs": 3600
}
```

| Field | Type | Description |
|-------|------|-------------|
| `status` | `string` | Always `"ok"` if the server is running |
| `version` | `string` | `CARGO_PKG_VERSION` at compile time |
| `uptime_secs` | `u64` | Seconds since server process start |

**Example:**

```bash
curl http://localhost:18789/api/health
```

---

## Agents

Agent endpoints list and control registered agents. Agent definitions are
discovered at startup from the `skills/` directory and are held as an immutable
snapshot in `AgentBridge`.

### List Agents

```
GET /api/agents
```

Returns all registered agents.

**Status:** Live

**Response:** `AgentInfo[]`

```json
[
  {
    "name": "general-agent",
    "description": "General purpose assistant",
    "model": "claude-sonnet-4",
    "skills": ["code-review", "web-search"]
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Unique agent identifier |
| `description` | `string` | Human-readable description |
| `model` | `string` | Default LLM model for this agent |
| `skills` | `string[]` | List of skill names the agent can use |

**Example:**

```bash
curl http://localhost:18789/api/agents
```

### Get Agent Detail

```
GET /api/agents/{name}
```

Returns detailed information about a specific agent by name.

**Status:** Live

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `name` | `string` | Agent name |

**Response:** `AgentInfo | null`

```json
{
  "name": "general-agent",
  "description": "General purpose assistant",
  "model": "claude-sonnet-4",
  "skills": ["code-review", "web-search"]
}
```

Returns `null` if the agent is not found.

**Example:**

```bash
curl http://localhost:18789/api/agents/general-agent
```

### Start Agent

```
POST /api/agents/{name}/start
```

Start a stopped agent.

**Status:** Stub (returns success but does not yet control agent lifecycle)

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `name` | `string` | Agent name |

**Response:**

```json
{ "ok": true }
```

### Stop Agent

```
POST /api/agents/{name}/stop
```

Stop a running agent.

**Status:** Stub (returns success but does not yet control agent lifecycle)

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `name` | `string` | Agent name |

**Response:**

```json
{ "ok": true }
```

---

## Sessions

Session endpoints manage chat sessions persisted by `SessionManager`. Sessions
are identified by a unique string key.

### List Sessions

```
GET /api/sessions
```

Returns all active sessions with metadata.

**Status:** Live

**Response:** `SessionInfo[]`

```json
[
  {
    "key": "web:a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "message_count": 42,
    "created_at": "2026-02-24T10:00:00Z",
    "updated_at": "2026-02-24T10:30:00Z"
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `key` | `string` | Unique session identifier |
| `message_count` | `usize` | Number of messages in the session |
| `created_at` | `string?` | ISO 8601 creation timestamp (null if metadata unavailable) |
| `updated_at` | `string?` | ISO 8601 last-update timestamp (null if metadata unavailable) |

**Example:**

```bash
curl http://localhost:18789/api/sessions
```

### Get Session Detail

```
GET /api/sessions/{key}
```

Returns the full session including message history.

**Status:** Live

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `key` | `string` | Session key |

**Response:** `SessionDetail | null`

```json
{
  "key": "web:a1b2c3d4",
  "messages": [
    { "role": "user", "content": "Hello", "timestamp": "2026-02-24T10:30:00Z" },
    { "role": "assistant", "content": "Hi there!", "timestamp": "2026-02-24T10:30:01Z" }
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `key` | `string` | Session key |
| `messages` | `Value[]` | Array of message objects (schema depends on agent pipeline) |

Returns `null` if the session is not found.

### Delete Session

```
DELETE /api/sessions/{key}
```

Deletes a session by key.

**Status:** Live

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `key` | `string` | Session key |

**Response:** `boolean`

Returns `true` if the session existed and was deleted, `false` otherwise.

**Example:**

```bash
curl -X DELETE http://localhost:18789/api/sessions/web:a1b2c3d4
```

---

## Chat and SSE Streaming

Chat routes handle session creation, message sending, session export, and
real-time SSE streaming. These are defined in `chat.rs`.

### Create Session

```
POST /api/sessions
```

Creates a new chat session for a given agent.

**Status:** Live (generates a session key but does not yet persist to SessionManager)

**Request body:**

```json
{
  "agent_id": "general-agent"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `agent_id` | `string` | Yes | Agent to associate with this session |

**Response:** `SessionSummaryResponse`

```json
{
  "key": "web:a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "agent_id": "general-agent",
  "message_count": 0,
  "updated_at": "2026-02-24T10:30:00+00:00"
}
```

**Example:**

```bash
curl -X POST http://localhost:18789/api/sessions \
  -H "Content-Type: application/json" \
  -d '{"agent_id":"general-agent"}'
```

### Send Message

```
POST /api/sessions/{key}/messages
```

Sends a user message to a session. The message is published to the internal
`MessageBus` (channel: `"web"`, chat_id: session key) for agent processing.
Returns the user message immediately; the agent's response arrives via
WebSocket or SSE.

**Status:** Live

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `key` | `string` | Session key |

**Request body:**

```json
{
  "content": "What files are in the project?"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `content` | `string` | Yes | Message text |

**Response:** `ChatMessageResponse`

```json
{
  "role": "user",
  "content": "What files are in the project?",
  "timestamp": "2026-02-24T10:30:00.123456+00:00"
}
```

**Example:**

```bash
curl -X POST http://localhost:18789/api/sessions/web:abc123/messages \
  -H "Content-Type: application/json" \
  -d '{"content":"Hello, agent!"}'
```

### Export Session

```
GET /api/sessions/{key}/export
```

Exports all messages from a session in a structured format.

**Status:** Live

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `key` | `string` | Session key |

**Response:** `ExportResponse`

```json
{
  "messages": [
    { "role": "user", "content": "Hello", "timestamp": "2026-02-24T10:30:00Z" },
    { "role": "assistant", "content": "Hi there!", "timestamp": "2026-02-24T10:30:01Z" }
  ]
}
```

Returns `{ "messages": [] }` if the session is not found.

**Example:**

```bash
curl http://localhost:18789/api/sessions/web:abc123/export
```

### Stream Session (SSE)

```
GET /api/sessions/{key}/stream
```

Opens a Server-Sent Events (SSE) stream for real-time session updates.
Subscribes to the `sessions:{key}` topic on the `TopicBroadcaster` and
forwards each published message as an SSE data frame.

**Status:** Live

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `key` | `string` | Session key |

**Response:** `text/event-stream`

Each SSE frame contains a JSON-serialized event:

```
data: {"role":"assistant","content":"Here are the files...","timestamp":"2026-02-24T10:30:01Z"}

data: {"type":"tool_use","name":"read_file","input":{"path":"src/main.rs"}}

```

**Behavior:**

- The stream stays open until the client disconnects or the broadcast channel closes.
- If the consumer is too slow, lagged messages are silently skipped (broadcast channel capacity: 256).
- Reconnection is handled by the browser's `EventSource` API automatically.

**JavaScript example:**

```javascript
const session = "web:abc123";
const source = new EventSource(`http://localhost:18789/api/sessions/${session}/stream`);

source.onmessage = (event) => {
  const data = JSON.parse(event.data);
  console.log("Received:", data);

  if (data.role === "assistant") {
    // Display agent response in the UI
    appendMessage(data);
  }
};

source.onerror = (err) => {
  console.error("SSE error:", err);
  // EventSource will automatically reconnect
};
```

**curl example:**

```bash
curl -N http://localhost:18789/api/sessions/web:abc123/stream
```

---

## Tools

Tool endpoints provide read-only access to the tool registry.

### List Tools

```
GET /api/tools
```

Returns all registered tools.

**Status:** Live

**Response:** `ToolInfo[]`

```json
[
  { "name": "read_file", "description": "Read a file from the workspace" },
  { "name": "write_file", "description": "Write content to a file" },
  { "name": "bash", "description": "Execute a bash command" }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Tool identifier |
| `description` | `string` | Human-readable description |

**Example:**

```bash
curl http://localhost:18789/api/tools
```

### Get Tool Schema

```
GET /api/tools/{name}/schema
```

Returns the JSON Schema for a tool's input parameters.

**Status:** Live

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `name` | `string` | Tool name |

**Response:** `Value | null` (JSON Schema object)

```json
{
  "type": "object",
  "properties": {
    "path": { "type": "string", "description": "File path to read" }
  },
  "required": ["path"]
}
```

Returns `null` if the tool is not found.

**Example:**

```bash
curl http://localhost:18789/api/tools/read_file/schema
```

---

## Skills

Skills endpoints manage the skill lifecycle. Skills are loaded from the
`skills/` directory via `SkillsLoader`. The registry search endpoint is a stub
pending ClawHub integration.

### List Installed Skills

```
GET /api/skills
```

Returns all installed skills.

**Status:** Live

**Response:** `SkillDataResponse[]`

```json
[
  {
    "name": "code-review",
    "version": "1.0.0",
    "description": "Automated code review skill",
    "tags": [],
    "installed": true,
    "enabled": true
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Skill identifier |
| `version` | `string` | Semantic version |
| `description` | `string` | Human-readable description |
| `author` | `string?` | Skill author (omitted if null) |
| `tags` | `string[]` | Categorization tags |
| `installed` | `boolean` | Always `true` for listed skills |
| `enabled` | `boolean` | Always `true` for listed skills |

**Example:**

```bash
curl http://localhost:18789/api/skills
```

### Install Skill

```
POST /api/skills/install
```

Install a skill from the ClawHub registry by ID.

**Status:** Stub (returns error "not implemented")

**Request body:**

```json
{
  "id": "clawhub:code-review@1.2.0"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | `string` | Yes | Registry skill identifier |

**Response:**

```json
{ "success": true }
```

Or on error:

```json
{ "success": false, "error": "not implemented" }
```

### Uninstall Skill

```
DELETE /api/skills/{name}
```

Uninstall a skill by name.

**Status:** Stub (returns error "not implemented")

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `name` | `string` | Skill name |

**Response:**

```json
{ "success": true }
```

Or on error:

```json
{ "success": false, "error": "not implemented" }
```

### Search Registry

```
GET /api/skills/registry/search?q=deploy
```

Search the ClawHub skill registry.

**Status:** Stub (always returns empty array)

**Query parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `q` | `string` | No | Search query string |

**Response:** `RegistrySkillResponse[]`

```json
[
  {
    "id": "clawhub:deploy-k8s@2.0.0",
    "name": "deploy-k8s",
    "description": "Kubernetes deployment automation",
    "version": "2.0.0",
    "author": "clawft-team",
    "stars": 42,
    "tags": ["deploy", "kubernetes"],
    "signed": true
  }
]
```

---

## Memory

Memory endpoints provide CRUD and search access to the agent memory system.
Memory is stored as markdown files via `MemoryStore`. The search endpoint
delegates to `MemoryStore::search()`.

### List Entries

```
GET /api/memory
```

Returns all memory entries. Long-term memory is split into paragraphs, each
returned as a separate entry with a generated key.

**Status:** Live

**Response:** `MemoryEntryInfo[]`

```json
[
  {
    "key": "memory:0",
    "value": "JWT with refresh tokens is the preferred auth pattern",
    "namespace": "long_term",
    "tags": [],
    "updated_at": ""
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `key` | `string` | Generated key (`memory:{index}`) |
| `value` | `string` | Entry content |
| `namespace` | `string` | Always `"long_term"` for listed entries |
| `tags` | `string[]` | Tags (currently always empty for listed entries) |
| `updated_at` | `string` | ISO 8601 timestamp (empty for existing entries) |
| `similarity` | `f64?` | Omitted for list results |

**Example:**

```bash
curl http://localhost:18789/api/memory
```

### Search Memory

```
GET /api/memory/search?q=auth&threshold=0.7
```

Search memory entries by query. Returns up to 50 results.

**Status:** Live

**Query parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `q` | `string` | No | `""` | Search query |
| `threshold` | `f64` | No | `0.0` | Similarity threshold (accepted but not yet used in filtering) |

**Response:** `MemoryEntryInfo[]`

```json
[
  {
    "key": "search:0",
    "value": "JWT with refresh tokens is the preferred auth pattern",
    "namespace": "search",
    "tags": [],
    "updated_at": ""
  }
]
```

**Example:**

```bash
curl "http://localhost:18789/api/memory/search?q=authentication&threshold=0.7"
```

### Create Entry

```
POST /api/memory
```

Store a new memory entry. Appends to long-term memory.

**Status:** Live

**Request body:**

```json
{
  "key": "pattern-auth",
  "value": "JWT with refresh tokens is the preferred auth pattern",
  "namespace": "patterns",
  "tags": ["auth", "jwt"]
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `key` | `string` | Yes | -- | Entry key (currently unused; content is appended) |
| `value` | `string` | Yes | -- | Entry content |
| `namespace` | `string` | No | `""` | Namespace (currently unused) |
| `tags` | `string[]` | No | `[]` | Tags (currently unused) |

**Response:** `MemoryEntryInfo`

```json
{
  "key": "memory:a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "value": "JWT with refresh tokens is the preferred auth pattern",
  "namespace": "long_term",
  "tags": [],
  "updated_at": "2026-02-24T10:30:00.123456+00:00"
}
```

Or on error:

```json
{ "error": "failed to write memory file" }
```

**Example:**

```bash
curl -X POST http://localhost:18789/api/memory \
  -H "Content-Type: application/json" \
  -d '{"key":"note-1","value":"Important finding about caching","namespace":"notes","tags":["cache"]}'
```

### Delete Entry

```
DELETE /api/memory/{key}
```

Delete a memory entry by key.

**Status:** Stub (always returns `false`; memory files are append-only)

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `key` | `string` | Memory entry key |

**Response:**

```json
{ "success": false }
```

---

## Config

Configuration endpoints expose and (eventually) allow modification of the
runtime configuration. Secrets (API keys) are stripped from the response --
only a boolean `api_key_set` flag is returned.

### Get Configuration

```
GET /api/config
```

Returns the current configuration with secrets redacted.

**Status:** Live

**Response:** `ConfigData`

```json
{
  "agents": {
    "defaults": {
      "model": "claude-sonnet-4",
      "max_tokens": 4096,
      "temperature": 0.7
    }
  },
  "providers": {
    "anthropic": {
      "api_key_set": true,
      "api_base": "https://api.anthropic.com",
      "enabled": true
    },
    "openai": {
      "api_key_set": false,
      "api_base": "",
      "enabled": false
    },
    "deepseek": {
      "api_key_set": false,
      "api_base": "",
      "enabled": false
    },
    "openrouter": {
      "api_key_set": false,
      "api_base": "",
      "enabled": false
    }
  },
  "channels": {
    "telegram": { "enabled": false },
    "slack": { "enabled": false },
    "discord": { "enabled": true }
  },
  "gateway": {
    "api_port": 18789,
    "api_enabled": true
  }
}
```

**Example:**

```bash
curl http://localhost:18789/api/config
```

### Update Configuration

```
PUT /api/config
```

Replace the current configuration. Accepts a full `ConfigData` JSON object.

**Status:** Stub (always returns error "config saving not yet implemented")

**Request body:** Full `ConfigData` JSON object (same shape as GET response).

**Response:**

```json
{ "success": true }
```

Or on error:

```json
{ "success": false, "error": "config saving not yet implemented" }
```

---

## Cron (Stub)

Cron endpoints manage scheduled jobs. All handlers currently return stub
data -- `CronService` integration is planned for a future phase.

### List Jobs

```
GET /api/cron
```

**Status:** Stub (returns empty array)

**Response:** `CronJobResponse[]`

```json
[]
```

### Create Job

```
POST /api/cron
```

**Status:** Stub (returns the job as if created with a generated UUID)

**Request body:**

```json
{
  "name": "daily-backup",
  "schedule": "0 2 * * *",
  "enabled": true,
  "payload": "{\"action\":\"backup\"}"
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | `string` | Yes | -- | Human-readable job name |
| `schedule` | `string` | Yes | -- | Cron expression |
| `enabled` | `boolean` | No | `true` | Whether the job is active |
| `payload` | `string?` | No | `null` | JSON payload for the job |

**Response:** `CronJobResponse`

```json
{
  "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "name": "daily-backup",
  "schedule": "0 2 * * *",
  "enabled": true,
  "status": "idle"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | `string` | UUID of the job |
| `name` | `string` | Job name |
| `schedule` | `string` | Cron expression |
| `enabled` | `boolean` | Whether the job is active |
| `status` | `string` | Execution status (`idle`, `running`, `error`) |
| `last_run` | `string?` | ISO 8601 timestamp of last run |
| `next_run` | `string?` | ISO 8601 timestamp of next scheduled run |
| `payload` | `string?` | JSON payload |

### Update Job

```
PUT /api/cron/{id}
```

**Status:** Stub (returns the job as if updated)

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `string` | Job UUID |

**Request body:** Partial update fields.

```json
{
  "name": "weekly-backup",
  "schedule": "0 2 * * 0",
  "enabled": false,
  "payload": null
}
```

All fields are optional.

**Response:** `CronJobResponse`

### Delete Job

```
DELETE /api/cron/{id}
```

**Status:** Stub (always returns success)

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `string` | Job UUID |

**Response:**

```json
{ "success": true }
```

### Run Job Now

```
POST /api/cron/{id}/run
```

Trigger immediate execution of a scheduled job.

**Status:** Stub (always returns success)

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `string` | Job UUID |

**Response:**

```json
{ "success": true }
```

---

## Channels

### List Channels

```
GET /api/channels
```

Returns status information for all configured communication channels. Channel
statuses are built from `ChannelsConfig` at startup.

**Status:** Live

**Response:** `ChannelStatusInfo[]`

```json
[
  {
    "name": "telegram",
    "type": "telegram",
    "status": "disconnected",
    "message_count": 0
  },
  {
    "name": "slack",
    "type": "slack",
    "status": "disconnected",
    "message_count": 0
  },
  {
    "name": "discord",
    "type": "discord",
    "status": "connected",
    "message_count": 0
  },
  {
    "name": "web",
    "type": "web",
    "status": "connected",
    "message_count": 0
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Channel identifier |
| `type` | `string` | Channel type (`telegram`, `slack`, `discord`, `web`) |
| `status` | `string` | `"connected"` or `"disconnected"` |
| `message_count` | `u64` | Total messages processed (starts at 0) |
| `last_activity` | `string?` | ISO 8601 timestamp of last activity (omitted if null) |
| `routes_to` | `string?` | Agent this channel routes to (omitted if null) |

**Example:**

```bash
curl http://localhost:18789/api/channels
```

---

## Delegation

Delegation endpoints expose the 3-tier model routing system. The delegation
manager routes tasks to Agent Booster (WASM, Tier 1), Haiku (Tier 2), or
Sonnet/Opus (Tier 3) based on complexity scores and configured rules.

### List Active Delegations

```
GET /api/delegation/active
```

Returns currently active (in-flight) delegations.

**Status:** Mock (returns realistic sample data)

**Response:** `ActiveDelegation[]`

```json
[
  {
    "task_id": "del-001",
    "session_key": "sess-abc-123",
    "target": "claude-sonnet-4",
    "status": "running",
    "started_at": "2026-02-24T10:30:00Z",
    "latency_ms": 1250,
    "tool_name": "code-review",
    "complexity": 0.72
  },
  {
    "task_id": "del-002",
    "session_key": "sess-def-456",
    "target": "claude-haiku-3.5",
    "status": "pending",
    "latency_ms": null,
    "started_at": "2026-02-24T10:31:00Z",
    "tool_name": "file-search",
    "complexity": 0.18
  },
  {
    "task_id": "del-003",
    "session_key": "sess-abc-123",
    "target": "agent-booster",
    "status": "running",
    "started_at": "2026-02-24T10:31:30Z",
    "latency_ms": 2,
    "tool_name": "format-code",
    "complexity": 0.05
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `task_id` | `string` | Unique delegation identifier |
| `session_key` | `string` | Session that triggered the delegation |
| `target` | `string` | Model/engine handling the task |
| `status` | `string` | One of: `pending`, `running`, `completed`, `failed` |
| `started_at` | `string` | ISO 8601 start timestamp |
| `latency_ms` | `u64?` | Latency in milliseconds (null if still pending) |
| `tool_name` | `string` | Tool being delegated |
| `complexity` | `f64` | Computed complexity score (0.0 to 1.0) |

**Example:**

```bash
curl http://localhost:18789/api/delegation/active
```

### List Delegation Rules

```
GET /api/delegation/rules
```

Returns configured delegation routing rules.

**Status:** Mock (returns sample rules)

**Response:** `DelegationRule[]`

```json
[
  {
    "name": "simple-transforms",
    "pattern": "format-*|lint-*",
    "target": "agent-booster",
    "complexity_threshold": 0.1,
    "enabled": true,
    "priority": 1
  },
  {
    "name": "low-complexity",
    "pattern": "search-*|list-*",
    "target": "claude-haiku-3.5",
    "complexity_threshold": 0.3,
    "enabled": true,
    "priority": 2
  },
  {
    "name": "high-complexity",
    "pattern": "*",
    "target": "claude-sonnet-4",
    "complexity_threshold": 1.0,
    "enabled": true,
    "priority": 10
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Rule identifier |
| `pattern` | `string` | Glob pattern matching tool names |
| `target` | `string` | Delegation target model/engine |
| `complexity_threshold` | `f64` | Max complexity for this rule (0.0 to 1.0) |
| `enabled` | `boolean` | Whether the rule is active |
| `priority` | `u32` | Lower values are evaluated first |

### Upsert Delegation Rule

```
PATCH /api/delegation/rules
```

Create or update a delegation rule.

**Status:** Stub (returns the rule as-is without persisting)

**Request body:** `DelegationRule`

```json
{
  "name": "custom-rule",
  "pattern": "analyze-*",
  "target": "claude-sonnet-4",
  "complexity_threshold": 0.5,
  "enabled": true,
  "priority": 5
}
```

**Response:** `DelegationRule` (the submitted rule echoed back)

### Delete Delegation Rule

```
DELETE /api/delegation/rules/{name}
```

Delete a delegation rule by name.

**Status:** Stub (always returns success)

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `name` | `string` | Rule name |

**Response:**

```json
{ "deleted": "custom-rule" }
```

### Delegation History

```
GET /api/delegation/history?session=sess-abc&target=claude&offset=0&limit=50
```

Query past delegation records with optional filtering and pagination.

**Status:** Mock (returns sample history data with working filter/pagination)

**Query parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `session` | `string` | No | -- | Filter by session key (exact match) |
| `target` | `string` | No | -- | Filter by delegation target (exact match) |
| `offset` | `usize` | No | `0` | Pagination offset |
| `limit` | `usize` | No | `50` | Maximum results per page |

**Response:** `PaginatedHistory`

```json
{
  "items": [
    {
      "task_id": "del-h01",
      "session_key": "sess-abc-123",
      "target": "claude-sonnet-4",
      "tool_name": "code-review",
      "status": "completed",
      "started_at": "2026-02-24T09:00:00Z",
      "completed_at": "2026-02-24T09:00:03Z",
      "latency_ms": 3200,
      "complexity": 0.65
    }
  ],
  "total": 5,
  "limit": 50,
  "offset": 0
}
```

| Field | Type | Description |
|-------|------|-------------|
| `items` | `DelegationHistoryEntry[]` | Page of history entries |
| `total` | `usize` | Total entries matching the filter |
| `limit` | `usize` | Page size |
| `offset` | `usize` | Page offset |

**DelegationHistoryEntry fields:**

| Field | Type | Description |
|-------|------|-------------|
| `task_id` | `string` | Delegation identifier |
| `session_key` | `string` | Source session |
| `target` | `string` | Model/engine that handled the task |
| `tool_name` | `string` | Tool that was delegated |
| `status` | `string` | One of: `pending`, `running`, `completed`, `failed` |
| `started_at` | `string` | ISO 8601 start timestamp |
| `completed_at` | `string?` | ISO 8601 completion timestamp (null if not yet completed) |
| `latency_ms` | `u64?` | Total latency in milliseconds |
| `complexity` | `f64` | Computed complexity score |

**Example:**

```bash
# All history
curl http://localhost:18789/api/delegation/history

# Filter by session
curl "http://localhost:18789/api/delegation/history?session=sess-abc-123&limit=10"

# Filter by target
curl "http://localhost:18789/api/delegation/history?target=agent-booster"
```

---

## Monitoring

Monitoring endpoints provide telemetry data for token usage, cost tracking,
and pipeline run history.

### Token Usage

```
GET /api/monitoring/token-usage
```

Returns aggregated token usage statistics.

**Status:** Mock (returns sample data)

**Response:** `TokenUsageSummary`

```json
{
  "total_input": 334000,
  "total_output": 113000,
  "total_requests": 460,
  "by_provider": [
    {
      "provider": "anthropic",
      "model": "claude-sonnet-4",
      "input_tokens": 245000,
      "output_tokens": 82000,
      "total_tokens": 327000,
      "request_count": 142
    },
    {
      "provider": "anthropic",
      "model": "claude-haiku-3.5",
      "input_tokens": 89000,
      "output_tokens": 31000,
      "total_tokens": 120000,
      "request_count": 318
    }
  ],
  "by_session": [
    {
      "session_key": "sess-abc-123",
      "input_tokens": 180000,
      "output_tokens": 65000,
      "request_count": 95
    }
  ]
}
```

**Example:**

```bash
curl http://localhost:18789/api/monitoring/token-usage
```

### Cost Breakdown

```
GET /api/monitoring/costs
```

Returns cost breakdown by provider and 3-tier routing.

**Status:** Mock (returns sample data)

**Response:** `CostBreakdown`

```json
{
  "total_cost_usd": 2.018,
  "by_provider": [
    {
      "provider": "anthropic",
      "model": "claude-sonnet-4",
      "input_cost_usd": 0.735,
      "output_cost_usd": 1.23,
      "total_cost_usd": 1.965
    },
    {
      "provider": "anthropic",
      "model": "claude-haiku-3.5",
      "input_cost_usd": 0.022,
      "output_cost_usd": 0.031,
      "total_cost_usd": 0.053
    }
  ],
  "by_tier": [
    { "tier": 1, "label": "Agent Booster (WASM)", "request_count": 1240, "total_cost_usd": 0.0 },
    { "tier": 2, "label": "Haiku", "request_count": 318, "total_cost_usd": 0.053 },
    { "tier": 3, "label": "Sonnet/Opus", "request_count": 142, "total_cost_usd": 1.965 }
  ]
}
```

**Example:**

```bash
curl http://localhost:18789/api/monitoring/costs
```

### Pipeline Runs

```
GET /api/monitoring/pipeline-runs
```

Returns recent pipeline execution history.

**Status:** Mock (returns sample data)

**Response:** `PipelineRun[]`

```json
[
  {
    "id": "run-001",
    "session_key": "sess-abc-123",
    "model": "claude-sonnet-4",
    "complexity": 0.72,
    "latency_ms": 3200,
    "status": "success",
    "timestamp": "2026-02-24T10:30:00Z"
  },
  {
    "id": "run-004",
    "session_key": "sess-ghi-789",
    "model": "claude-sonnet-4",
    "complexity": 0.88,
    "latency_ms": 5000,
    "status": "error",
    "timestamp": "2026-02-24T10:32:00Z"
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | `string` | Pipeline run identifier |
| `session_key` | `string` | Session that triggered the run |
| `model` | `string` | Model used (`claude-sonnet-4`, `claude-haiku-3.5`, `agent-booster`) |
| `complexity` | `f64` | Computed complexity score |
| `latency_ms` | `u64` | Total latency in milliseconds |
| `status` | `string` | `"success"` or `"error"` |
| `timestamp` | `string` | ISO 8601 timestamp |

**Example:**

```bash
curl http://localhost:18789/api/monitoring/pipeline-runs
```

---

## Voice

Voice endpoints manage the voice pipeline (wake word, speech-to-text,
text-to-speech). Status and settings endpoints are live; hardware test
endpoints remain stubs. Cloud TTS synthesis is available via the `/tts`
endpoint.

### Get Voice Status

```
GET /api/voice/status
```

**Status:** Live

**Response:** `VoiceStatusResponse`

```json
{
  "state": "idle",
  "talkModeActive": false,
  "wakeWordEnabled": false,
  "settings": {
    "enabled": false,
    "wakeWordEnabled": false,
    "language": "en-US",
    "echoCancel": true,
    "noiseSuppression": true,
    "pushToTalk": false
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `state` | `string` | Voice state (`idle`, `listening`, `speaking`) |
| `talkModeActive` | `boolean` | Whether continuous talk mode is on |
| `wakeWordEnabled` | `boolean` | Whether wake word detection is active |
| `settings` | `VoiceSettings` | Current voice configuration |

### Update Voice Settings

```
PUT /api/voice/settings
```

**Status:** Live (persists in memory for the session)

**Request body:** Partial `VoiceSettings`

```json
{
  "enabled": true,
  "wakeWordEnabled": true,
  "language": "en-US",
  "echoCancel": true,
  "noiseSuppression": true,
  "pushToTalk": false
}
```

All fields are optional.

**Response:**

```json
{ "success": true }
```

### Test Microphone

```
POST /api/voice/test-mic
```

**Status:** Stub

**Response:**

```json
{ "success": true, "level": 0.0 }
```

### Test Speaker

```
POST /api/voice/test-speaker
```

**Status:** Stub

**Response:**

```json
{ "success": true }
```

### Get TTS Configuration

```
GET /api/voice/tts/config
```

**Status:** Live

Returns the current TTS provider configuration. No secrets are exposed.

**Response:**

```json
{
  "provider": "openai",
  "model": "tts-1",
  "voice": "alloy",
  "speed": 1.0
}
```

| Field | Type | Description |
|-------|------|-------------|
| `provider` | `string` | TTS provider name (e.g. `openai`, `browser`) |
| `model` | `string` | Model identifier used by the provider |
| `voice` | `string` | Default voice name |
| `speed` | `number` | Default playback speed multiplier |

### Synthesize Speech (Cloud TTS)

```
POST /api/voice/tts
```

**Status:** Live

Cloud TTS synthesis proxy. Sends text to the configured cloud TTS provider
and returns `audio/mpeg` bytes. API keys are kept server-side.

**Request body:**

```json
{
  "text": "Hello, how can I help you?",
  "voice": "nova",
  "speed": 1.1
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `text` | `string` | Yes | Text to synthesize |
| `voice` | `string` | No | Override the configured voice |
| `speed` | `number` | No | Override the configured speed (0.25 - 4.0) |

**Response:** `audio/mpeg` binary data (`Content-Type: audio/mpeg`)

**Error responses** (JSON):

| Status | Reason |
|--------|--------|
| `400` | Provider is `browser` -- client should use Web Speech API directly |
| `400` | Empty `text` field |
| `500` | No API key configured for the TTS provider |
| `502` | Upstream TTS API error |

---

## WebSocket

The WebSocket endpoint provides topic-based pub/sub for real-time events. It
is implemented in `ws.rs` and uses the `TopicBroadcaster` from
`broadcaster.rs`.

### Connection

```
ws://localhost:18789/ws
```

On connection, the server immediately sends a welcome message:

```json
{ "type": "connected", "message": "ClawFT WebSocket connected" }
```

### Protocol

All messages are JSON objects. The client sends **commands** and the server
sends **events**.

#### Client Commands

| Command | Fields | Description |
|---------|--------|-------------|
| `subscribe` | `type`, `topic` | Subscribe to a named topic |
| `unsubscribe` | `type`, `topic` | Unsubscribe from a topic |
| `ping` | `type` | Keepalive ping |

**Subscribe:**

```json
{ "type": "subscribe", "topic": "agents" }
```

If `topic` is omitted, defaults to `"*"`.

**Unsubscribe:**

```json
{ "type": "unsubscribe", "topic": "agents" }
```

**Ping:**

```json
{ "type": "ping" }
```

#### Server Events

| Event | Fields | Description |
|-------|--------|-------------|
| `connected` | `type`, `message` | Sent on initial connection |
| `subscribed` | `type`, `topic` | Acknowledgement after subscribe |
| `unsubscribed` | `type`, `topic` | Acknowledgement after unsubscribe |
| `event` | `type`, `topic`, `data` | Broadcast event on a subscribed topic |
| `pong` | `type` | Response to ping |

**Subscription ack:**

```json
{ "type": "subscribed", "topic": "agents" }
```

**Broadcast event:**

```json
{
  "type": "event",
  "topic": "agents",
  "data": {
    "event": "agent_started",
    "name": "coder"
  }
}
```

**Pong:**

```json
{ "type": "pong" }
```

### Topics

Topics are arbitrary strings. Producers publish to topics via
`TopicBroadcaster::publish()`. Common topics include:

| Topic | Description |
|-------|-------------|
| `agents` | Agent lifecycle events (start, stop, status changes) |
| `sessions` | Global session events |
| `sessions:{key}` | Events for a specific session (messages, completions) |

New topics are created automatically when the first subscriber connects.

### Behavior Details

- **Channel capacity:** Each topic uses a `tokio::sync::broadcast` channel
  with capacity **256** messages.
- **Slow consumers:** If a client falls behind, lagged messages are silently
  skipped (no error sent to the client).
- **Duplicate subscriptions:** Subscribing to the same topic twice returns
  another `subscribed` ack but does not create a duplicate forwarding task.
- **Cleanup:** When the WebSocket closes, all subscription forwarding tasks
  are aborted.
- **Invalid JSON:** Non-JSON text messages are silently ignored.
- **Binary messages:** Ignored.
- **Close frames:** Trigger cleanup and disconnection.

### Full JavaScript Example

```javascript
const ws = new WebSocket("ws://localhost:18789/ws");

ws.onopen = () => {
  console.log("Connected to ClawFT WebSocket");

  // Subscribe to agent events
  ws.send(JSON.stringify({ type: "subscribe", topic: "agents" }));

  // Subscribe to a specific session
  ws.send(JSON.stringify({ type: "subscribe", topic: "sessions:web:abc123" }));

  // Start keepalive pings every 30 seconds
  setInterval(() => {
    ws.send(JSON.stringify({ type: "ping" }));
  }, 30000);
};

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);

  switch (msg.type) {
    case "connected":
      console.log("Server says:", msg.message);
      break;

    case "subscribed":
      console.log("Subscribed to topic:", msg.topic);
      break;

    case "unsubscribed":
      console.log("Unsubscribed from topic:", msg.topic);
      break;

    case "event":
      console.log(`Event on [${msg.topic}]:`, msg.data);
      handleEvent(msg.topic, msg.data);
      break;

    case "pong":
      // Keepalive acknowledged
      break;
  }
};

ws.onclose = () => {
  console.log("WebSocket disconnected");
};

ws.onerror = (error) => {
  console.error("WebSocket error:", error);
};

function handleEvent(topic, data) {
  if (topic === "agents") {
    updateAgentStatus(data);
  } else if (topic.startsWith("sessions:")) {
    appendSessionMessage(data);
  }
}
```

### WebSocket via curl (wscat)

```bash
# Install wscat
npm install -g wscat

# Connect
wscat -c ws://localhost:18789/ws

# Then type commands:
> {"type":"subscribe","topic":"agents"}
< {"type":"subscribed","topic":"agents"}
> {"type":"ping"}
< {"type":"pong"}
```

---

## CORS Configuration

CORS is handled via `tower_http::cors::CorsLayer`.

- **Default (no `cors_origins` configured):** `CorsLayer::permissive()` -- all
  origins, methods, and headers allowed.
- **With `cors_origins` array:** Only the listed origins are allowed. Methods
  and headers remain permissive (`Any`).

```rust
// In build_router():
let cors = if cors_origins.is_empty() {
    CorsLayer::permissive()
} else {
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods(Any)
        .allow_headers(Any)
};
```

---

## SPA Fallback

When the `--ui-dir` flag is provided at startup, a `tower_http::services::ServeDir`
fallback is installed. Any path not matched by `/api` or `/ws` routes serves
the static file from that directory, with `index.html` appended for directories
(SPA-style routing).

```bash
# Serve the built UI from ./clawft-ui/dist
cargo run --bin clawft -- --ui-dir ./clawft-ui/dist
```

With this configuration:
- `GET /api/agents` -- handled by the API router
- `GET /ws` -- handled by the WebSocket router
- `GET /` -- serves `./clawft-ui/dist/index.html`
- `GET /settings` -- serves `./clawft-ui/dist/index.html` (SPA fallback)
- `GET /assets/style.css` -- serves `./clawft-ui/dist/assets/style.css`

---

## Error Handling

The API uses standard HTTP status codes. Most endpoints return JSON responses
even on error.

| Status | Meaning |
|--------|---------|
| `200 OK` | Success |
| `401 Unauthorized` | Invalid or missing bearer token (when auth is enabled) |
| `404 Not Found` | Resource not found (returned as `null` for GET-by-id endpoints) |
| `422 Unprocessable Entity` | Invalid request body (Axum JSON deserialization failure) |
| `500 Internal Server Error` | Unexpected server error |

Error responses from handlers that can fail:

```json
{ "error": "description of what went wrong" }
```

Or for boolean-result operations:

```json
{ "success": false, "error": "description of what went wrong" }
```

---

## Endpoint Summary Table

| # | Method | Path | Status | Description |
|---|--------|------|--------|-------------|
| 1 | `POST` | `/api/auth/token` | Live | Create bearer token |
| 2 | `GET` | `/api/health` | Live | Health check |
| 3 | `GET` | `/api/agents` | Live | List agents |
| 4 | `GET` | `/api/agents/{name}` | Live | Get agent detail |
| 5 | `POST` | `/api/agents/{name}/start` | Stub | Start agent |
| 6 | `POST` | `/api/agents/{name}/stop` | Stub | Stop agent |
| 7 | `GET` | `/api/sessions` | Live | List sessions |
| 8 | `GET` | `/api/sessions/{key}` | Live | Get session detail |
| 9 | `POST` | `/api/sessions` | Live | Create session |
| 10 | `DELETE` | `/api/sessions/{key}` | Live | Delete session |
| 11 | `GET` | `/api/sessions/{key}/export` | Live | Export session messages |
| 12 | `POST` | `/api/sessions/{key}/messages` | Live | Send message |
| 13 | `GET` | `/api/sessions/{key}/stream` | Live | SSE event stream |
| 14 | `GET` | `/api/tools` | Live | List tools |
| 15 | `GET` | `/api/tools/{name}/schema` | Live | Get tool JSON schema |
| 16 | `GET` | `/api/skills` | Live | List installed skills |
| 17 | `POST` | `/api/skills/install` | Stub | Install skill |
| 18 | `DELETE` | `/api/skills/{name}` | Stub | Uninstall skill |
| 19 | `GET` | `/api/skills/registry/search` | Stub | Search skill registry |
| 20 | `GET` | `/api/memory` | Live | List memory entries |
| 21 | `GET` | `/api/memory/search` | Live | Search memory |
| 22 | `POST` | `/api/memory` | Live | Create memory entry |
| 23 | `DELETE` | `/api/memory/{key}` | Stub | Delete memory entry |
| 24 | `GET` | `/api/config` | Live | Get configuration |
| 25 | `PUT` | `/api/config` | Stub | Update configuration |
| 26 | `GET` | `/api/cron` | Stub | List cron jobs |
| 27 | `POST` | `/api/cron` | Stub | Create cron job |
| 28 | `PUT` | `/api/cron/{id}` | Stub | Update cron job |
| 29 | `DELETE` | `/api/cron/{id}` | Stub | Delete cron job |
| 30 | `POST` | `/api/cron/{id}/run` | Stub | Run cron job now |
| 31 | `GET` | `/api/channels` | Live | List channel statuses |
| 32 | `GET` | `/api/delegation/active` | Mock | List active delegations |
| 33 | `GET` | `/api/delegation/rules` | Mock | List delegation rules |
| 34 | `PATCH` | `/api/delegation/rules` | Stub | Upsert delegation rule |
| 35 | `DELETE` | `/api/delegation/rules/{name}` | Stub | Delete delegation rule |
| 36 | `GET` | `/api/delegation/history` | Mock | Query delegation history |
| 37 | `GET` | `/api/monitoring/token-usage` | Mock | Token usage summary |
| 38 | `GET` | `/api/monitoring/costs` | Mock | Cost breakdown |
| 39 | `GET` | `/api/monitoring/pipeline-runs` | Mock | Pipeline run history |
| 40 | `GET` | `/api/voice/status` | Live | Voice status |
| 41 | `PUT` | `/api/voice/settings` | Live | Update voice settings |
| 42 | `POST` | `/api/voice/test-mic` | Stub | Test microphone |
| 43 | `POST` | `/api/voice/test-speaker` | Stub | Test speaker |
| 44 | `GET` | `/api/voice/tts/config` | Live | TTS provider configuration |
| 45 | `POST` | `/api/voice/tts` | Live | Cloud TTS synthesis |
| -- | `GET` | `/ws` | Live | WebSocket upgrade |

---

## TypeScript Types

The UI frontend at `ui/src/lib/types.ts` defines TypeScript interfaces matching
the API response shapes. The API client at `ui/src/lib/api-client.ts` wraps all
endpoints with typed fetch calls.

Key type mappings:

| Rust Type | TypeScript Type | Used By |
|-----------|-----------------|---------|
| `AgentInfo` | `AgentSummary` | `/api/agents` |
| `SessionInfo` | `SessionSummary` | `/api/sessions` |
| `SessionDetail` | `SessionDetail` | `/api/sessions/{key}` |
| `ChatMessageResponse` | `ChatMessage` | `/api/sessions/{key}/messages` |
| `ToolInfo` | `ToolInfo` | `/api/tools` |
| `SkillDataResponse` | `SkillData` | `/api/skills` |
| `RegistrySkillResponse` | `RegistrySkill` | `/api/skills/registry/search` |
| `MemoryEntryInfo` | `MemoryEntry` | `/api/memory` |
| `ActiveDelegation` | `ActiveDelegation` | `/api/delegation/active` |
| `DelegationRule` | `DelegationRule` | `/api/delegation/rules` |
| `TokenUsageSummary` | `TokenUsageSummary` | `/api/monitoring/token-usage` |
| `CostBreakdown` | `CostBreakdown` | `/api/monitoring/costs` |
| `PipelineRun` | `PipelineRun` | `/api/monitoring/pipeline-runs` |
| `ChannelStatusInfo` | `ChannelStatus` | `/api/channels` |
| `CronJobResponse` | `CronJob` | `/api/cron` |
| `VoiceStatusResponse` | `VoiceStatusData` | `/api/voice/status` |
