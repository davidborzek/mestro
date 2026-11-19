// one WS connection to mestro with auto-reconnect: requests are
// matched by id, push frames (no id) go to the push handlers. The
// auth cookie from POST /pair rides along automatically — the URL
// carries no secret.
//
// iOS specifics: WebSockets die silently in the background and
// after an unlock, and the first command afterwards would wait on
// a corpse. `resumed()` (wired to visibility/page-show/online by
// `attachLifecycle`) drops a stale socket at once, and a probe
// keepalive detects a gone socket while the app is visible. A
// dropped pairing (a factory reset) shows up only as a rejected
// handshake — the retry gate probes /pair/status and stops the
// loop so the app can switch to the pair screen.

import type { PushFrame, Reply, RemoteState, Request } from "./protocol";

export type SocketStatus = "idle" | "connecting" | "open" | "reconnecting" | "closed";

type Pending = {
  resolve: (data: unknown) => void;
  reject: (error: Error) => void;
};

type StatusHandler = (status: SocketStatus) => void;
type PushHandler = (state: RemoteState) => void;
type SettingsPushHandler = () => void;
type OpenHandler = () => void;
type KeepaliveHandler = (rttMs: number) => void;

// 500 ms after the first drop, doubling up to 8 s, ±25 % jitter so
// a fleet of phones does not reconnect in lockstep
export function backoffMs(attempt: number, rng = Math.random): number {
  const base = Math.min(500 * 2 ** attempt, 8000);
  return Math.round(base * (0.75 + rng() * 0.5));
}

// injectable clock + visibility, and the probe/keepalive cadence —
// the tests drive all of it with fakes
export interface KioskSocketOptions {
  now?: () => number;
  visibility?: () => string;
  probeMs?: number;
  keepaliveMs?: number;
  replyBudgetMs?: number;
  staleMs?: number;
}

const DEFAULTS = {
  probeMs: 1000,
  keepaliveMs: 20_000,
  replyBudgetMs: 5000,
  staleMs: 25_000,
};

export class KioskSocket {
  private ws: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private statusHandlers = new Set<StatusHandler>();
  private pushHandlers = new Set<PushHandler>();
  private settingsPushHandlers = new Set<SettingsPushHandler>();
  private openHandlers = new Set<OpenHandler>();
  private keepaliveHandlers = new Set<KeepaliveHandler>();
  private attempt = 0;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private closing = false;
  private status: SocketStatus = "idle";
  private retryGate: (() => Promise<boolean>) | null = null;
  // the liveness bookkeeping (the injected clock stamps it)
  private lastMessageAt = 0;
  private helloAt: number | null = null;
  private everOpen = false;
  reconnectCount = 0;
  // an immediate redial in flight: onclose must not schedule the
  // backoff on top of it
  private reviving = false;
  private readonly now: () => number;
  private readonly visibility: () => string;
  private readonly probeMs: number;
  private readonly keepaliveMs: number;
  private readonly replyBudgetMs: number;
  private readonly staleMs: number;
  private probeTimer: ReturnType<typeof setInterval> | null = null;

  constructor(
    private readonly url: string,
    options: KioskSocketOptions = {},
  ) {
    this.now = options.now ?? (() => Date.now());
    this.visibility =
      options.visibility ??
      (() => (typeof document === "undefined" ? "visible" : document.visibilityState));
    this.probeMs = options.probeMs ?? DEFAULTS.probeMs;
    this.keepaliveMs = options.keepaliveMs ?? DEFAULTS.keepaliveMs;
    this.replyBudgetMs = options.replyBudgetMs ?? DEFAULTS.replyBudgetMs;
    this.staleMs = options.staleMs ?? DEFAULTS.staleMs;
  }

  onStatus(handler: StatusHandler): () => void {
    this.statusHandlers.add(handler);
    handler(this.status);
    return () => this.statusHandlers.delete(handler);
  }

