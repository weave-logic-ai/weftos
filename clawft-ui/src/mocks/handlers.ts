import { http, HttpResponse } from "msw";
import type {
  SkillData,
  RegistrySkill,
  MemoryEntry,
  ConfigData,
  CronJob,
  ChannelStatus,
  VoiceSettingsData,
  ActiveDelegation,
  DelegationRule,
  DelegationHistoryEntry,
  TokenUsageSummary,
  CostBreakdown,
  PipelineRun,
} from "../lib/types";

// ── Skills mock data ────────────────────────────────────────────

const mockSkills: SkillData[] = [
  {
    name: "code-review",
    version: "1.2.0",
    description: "Automated code review with style and security checks",
    author: "clawft-team",
    tags: ["review", "security", "quality"],
    installed: true,
    enabled: true,
  },
  {
    name: "git-operations",
    version: "2.0.1",
    description: "Git workflow automation including branching and merging",
    author: "clawft-team",
    tags: ["git", "vcs", "automation"],
    installed: true,
    enabled: true,
  },
  {
    name: "web-search",
    version: "0.9.3",
    description: "Web search and content extraction for research tasks",
    author: "community",
    tags: ["search", "web", "research"],
    installed: true,
    enabled: false,
  },
  {
    name: "db-query",
    version: "1.1.0",
    description: "Database query builder and execution with safety checks",
    author: "clawft-team",
    tags: ["database", "sql", "query"],
    installed: true,
    enabled: true,
  },
  {
    name: "test-runner",
    version: "3.0.0",
    description: "Intelligent test suite runner with coverage analysis",
    author: "community",
    tags: ["testing", "coverage", "ci"],
    installed: true,
    enabled: true,
  },
];

const mockRegistrySkills: RegistrySkill[] = [
  {
    id: "reg-001",
    name: "deploy-k8s",
    description: "Kubernetes deployment automation with rollback support",
    version: "1.0.0",
    author: "infra-team",
    stars: 342,
    tags: ["kubernetes", "deploy", "infra"],
    signed: true,
  },
  {
    id: "reg-002",
    name: "doc-generator",
    description: "Automatic API documentation generation from source code",
    version: "2.1.0",
    author: "docs-team",
    stars: 218,
    tags: ["documentation", "api", "generation"],
    signed: true,
  },
  {
    id: "reg-003",
    name: "perf-profiler",
    description: "Performance profiling and bottleneck detection",
    version: "0.8.0",
    author: "community",
    stars: 156,
    tags: ["performance", "profiling", "optimization"],
    signed: false,
  },
];

// ── Memory mock data ────────────────────────────────────────────

const mockMemoryEntries: MemoryEntry[] = [
  {
    key: "pattern-auth",
    value: "JWT with refresh tokens, 15m access / 7d refresh TTL",
    namespace: "patterns",
    tags: ["auth", "jwt", "security"],
    updated_at: "2026-02-24T10:30:00Z",
  },
  {
    key: "agent-config-defaults",
    value: '{"model":"claude-sonnet-4","max_tokens":4096,"temperature":0.7}',
    namespace: "config",
    tags: ["agent", "defaults"],
    updated_at: "2026-02-24T09:15:00Z",
  },
  {
    key: "error-handling-strategy",
    value: "Exponential backoff with jitter, max 3 retries, circuit breaker after 5 failures",
    namespace: "patterns",
    tags: ["error", "resilience", "retry"],
    updated_at: "2026-02-23T16:45:00Z",
  },
  {
    key: "routing-rules-v2",
    value: '{"default_channel":"web","fallback":"discord","priority":["slack","discord","web"]}',
    namespace: "routing",
    tags: ["routing", "channels"],
    updated_at: "2026-02-23T14:20:00Z",
  },
  {
    key: "deployment-checklist",
    value: "1. Run tests 2. Build containers 3. Push to registry 4. Deploy staging 5. Smoke test 6. Deploy prod",
    namespace: "operations",
    tags: ["deploy", "checklist"],
    updated_at: "2026-02-22T11:00:00Z",
  },
  {
    key: "model-performance-metrics",
    value: '{"sonnet":{"avg_latency_ms":2100,"tokens_per_sec":85},"haiku":{"avg_latency_ms":480,"tokens_per_sec":210}}',
    namespace: "metrics",
    tags: ["model", "performance", "benchmark"],
    updated_at: "2026-02-22T08:30:00Z",
  },
];

// ── Config mock data ────────────────────────────────────────────

