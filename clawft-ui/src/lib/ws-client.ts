type MessageHandler = (data: unknown) => void;

export class WsClient {
  private ws: WebSocket | null = null;
  private url: string;
  private handlers: Map<string, Set<MessageHandler>> = new Map();
  private reconnectDelay = 1000;
  private maxReconnectDelay = 30000;
  private shouldReconnect = true;

  constructor(url?: string) {
    this.url = url || `${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/ws`;
  }

  connect() {
    this.shouldReconnect = true;
    this.ws = new WebSocket(this.url);

    this.ws.onopen = () => {
      this.reconnectDelay = 1000;
      this.emit('connected', {});
    };

    this.ws.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data);
        const type = data.type || 'message';
        // WEFT-300: auto-reply to server-initiated heartbeat pings
        // before fanning out to user handlers. The server evicts a
        // socket that fails to send a pong within HEARTBEAT_TIMEOUT
        // (60s in production); replying inline keeps long-lived
        // dashboards alive without each handler having to know.
        if (type === 'ping') {
          this.send({ type: 'pong' });
        }
        this.emit(type, data);
        this.emit('*', data);
      } catch {
        this.emit('raw', event.data);
      }
    };

    this.ws.onclose = () => {
      this.emit('disconnected', {});
      if (this.shouldReconnect) {
        setTimeout(() => this.connect(), this.reconnectDelay);
        this.reconnectDelay = Math.min(this.reconnectDelay * 2, this.maxReconnectDelay);
      }
    };

    this.ws.onerror = () => {
      this.ws?.close();
    };
  }

  disconnect() {
    this.shouldReconnect = false;
    this.ws?.close();
  }

  send(data: unknown) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(data));
    }
  }

  subscribe(topic: string) {
    this.send({ type: 'subscribe', topic });
  }

  unsubscribe(topic: string) {
    this.send({ type: 'unsubscribe', topic });
  }

  on(event: string, handler: MessageHandler) {
    if (!this.handlers.has(event)) {
      this.handlers.set(event, new Set());
    }
    this.handlers.get(event)!.add(handler);
    return () => this.handlers.get(event)?.delete(handler);
  }

  private emit(event: string, data: unknown) {
    this.handlers.get(event)?.forEach((h) => h(data));
  }
}

export const wsClient = new WsClient();
