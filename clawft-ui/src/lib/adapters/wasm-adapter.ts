/**
 * WasmAdapter -- BackendAdapter implementation for in-browser WASM mode.
 *
 * Uses dynamic import to load the wasm-bindgen JS glue, then calls
 * init/send_message/set_env on the WASM module. Limited capabilities
 * compared to AxumAdapter: no channels, cron, delegation, multiUser,
 * skillInstall, no realtime WS. Sessions are stored in-memory. A single
 * "browser-agent" agent is always present.
 */

import type {
  BackendAdapter,
  BackendCapabilities,
  BackendMode,
  ChatMessage,
  AgentInfo,
  SessionInfo,
  ToolInfo,
  MemoryEntry,
} from "../backend-adapter.ts";

// ---------------------------------------------------------------------------
// WASM module interface (wasm-bindgen exports)
// ---------------------------------------------------------------------------

interface ClawftWasm {
  init(config_json: string): Promise<void>;
  send_message(text: string): Promise<string>;
  set_env(key: string, value: string): void;
  /** WEFT-307: JSON-Schema introspection for a registered tool. */
  tool_schema?(slug: string): string;
  /** WEFT-307: list of registered tool names. */
  tool_list?(): string;
}

// ---------------------------------------------------------------------------
// WasmAdapter
// ---------------------------------------------------------------------------

export class WasmAdapter implements BackendAdapter {
  readonly mode: BackendMode = "wasm";
  readonly capabilities: BackendCapabilities = {
    channels: false,
    cron: false,
    delegation: false,
    multiUser: false,
    skillInstall: false,
    realtime: false,
    monitoring: false,
    ready: false,
  };

  private wasm: ClawftWasm | null = null;
  private messageCallbacks: Array<(msg: ChatMessage) => void> = [];
  private sessions: Map<string, ChatMessage[]> = new Map();
  private memoryStore: Map<string, MemoryEntry> = new Map();
  private config: Record<string, unknown> = {};
  private wasmUrl: string;
  private onProgress?: (phase: string, pct: number) => void;

  constructor(
    wasmUrl = "/clawft_wasm.js",
    onProgress?: (phase: string, pct: number) => void,
  ) {
    this.wasmUrl = wasmUrl;
    this.onProgress = onProgress;
  }

  async init(config?: Record<string, unknown>): Promise<void> {
    this.onProgress?.("download", 0);

    // Dynamic import of wasm-bindgen generated JS
    const wasmModule = await import(
      /* @vite-ignore */ this.wasmUrl
    ) as ClawftWasm & { default: () => Promise<void> };

    this.onProgress?.("compile", 30);
    await wasmModule.default(); // Loads and compiles .wasm binary

    this.onProgress?.("init", 70);
    this.wasm = wasmModule;

    if (config) {
      this.config = config;
      await this.wasm.init(JSON.stringify(config));
    }

    this.onProgress?.("ready", 100);
    (this.capabilities as { ready: boolean }).ready = true;
  }

  async dispose(): Promise<void> {
    this.wasm = null;
    this.messageCallbacks = [];
    (this.capabilities as { ready: boolean }).ready = false;
  }

  // -- Agents (single browser-agent) --

  async listAgents(): Promise<AgentInfo[]> {
    return [
      {
        id: "browser-agent",
        name: "Browser Agent",
        status: this.capabilities.ready ? "running" : "stopped",
        model:
          (
            this.config as Record<string, Record<string, string>>
          )?.defaults?.model ?? "unknown",
      },
    ];
  }

  async getAgent(id: string): Promise<AgentInfo | null> {
    const agents = await this.listAgents();
    return agents.find((a) => a.id === id) ?? null;
  }

  async startAgent(): Promise<void> {
    // Agent is always running in WASM mode once initialized
  }

  async stopAgent(): Promise<void> {
    // Cannot stop the agent in WASM mode (it is the runtime)
  }

  // -- Sessions (in-memory) --