  onPush(handler: PushHandler): () => void {
    this.pushHandlers.add(handler);
    return () => this.pushHandlers.delete(handler);
  }

  // settings pushes carry no payload — the page refetches state
  // (and the schema, whose VU section follows the cycle)
  onSettingsPush(handler: SettingsPushHandler): () => void {
    this.settingsPushHandlers.add(handler);
    return () => this.settingsPushHandlers.delete(handler);
  }

  // runs `handler` on every transition into "open" — the initial
  // connect and every reconnect, so callers can repeat work a
  // fresh server side requires (the device handshake)
  onOpen(handler: OpenHandler): () => void {
    this.openHandlers.add(handler);
    return () => this.openHandlers.delete(handler);
  }

  // the keepalive's round-trip — the Diagnose row shows it
  onKeepalive(handler: KeepaliveHandler): () => void {
    this.keepaliveHandlers.add(handler);
    return () => this.keepaliveHandlers.delete(handler);
  }

  // consulted before every retry: returning false stops the loop
  // (the pairing gate is gone — the app switches to the pair
  // screen instead of spinning forever)
  setRetryGate(gate: () => Promise<boolean>): void {
    this.retryGate = gate;
  }

  private setStatus(status: SocketStatus) {
    this.status = status;
    for (const handler of this.statusHandlers) handler(status);
  }

  connect(): void {
    if (this.closing) return;
    if (this.connected) return;
    if (this.ws && this.ws.readyState === WebSocket.CONNECTING) return;
    this.ensureProbe();
    this.setStatus(this.attempt === 0 ? "connecting" : "reconnecting");
    const ws = new WebSocket(this.url);
    this.ws = ws;
    ws.onopen = () => {
      if (this.everOpen) this.reconnectCount += 1;
      this.everOpen = true;
      this.lastMessageAt = this.now();
      this.helloAt = null;
      this.attempt = 0;
      this.setStatus("open");
      for (const handler of this.openHandlers) handler();
    };
    ws.onclose = () => {
      this.ws = null;
      for (const { reject: fail } of this.pending.values()) {
        fail(new Error("connection lost"));
      }
      this.pending.clear();
      this.helloAt = null;
      if (this.closing) {
        this.setStatus("closed");
        return;
      }
      if (this.reviving) {
        // an immediate redial: no backoff, dial now
        this.reviving = false;
        this.connect();
        return;
      }
      const attempt = this.attempt;
      this.retryTimer = setTimeout(() => void this.retry(attempt), backoffMs(attempt));
      this.attempt = attempt + 1;
    };
    ws.onerror = () => {
      // the close event follows; nothing to do here
    };
    ws.onmessage = (event) => this.receive(event.data);
  }

  // one scheduled retry: the gate may stop the loop before the
  // dial (a factory reset rejects the cookie at the handshake,
  // which only shows up as a closed socket)
  private async retry(attempt: number): Promise<void> {
    this.retryTimer = null;
    if (this.retryGate && !(await this.retryGate())) {
      this.attempt = 0;
      this.setStatus("closed");
      return;
    }
    this.connect();
  }

  // back in the foreground (unlock, tab switch, back online): a
  // socket that is not OPEN or has been quiet too long is dropped
  // and redialed at once — no backoff
  resumed(): void {
    if (this.closing) return;
    if (this.connected && this.now() - this.lastMessageAt <= this.staleMs) return;
    this.revive();
  }

  // drop a possibly-dead socket and dial again immediately. iOS
  // hands back sockets that are silently gone; waiting for their
  // close (or the backoff) stalls every command after an unlock
  private revive(): void {
    if (this.closing) return;
    if (this.retryTimer) {
      clearTimeout(this.retryTimer);
      this.retryTimer = null;
    }
    const ws = this.ws;
    if (!ws) {
      this.connect();
      return;
    }
    if (ws.readyState === WebSocket.CONNECTING) return;
    this.reviving = true;
    ws.close();
  }

