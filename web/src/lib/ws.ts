import type { Reply, Request } from "./protocol";

// one WS connection to mestro, request/response by id. The auth
// secret rides the URL hash (`#pair=…`) and goes into the query —
// a plain HTTP log never sees it. `send` transparently reconnects:
// the app restarts once at the end of the wizard (connection
// settings are restart-class), so requests just wait it out.

type Pending = {
  resolve: (data: unknown) => void;
  reject: (error: Error) => void;
};

export class WizardSocket {
  private ws: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private connecting: Promise<void> | null = null;

  constructor(private readonly url: string) {}

  connect(): Promise<void> {
    if (this.connected) return Promise.resolve();
    if (this.connecting) return this.connecting;
    this.connecting = new Promise((resolve, reject) => {
      const ws = new WebSocket(this.url);
      this.ws = ws;
      ws.onopen = () => {
        this.connecting = null;
        resolve();
      };
      ws.onerror = () => {
        this.connecting = null;
        reject(new Error("Verbindung fehlgeschlagen"));
      };
      ws.onclose = () => {
        this.ws = null;
        this.connecting = null;
        for (const { reject: fail } of this.pending.values()) {
          fail(new Error("Verbindung getrennt"));
        }
        this.pending.clear();
      };
      ws.onmessage = (event) => {
        let reply: Reply;
        try {
          reply = JSON.parse(event.data);
        } catch {
          return;
        }
        const waiter = this.pending.get(reply.id);
        if (!waiter) return;
        this.pending.delete(reply.id);
        if (reply.ok) waiter.resolve(reply.data);
        else waiter.reject(new Error(reply.error ?? "unbekannter Fehler"));
      };
    });
    return this.connecting;
  }

  get connected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  async send<T>(request: Request): Promise<T> {
    await this.connect();
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      this.pending.set(id, {
        resolve: (data) => resolve(data as T),
        reject,
      });
      this.ws!.send(JSON.stringify({ id, ...request }));
    });
  }
}

export function wizardUrl(): string {
  const pair = location.hash.match(/pair=([A-Z0-9]+)/i)?.[1] ?? "";
  return `ws://${location.host}/ws${pair ? `?pair=${pair}` : ""}`;
}