const mockConfig: ConfigData = {
  agents: {
    defaults: {
      model: "claude-sonnet-4",
      max_tokens: 4096,
      temperature: 0.7,
    },
  },
  providers: {
    anthropic: {
      api_key_set: true,
      api_base: "https://api.anthropic.com",
      enabled: true,
    },
    openai: {
      api_key_set: false,
      api_base: "https://api.openai.com/v1",
      enabled: false,
    },
  },
  channels: {
    discord: { enabled: true },
    telegram: { enabled: false },
    slack: { enabled: true },
    web: { enabled: true },
  },
  gateway: {
    api_port: 3100,
    api_enabled: true,
  },
};

// ── Cron mock data ──────────────────────────────────────────────

const mockCronJobs: CronJob[] = [
  {
    id: "cron-001",
    name: "health-check",
    schedule: "*/5 * * * *",
    enabled: true,
    status: "idle",
    last_run: "2026-02-24T10:25:00Z",
    next_run: "2026-02-24T10:30:00Z",
  },
  {
    id: "cron-002",
    name: "memory-cleanup",
    schedule: "0 2 * * *",
    enabled: true,
    status: "idle",
    last_run: "2026-02-24T02:00:00Z",
    next_run: "2026-02-25T02:00:00Z",
  },
  {
    id: "cron-003",
    name: "metrics-report",
    schedule: "0 */6 * * *",
    enabled: true,
    status: "running",
    last_run: "2026-02-24T06:00:00Z",
    next_run: "2026-02-24T12:00:00Z",
  },
  {
    id: "cron-004",
    name: "backup-sessions",
    schedule: "0 0 * * 0",
    enabled: false,
    status: "idle",
    last_run: "2026-02-16T00:00:00Z",
    next_run: "2026-03-02T00:00:00Z",
    payload: '{"compress":true,"retain_days":30}',
  },
];

// ── Channels mock data ──────────────────────────────────────────

const mockChannels: ChannelStatus[] = [
  {
    name: "discord-main",
    type: "discord",
    status: "connected",
    message_count: 1247,
    last_activity: "2026-02-24T10:28:00Z",
    routes_to: "general-agent",
  },
  {
    name: "slack-engineering",
    type: "slack",
    status: "connected",
    message_count: 893,
    last_activity: "2026-02-24T10:25:00Z",
    routes_to: "code-review-agent",
  },
  {
    name: "telegram-alerts",
    type: "telegram",
    status: "disconnected",
    message_count: 56,
    last_activity: "2026-02-23T18:00:00Z",
    routes_to: "alert-agent",
  },
  {
    name: "web-chat",
    type: "web",
    status: "connected",
    message_count: 3421,
    last_activity: "2026-02-24T10:29:00Z",
    routes_to: "assistant-agent",
  },
  {
    name: "voice-channel",
    type: "voice",
    status: "error",
    message_count: 12,
    last_activity: "2026-02-24T08:00:00Z",
    routes_to: "voice-agent",
  },
];

// ── Agents mock data ───────────────────────────────────────────

const mockAgents = [
  { id: "agent-001", name: "general-assistant", status: "running" as const, model: "claude-sonnet-4", created_at: "2026-02-24T08:00:00Z" },
  { id: "agent-002", name: "code-reviewer", status: "running" as const, model: "claude-sonnet-4", created_at: "2026-02-24T08:05:00Z" },
  { id: "agent-003", name: "researcher", status: "idle" as const, model: "claude-haiku-3.5", created_at: "2026-02-23T14:00:00Z" },
  { id: "agent-004", name: "test-runner", status: "idle" as const, model: "claude-haiku-3.5", created_at: "2026-02-23T10:00:00Z" },
  { id: "agent-005", name: "voice-agent", status: "error" as const, model: "claude-sonnet-4", created_at: "2026-02-22T12:00:00Z" },
];

// ── Sessions mock data ─────────────────────────────────────────

const mockSessions = [
  { key: "sess-abc-123", agent_id: "agent-001", message_count: 42, updated_at: "2026-02-24T10:30:00Z" },
  { key: "sess-def-456", agent_id: "agent-002", message_count: 18, updated_at: "2026-02-24T10:25:00Z" },
  { key: "sess-ghi-789", agent_id: "agent-003", message_count: 7, updated_at: "2026-02-24T09:45:00Z" },
  { key: "sess-jkl-012", agent_id: "agent-001", message_count: 95, updated_at: "2026-02-23T18:00:00Z" },
];

// ── Tools mock data ────────────────────────────────────────────

