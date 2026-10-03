/**
 * AxumAdapter -- BackendAdapter implementation for the Axum REST/WS backend.
 *
 * Wraps the existing api-client.ts fetch wrapper and ws-client.ts WebSocket
 * client behind the BackendAdapter interface. All capabilities are enabled.
 */

import type {
  BackendAdapter,
  BackendCapabilities,
  BackendMode,
  AgentInfo,
  SessionInfo,
  ChatMessage,
  ToolInfo,
  MemoryEntry,
} from "../backend-adapter.ts";
import { api, setAuthToken } from "../api-client.ts";
import { WsClient } from "../ws-client.ts";

export class AxumAdapter implements BackendAdapter {
  readonly mode: BackendMode = "axum";
  readonly capabilities: BackendCapabilities = {
    channels: true,
    cron: true,
    delegation: true,
    multiUser: true,
    skillInstall: true,
    realtime: true,
    monitoring: true,
    ready: false,
  };

  private ws: WsClient;
  private messageCallbacks: Array<(msg: ChatMessage) => void> = [];
  private eventCallbacks: Array<
    (event: { topic: string; data: unknown }) => void
  > = [];

  constructor(
    _apiUrl: string,
    wsUrl: string,
    token?: string,
  ) {
    if (token) {
      setAuthToken(token);
    }
    this.ws = new WsClient(wsUrl);
  }

  async init(): Promise<void> {
    // The gateway no longer mints tokens (ADR-102 D5). A token reaches the
    // dashboard through the constructor or the `#token=` URL fragment that
    // `weft ui` and `weft token issue` print; `use-auth.ts` consumes it.
    // Without one, API calls fail with 401.

    // Connect WebSocket
    this.ws.connect();

    // Wire WS message events to our callbacks
    this.ws.on("chat:message", (data: unknown) => {
      const msg = data as ChatMessage;
      this.messageCallbacks.forEach((cb) => cb(msg));
    });

    this.ws.on("*", (data: unknown) => {
      const event = data as { type?: string };
      if (event.type) {
        this.eventCallbacks.forEach((cb) =>
          cb({ topic: event.type!, data }),
        );
      }
    });

    (this.capabilities as { ready: boolean }).ready = true;
  }

  async dispose(): Promise<void> {
    this.ws.disconnect();
    this.messageCallbacks = [];
    this.eventCallbacks = [];
    (this.capabilities as { ready: boolean }).ready = false;
  }

  // -- Agents --

  async listAgents(): Promise<AgentInfo[]> {
    const agents = await api.agents.list();
    return agents.map((a) => ({
      id: a.id,
      name: a.name,
      status: a.status,
      model: a.model ?? "unknown",
    }));
  }

  async getAgent(id: string): Promise<AgentInfo | null> {
    try {
      const a = await api.agents.get(id);
      return {
        id: a.id,
        name: a.name,
        status: a.status,
        model: a.model ?? "unknown",
      };
    } catch {
      return null;
    }
  }

  async startAgent(id: string): Promise<void> {
    await api.agents.start(id);
  }

  async stopAgent(id: string): Promise<void> {
    await api.agents.stop(id);
  }

  // -- Sessions --

  async listSessions(): Promise<SessionInfo[]> {
    const sessions = await api.sessions.list();
    return sessions.map((s) => ({
      key: s.key,
      messageCount: s.message_count,
      createdAt: s.updated_at,
      updatedAt: s.updated_at,
    }));
  }

  async getSessionMessages(key: string): Promise<ChatMessage[]> {
    const detail = await api.sessions.get(key);
    return detail.messages.map((m) => ({
      role: m.role,
      content: m.content,
      timestamp: m.timestamp,
      toolCalls: m.tool_calls?.map((tc) => ({
        toolName: tc.name,
        arguments: JSON.parse(tc.arguments || "{}") as Record<string, unknown>,
        result: tc.result,
      })),
    }));
  }

  async deleteSession(key: string): Promise<void> {
    await api.sessions.delete(key);
  }

  // -- Chat --

  async sendMessage(sessionKey: string, content: string): Promise<ChatMessage> {
    const msg = await api.chat.send(sessionKey, content);
    return {
      role: msg.role,
      content: msg.content,
      timestamp: msg.timestamp,
      toolCalls: msg.tool_calls?.map((tc) => ({
        toolName: tc.name,
        arguments: JSON.parse(tc.arguments || "{}") as Record<string, unknown>,
        result: tc.result,
      })),
    };
  }

  onMessage(callback: (msg: ChatMessage) => void): () => void {
    this.messageCallbacks.push(callback);
    return () => {
      this.messageCallbacks = this.messageCallbacks.filter(
        (cb) => cb !== callback,
      );
    };
  }

  // -- Tools --

  async listTools(): Promise<ToolInfo[]> {
    const tools = await api.tools.list();
    return tools.map((t) => ({
      name: t.name,
      description: t.description,
    }));
  }

  async getToolSchema(name: string): Promise<Record<string, unknown> | null> {
    try {
      return await api.tools.schema(name);
    } catch {
      return null;
    }
  }

  // -- Memory --

  async listMemory(): Promise<MemoryEntry[]> {
    const entries = await api.memory.list();
    return entries.map((e) => ({
      key: e.key,
      value: e.value,
      namespace: e.namespace,
      tags: e.tags,
      createdAt: e.updated_at,
      updatedAt: e.updated_at,
      similarity: e.similarity,
    }));
  }

  async searchMemory(
    query: string,
    _namespace?: string,
    threshold = 0.7,
  ): Promise<MemoryEntry[]> {
    const entries = await api.memory.search(query, threshold);
    return entries.map((e) => ({
      key: e.key,
      value: e.value,
      namespace: e.namespace,
      tags: e.tags,
      createdAt: e.updated_at,
      updatedAt: e.updated_at,
      similarity: e.similarity,
    }));
  }

  async writeMemory(
    key: string,
    value: string,
    namespace = "default",
    tags: string[] = [],
  ): Promise<void> {
    await api.memory.create({ key, value, namespace, tags });
  }

  async deleteMemory(key: string): Promise<void> {
    await api.memory.delete(key);
  }

  // -- Config --

  async getConfig(): Promise<Record<string, unknown>> {
    return (await api.config.get()) as unknown as Record<string, unknown>;
  }

  async updateConfig(patch: Record<string, unknown>): Promise<void> {
    const current = await api.config.get();
    const merged = { ...current, ...patch };
    await api.config.save(merged);
  }

  // -- Events --

  subscribe(topics: string[]): void {
    for (const topic of topics) {
      this.ws.subscribe(topic);
    }
  }

  unsubscribe(topics: string[]): void {
    for (const topic of topics) {
      this.ws.unsubscribe(topic);
    }
  }

  onEvent(
    callback: (event: { topic: string; data: unknown }) => void,
  ): () => void {
    this.eventCallbacks.push(callback);
    return () => {
      this.eventCallbacks = this.eventCallbacks.filter(
        (cb) => cb !== callback,
      );
    };
  }
}