  async listSessions(): Promise<SessionInfo[]> {
    return Array.from(this.sessions.entries()).map(([key, msgs]) => ({
      key,
      messageCount: msgs.length,
      createdAt: msgs[0]?.timestamp ?? new Date().toISOString(),
      updatedAt:
        msgs[msgs.length - 1]?.timestamp ?? new Date().toISOString(),
    }));
  }

  async getSessionMessages(key: string): Promise<ChatMessage[]> {
    return this.sessions.get(key) ?? [];
  }

  async deleteSession(key: string): Promise<void> {
    this.sessions.delete(key);
  }

  // -- Chat (primary interaction) --

  async sendMessage(
    sessionKey: string,
    content: string,
  ): Promise<ChatMessage> {
    if (!this.wasm) throw new Error("WASM module not initialized");

    const userMsg: ChatMessage = {
      role: "user",
      content,
      timestamp: new Date().toISOString(),
    };

    // Store user message in local session
    const messages = this.sessions.get(sessionKey) ?? [];
    messages.push(userMsg);
    this.sessions.set(sessionKey, messages);

    // Send through WASM pipeline
    const responseText = await this.wasm.send_message(content);

    const assistantMsg: ChatMessage = {
      role: "assistant",
      content: responseText,
      timestamp: new Date().toISOString(),
    };

    messages.push(assistantMsg);
    this.messageCallbacks.forEach((cb) => cb(assistantMsg));

    return assistantMsg;
  }

  onMessage(callback: (msg: ChatMessage) => void): () => void {
    this.messageCallbacks.push(callback);
    return () => {
      this.messageCallbacks = this.messageCallbacks.filter(
        (cb) => cb !== callback,
      );
    };
  }

  // -- Tools (browser-safe subset) --

  async listTools(): Promise<ToolInfo[]> {
    return [
      { name: "read_file", description: "Read file from OPFS workspace" },
      { name: "write_file", description: "Write file to OPFS workspace" },
      { name: "edit_file", description: "Edit file in OPFS workspace" },
      {
        name: "list_directory",
        description: "List OPFS directory contents",
      },
      { name: "web_search", description: "Search the web" },
      { name: "web_fetch", description: "Fetch URL content" },
      { name: "memory_read", description: "Read from memory store" },
      { name: "memory_write", description: "Write to memory store" },
    ];
  }

  async getToolSchema(name: string): Promise<Record<string, unknown> | null> {
    // WEFT-307: clawft-wasm exposes a `tool_schema(slug)` entry point
    // that returns either `null` or a JSON object with `name`,
    // `description`, and `parameters` (JSON-Schema). Fall back to
    // returning `null` when the runtime isn't initialized yet or the
    // build predates the entry point.
    if (!this.wasm || typeof this.wasm.tool_schema !== "function") {
      return null;
    }
    try {
      const raw = this.wasm.tool_schema(name);
      const parsed = JSON.parse(raw) as Record<string, unknown> | null;
      return parsed;
    } catch {
      return null;
    }
  }

  // -- Memory (in-memory store in WASM mode) --

  async listMemory(namespace?: string): Promise<MemoryEntry[]> {
    const entries = Array.from(this.memoryStore.values());
    if (namespace) {
      return entries.filter((e) => e.namespace === namespace);
    }
    return entries;
  }

  async searchMemory(
    query: string,
    namespace?: string,
  ): Promise<MemoryEntry[]> {
    const all = await this.listMemory(namespace);
    const q = query.toLowerCase();
    return all.filter(
      (e) =>
        e.key.toLowerCase().includes(q) ||
        e.value.toLowerCase().includes(q) ||
        e.tags.some((t) => t.toLowerCase().includes(q)),
    );
  }

  async writeMemory(
    key: string,
    value: string,
    namespace = "default",
    tags: string[] = [],
  ): Promise<void> {
    const now = new Date().toISOString();
    this.memoryStore.set(key, {
      key,
      value,
      namespace,
      tags,
      createdAt: this.memoryStore.get(key)?.createdAt ?? now,
      updatedAt: now,
    });
  }

  async deleteMemory(key: string): Promise<void> {
    this.memoryStore.delete(key);
  }

  // -- Config --

  async getConfig(): Promise<Record<string, unknown>> {
    return this.config;
  }
}