const mockTools = [
  { name: "read_file", description: "Read a file from the filesystem" },
  { name: "write_file", description: "Write content to a file" },
  { name: "edit_file", description: "Make targeted edits to a file" },
  { name: "list_directory", description: "List contents of a directory" },
  { name: "exec_shell", description: "Execute a shell command" },
  { name: "web_search", description: "Search the web for information" },
  { name: "web_fetch", description: "Fetch content from a URL" },
  { name: "memory_read", description: "Read from agent memory" },
  { name: "memory_write", description: "Write to agent memory" },
  { name: "delegate_task", description: "Delegate a subtask to another model" },
];

// ── Handlers ────────────────────────────────────────────────────

export const handlers = [
  // Agents
  http.get("/api/agents", () => HttpResponse.json(mockAgents)),
  http.get("/api/agents/:id", ({ params }) => {
    const agent = mockAgents.find((a) => a.id === params.id);
    if (!agent) return new HttpResponse(null, { status: 404 });
    return HttpResponse.json({ ...agent, config: {}, session_count: 2, description: "Mock agent", skills: ["code-review"] });
  }),
  http.post("/api/agents/:id/start", () => HttpResponse.json({ ok: true })),
  http.post("/api/agents/:id/stop", () => HttpResponse.json({ ok: true })),

  // Sessions
  http.get("/api/sessions", () => HttpResponse.json(mockSessions)),
  http.get("/api/sessions/:key", ({ params }) => {
    const session = mockSessions.find((s) => s.key === params.key);
    if (!session) return new HttpResponse(null, { status: 404 });
    return HttpResponse.json({ ...session, messages: [
      { role: "user", content: "Hello", timestamp: "2026-02-24T10:00:00Z" },
      { role: "assistant", content: "Hi! How can I help?", timestamp: "2026-02-24T10:00:01Z" },
    ]});
  }),
  http.post("/api/sessions", () => HttpResponse.json({ key: `sess-${Date.now()}`, agent_id: "agent-001", message_count: 0, updated_at: new Date().toISOString() })),
  http.get("/api/sessions/:key/export", () => HttpResponse.json({ messages: [] })),
  http.delete("/api/sessions/:key", () => HttpResponse.json({ ok: true })),

  // Chat
  http.post("/api/sessions/:key/messages", () =>
    HttpResponse.json({ role: "assistant", content: "This is a mock response from MSW.", timestamp: new Date().toISOString() }),
  ),

  // Tools
  http.get("/api/tools", () => HttpResponse.json(mockTools)),
  http.get("/api/tools/:name/schema", ({ params }) =>
    HttpResponse.json({ type: "object", properties: { path: { type: "string", description: `Path for ${params.name}` } }, required: ["path"] }),
  ),

  // Auth

  // Skills
  http.get("/api/skills", () => HttpResponse.json(mockSkills)),
  http.post("/api/skills/install", () =>
    HttpResponse.json({ success: true }),
  ),
  http.delete("/api/skills/:name", () =>
    HttpResponse.json({ success: true }),
  ),
  http.get("/api/skills/registry/search", ({ request }) => {
    const url = new URL(request.url);
    const q = url.searchParams.get("q")?.toLowerCase() ?? "";
    const results = q
      ? mockRegistrySkills.filter(
          (s) =>
            s.name.includes(q) ||
            s.description.toLowerCase().includes(q) ||
            s.tags.some((t) => t.includes(q)),
        )
      : mockRegistrySkills;
    return HttpResponse.json(results);
  }),

  // Memory
  http.get("/api/memory", () => HttpResponse.json(mockMemoryEntries)),
  http.get("/api/memory/search", ({ request }) => {
    const url = new URL(request.url);
    const q = url.searchParams.get("q")?.toLowerCase() ?? "";
    const results = mockMemoryEntries
      .filter(
        (e) =>
          e.key.includes(q) ||
          e.value.toLowerCase().includes(q) ||
          e.tags.some((t) => t.includes(q)),
      )
      .map((e) => ({
        ...e,
        similarity: Math.round((0.7 + Math.random() * 0.3) * 100) / 100,
      }));
    return HttpResponse.json(results);
  }),
  http.post("/api/memory", () =>
    HttpResponse.json({
      key: "new-entry",
      value: "",
      namespace: "default",
      tags: [],
      updated_at: new Date().toISOString(),
    }),
  ),
  http.delete("/api/memory/:key", () =>
    HttpResponse.json({ success: true }),
  ),

  // Config
  http.get("/api/config", () => HttpResponse.json(mockConfig)),
  http.put("/api/config", () => HttpResponse.json({ success: true })),

  // Cron
  http.get("/api/cron", () => HttpResponse.json(mockCronJobs)),
  http.post("/api/cron", async ({ request }) => {
    const body = (await request.json()) as Record<string, unknown>;
    return HttpResponse.json({
      id: `cron-${Date.now()}`,
      status: "idle",
      ...body,
    });
  }),
  http.put("/api/cron/:id", () =>
    HttpResponse.json({ success: true }),
  ),
  http.delete("/api/cron/:id", () =>
    HttpResponse.json({ success: true }),
  ),
  http.post("/api/cron/:id/run", () =>
    HttpResponse.json({ success: true }),
  ),

  // Channels
  http.get("/api/channels", () => HttpResponse.json(mockChannels)),

  // Health
  http.get("/api/health", () =>
    HttpResponse.json({
      status: "ok",
      version: "0.1.0",
      uptime_secs: 3600,
    }),
  ),

  // Delegation
  http.get("/api/delegation/active", () =>
    HttpResponse.json<ActiveDelegation[]>([
      {
        task_id: "del-001",
        session_key: "sess-abc-123",
        target: "claude-sonnet-4",
        status: "running",
        started_at: "2026-02-24T10:30:00Z",
        latency_ms: 1250,
        tool_name: "code-review",
        complexity: 0.72,
      },
      {
        task_id: "del-002",
        session_key: "sess-def-456",
        target: "claude-haiku-3.5",
        status: "pending",
        started_at: "2026-02-24T10:31:00Z",
        tool_name: "file-search",
        complexity: 0.18,
      },
      {
        task_id: "del-003",
        session_key: "sess-abc-123",
        target: "agent-booster",
        status: "running",
        started_at: "2026-02-24T10:31:30Z",
        latency_ms: 2,
        tool_name: "format-code",
        complexity: 0.05,
      },
    ]),
  ),
  http.get("/api/delegation/rules", () =>
    HttpResponse.json<DelegationRule[]>([
      {
        name: "simple-transforms",
        pattern: "format-*|lint-*",
        target: "agent-booster",
        complexity_threshold: 0.1,
        enabled: true,
        priority: 1,
      },
      {
        name: "low-complexity",
        pattern: "search-*|list-*",
        target: "claude-haiku-3.5",
        complexity_threshold: 0.3,
        enabled: true,
        priority: 2,
      },
      {
        name: "high-complexity",
        pattern: "*",
        target: "claude-sonnet-4",
        complexity_threshold: 1.0,
        enabled: true,
        priority: 10,
      },
    ]),
  ),
  http.patch("/api/delegation/rules", async ({ request }) => {
    const body = (await request.json()) as DelegationRule;
    return HttpResponse.json(body);
  }),
  http.delete("/api/delegation/rules/:name", ({ params }) =>
    HttpResponse.json({ deleted: params.name }),
  ),
  http.get("/api/delegation/history", ({ request }) => {
    const url = new URL(request.url);
    const session = url.searchParams.get("session");
    const target = url.searchParams.get("target");
    const offset = parseInt(url.searchParams.get("offset") ?? "0", 10);
    const limit = parseInt(url.searchParams.get("limit") ?? "50", 10);
    const allItems: DelegationHistoryEntry[] = [
      {
        task_id: "del-h01",
        session_key: "sess-abc-123",
        target: "claude-sonnet-4",
        tool_name: "code-review",
        status: "completed",
        started_at: "2026-02-24T09:00:00Z",
        completed_at: "2026-02-24T09:00:03Z",
        latency_ms: 3200,
        complexity: 0.65,
      },
      {
        task_id: "del-h02",
        session_key: "sess-def-456",
        target: "claude-haiku-3.5",
        tool_name: "file-search",
        status: "completed",
        started_at: "2026-02-24T09:15:00Z",
        completed_at: "2026-02-24T09:15:01Z",
        latency_ms: 480,
        complexity: 0.12,
      },
      {
        task_id: "del-h03",
        session_key: "sess-abc-123",
        target: "agent-booster",
        tool_name: "format-code",
        status: "completed",
        started_at: "2026-02-24T09:20:00Z",
        completed_at: "2026-02-24T09:20:00Z",
        latency_ms: 1,
        complexity: 0.03,
      },
      {
        task_id: "del-h04",
        session_key: "sess-ghi-789",
        target: "claude-sonnet-4",
        tool_name: "architecture-review",
        status: "failed",
        started_at: "2026-02-24T09:30:00Z",
        completed_at: "2026-02-24T09:30:05Z",
        latency_ms: 5000,
        complexity: 0.88,
      },
      {
        task_id: "del-h05",
        session_key: "sess-def-456",
        target: "claude-haiku-3.5",
        tool_name: "summarize",
        status: "completed",
        started_at: "2026-02-24T09:45:00Z",
        completed_at: "2026-02-24T09:45:01Z",
        latency_ms: 620,
        complexity: 0.22,
      },
    ];
    const filtered = allItems
      .filter((e) => !session || e.session_key === session)
      .filter((e) => !target || e.target === target);
    const items = filtered.slice(offset, offset + limit);
    return HttpResponse.json({
      items,
      total: filtered.length,
      limit,
      offset,
    });
  }),

  // Monitoring
  http.get("/api/monitoring/token-usage", () =>
    HttpResponse.json<TokenUsageSummary>({
      total_input: 334000,
      total_output: 113000,
      total_requests: 460,
      by_provider: [
        {
          provider: "anthropic",
          model: "claude-sonnet-4",
          input_tokens: 245000,
          output_tokens: 82000,
          total_tokens: 327000,
          request_count: 142,
        },
        {
          provider: "anthropic",
          model: "claude-haiku-3.5",
          input_tokens: 89000,
          output_tokens: 31000,
          total_tokens: 120000,
          request_count: 318,
        },
      ],
      by_session: [
        { session_key: "sess-abc-123", input_tokens: 180000, output_tokens: 65000, request_count: 95 },
        { session_key: "sess-def-456", input_tokens: 98000, output_tokens: 32000, request_count: 210 },
        { session_key: "sess-ghi-789", input_tokens: 56000, output_tokens: 16000, request_count: 155 },
      ],
    }),
  ),
  http.get("/api/monitoring/costs", () =>
    HttpResponse.json<CostBreakdown>({
      total_cost_usd: 2.018,
      by_provider: [
        { provider: "anthropic", model: "claude-sonnet-4", input_cost_usd: 0.735, output_cost_usd: 1.23, total_cost_usd: 1.965 },
        { provider: "anthropic", model: "claude-haiku-3.5", input_cost_usd: 0.022, output_cost_usd: 0.031, total_cost_usd: 0.053 },
      ],
      by_tier: [
        { tier: 1, label: "Agent Booster (WASM)", request_count: 1240, total_cost_usd: 0 },
        { tier: 2, label: "Haiku", request_count: 318, total_cost_usd: 0.053 },
        { tier: 3, label: "Sonnet/Opus", request_count: 142, total_cost_usd: 1.965 },
      ],
    }),
  ),
  http.get("/api/monitoring/pipeline-runs", () =>
    HttpResponse.json<PipelineRun[]>([
      { id: "run-001", session_key: "sess-abc-123", model: "claude-sonnet-4", complexity: 0.72, latency_ms: 3200, status: "success", timestamp: "2026-02-24T10:30:00Z" },
      { id: "run-002", session_key: "sess-def-456", model: "claude-haiku-3.5", complexity: 0.18, latency_ms: 480, status: "success", timestamp: "2026-02-24T10:31:00Z" },
      { id: "run-003", session_key: "sess-abc-123", model: "agent-booster", complexity: 0.05, latency_ms: 2, status: "success", timestamp: "2026-02-24T10:31:30Z" },
      { id: "run-004", session_key: "sess-ghi-789", model: "claude-sonnet-4", complexity: 0.88, latency_ms: 5000, status: "error", timestamp: "2026-02-24T10:32:00Z" },
      { id: "run-005", session_key: "sess-def-456", model: "claude-haiku-3.5", complexity: 0.22, latency_ms: 620, status: "success", timestamp: "2026-02-24T10:33:00Z" },
    ]),
  ),

  // Voice
  http.get("/api/voice/status", () =>
    HttpResponse.json({
      state: "idle" as const,
      talkModeActive: false,
      wakeWordEnabled: false,
      settings: {
        enabled: false,
        wakeWordEnabled: false,
        language: "en",
        echoCancel: true,
        noiseSuppression: true,
        pushToTalk: false,
      } satisfies VoiceSettingsData,
    }),
  ),
  // WEFT-218: pipeline state push (real daemon broadcasts; MSW acks only)
  http.post("/api/voice/status", async ({ request }) => {
    const body = (await request.json().catch(() => ({}))) as {
      state?: string;
      talkModeActive?: boolean;
    };
    return HttpResponse.json({
      success: true,
      state: body.state ?? "idle",
      talkModeActive: body.talkModeActive ?? false,
      wakeWordEnabled: false,
    });
  }),
  http.put("/api/voice/settings", () =>
    HttpResponse.json({ success: true }),
  ),
  http.post("/api/voice/test-mic", () =>
    HttpResponse.json({ success: true, level: 0.6 }),
  ),
  http.post("/api/voice/test-speaker", () =>
    HttpResponse.json({ success: true }),
  ),
];