  // the liveness probe, on its own cadence: while visible and
  // OPEN, a quiet socket gets a tiny hello every 20 s; a hello
  // that stays unanswered for 5 s means the socket is dead
  private ensureProbe(): void {
    if (this.probeTimer) return;
    this.probeTimer = setInterval(() => this.probe(), this.probeMs);
  }

  private probe(): void {
    if (this.closing || this.visibility() !== "visible") return;
    if (!this.connected) return;
    const now = this.now();
    if (this.helloAt === null) {
      if (now - this.lastMessageAt >= this.keepaliveMs) this.sendKeepalive();
    } else if (now - this.helloAt >= this.replyBudgetMs) {
      this.revive();
    }
  }

  private sendKeepalive(): void {
    const ws = this.ws;
    if (!ws) return;
    this.helloAt = this.now();
    const id = this.nextId++;
    this.pending.set(id, { resolve: () => {}, reject: () => {} });
    try {
      ws.send(JSON.stringify({ id, op: "hello" }));
    } catch {
      // sending on a corpse throws: treat it as dead
      this.revive();
    }
  }

  private receive(raw: unknown) {
    let frame: Reply | PushFrame;
    try {
      frame = JSON.parse(String(raw));
    } catch {
      return;
    }
    const now = this.now();
    if (this.helloAt !== null) {
      // any traffic proves liveness; the reply to the keepalive
      // measures the round-trip
      for (const handler of this.keepaliveHandlers) handler(now - this.helloAt);
      this.helloAt = null;
    }
    this.lastMessageAt = now;
    if ("event" in frame) {
      if (frame.event === "state") {
        for (const handler of this.pushHandlers) handler(frame.data);
      } else if (frame.event === "settings") {
        for (const handler of this.settingsPushHandlers) handler();
      }
      return;
    }
    const waiter = this.pending.get(frame.id);
    if (!waiter) return;
    this.pending.delete(frame.id);
    if (frame.ok) waiter.resolve(frame.data);
    else waiter.reject(new Error(frame.error ?? "unknown error"));
  }

  get connected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  async send<T>(request: Request): Promise<T> {
    this.connect();
    // a request issued mid-backoff redials at once — the command
    // must not wait out the timer
    if (!this.connected) this.revive();
    // a request issued while still reconnecting waits for the
    // socket instead of failing — the wizard relies on that
    await this.waitOpen();
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      this.pending.set(id, {
        resolve: (data) => resolve(data as T),
        reject,
      });
      this.ws!.send(JSON.stringify({ id, ...request }));
    });
  }

  private waitOpen(): Promise<void> {
    if (this.connected) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const off = this.onStatus((status) => {
        if (status === "open") {
          off();
          resolve();
        } else if (status === "closed") {
          off();
          reject(new Error("connection lost"));
        }
      });
    });
  }

  close(): void {
    this.closing = true;
    if (this.retryTimer) clearTimeout(this.retryTimer);
    if (this.probeTimer) {
      clearInterval(this.probeTimer);
      this.probeTimer = null;
    }
    this.ws?.close();
    this.ws = null;
  }
}

export function socketUrl(): string {
  return `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/ws`;
}

// the app lifecycle wiring for `resumed()`: returning to the
// foreground (visibilitychange, a persisted pageshow) and coming
// back online all check the socket at once. Call once, keep the
// returned teardown
export function attachLifecycle(socket: KioskSocket): () => void {
  if (typeof document === "undefined") return () => {};
  const visible = () => {
    if (document.visibilityState === "visible") socket.resumed();
  };
  const resumed = () => socket.resumed();
  const shown = (event: PageTransitionEvent) => {
    if (event.persisted) socket.resumed();
  };
  document.addEventListener("visibilitychange", visible);
  window.addEventListener("online", resumed);
  window.addEventListener("pageshow", shown);
  return () => {
    document.removeEventListener("visibilitychange", visible);
    window.removeEventListener("online", resumed);
    window.removeEventListener("pageshow", shown);
  };
}
