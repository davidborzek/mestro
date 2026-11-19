import { expect, test } from "bun:test";
import { backoffMs, KioskSocket, type KioskSocketOptions } from "./ws";
import { pairStatus } from "./pairing";

test("backoffMs doubles from the base delay", () => {
  expect(backoffMs(0, () => 0.5)).toBe(500);
  expect(backoffMs(1, () => 0.5)).toBe(1000);
  expect(backoffMs(2, () => 0.5)).toBe(2000);
});

test("backoffMs caps at the maximum delay", () => {
  expect(backoffMs(10, () => 0.5)).toBe(8000);
  expect(backoffMs(50, () => 0.5)).toBe(8000);
});

test("backoffMs jitters ±25 % around the base", () => {
  expect(backoffMs(0, () => 0)).toBe(375);
  expect(backoffMs(0, () => 1)).toBe(625);
});

class FakeWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  readyState = FakeWebSocket.CONNECTING;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  constructor(public url: string) {}
  open() {
    this.readyState = FakeWebSocket.OPEN;
    this.onopen?.();
  }
  close() {
    this.readyState = 3;
    this.onclose?.();
  }
  send(_raw?: unknown) {}
}

test("onOpen fires on every successful connect, reconnects included", () => {
  const real = globalThis.WebSocket;
  const sockets: FakeWebSocket[] = [];
  const factory = class extends FakeWebSocket {
    constructor(url: string) {
      super(url);
      sockets.push(this);
    }
  };
  (globalThis as { WebSocket: unknown }).WebSocket = factory;
  Object.defineProperty(globalThis.WebSocket, "CONNECTING", { value: 0 });

  const kiosk = new KioskSocket("ws://kiosk/ws");
  try {
    let opens = 0;
    kiosk.onOpen(() => {
      opens++;
    });
    kiosk.connect();
    sockets[0].open();
    expect(opens).toBe(1);

    // the connection drops and the retry loop dials again
    sockets[0].close();
    kiosk.connect();
    sockets[1].open();
    expect(opens).toBe(2);
  } finally {
    kiosk.close();
    globalThis.WebSocket = real;
  }
});

test("settings push frames reach the settings handler only", () => {
  const real = globalThis.WebSocket;
  const sockets: FakeWebSocket[] = [];
  const factory = class extends FakeWebSocket {
    constructor(url: string) {
      super(url);
      sockets.push(this);
    }
  };
  (globalThis as { WebSocket: unknown }).WebSocket = factory;
  Object.defineProperty(globalThis.WebSocket, "CONNECTING", { value: 0 });

  const kiosk = new KioskSocket("ws://kiosk/ws");
  try {
    let settingsPushes = 0;
    let statePushes = 0;
    kiosk.onSettingsPush(() => settingsPushes++);
    kiosk.onPush(() => statePushes++);
    kiosk.connect();
    sockets[0].open();

    sockets[0].onmessage?.({ data: JSON.stringify({ event: "settings" }) });
    expect(settingsPushes).toBe(1);
    expect(statePushes).toBe(0);

    sockets[0].onmessage?.({
      data: JSON.stringify({
        event: "state",
        data: { connected: true, player: {}, view: {}, display: {} },
      }),
    });
    expect(settingsPushes).toBe(1);
    expect(statePushes).toBe(1);
  } finally {
    kiosk.close();
    globalThis.WebSocket = real;
  }
});

// ── the iOS lifecycle probes + the retry gate ──
// every test below swaps the global WebSocket for a recording
// factory and drives the socket with a fake clock

class RecordingSocket extends FakeWebSocket {
  static installed: RecordingSocket[] = [];
  sent: string[] = [];
  constructor(url: string) {
    super(url);
    RecordingSocket.installed.push(this);
  }
  send(raw: unknown) {
    this.sent.push(String(raw));
  }
}

function withFakeSockets(
  options: KioskSocketOptions | undefined,
  run: (
    kiosk: KioskSocket,
    sockets: () => RecordingSocket[],
    clock: { now: number },
  ) => void | Promise<void>,
): void {
  void withFakeSocketsAsync(options, run);
}

async function withFakeSocketsAsync(
  options: KioskSocketOptions | undefined,
  run: (
    kiosk: KioskSocket,
    sockets: () => RecordingSocket[],
    clock: { now: number },
  ) => void | Promise<void>,
): Promise<void> {
  const real = globalThis.WebSocket;
  RecordingSocket.installed = [];
  (globalThis as { WebSocket: unknown }).WebSocket = RecordingSocket;
  Object.defineProperty(globalThis.WebSocket, "CONNECTING", { value: 0 });
  const kiosk = new KioskSocket("ws://kiosk/ws", options);
  try {
    await run(kiosk, () => RecordingSocket.installed, { now: 0 });
  } finally {
    kiosk.close();
    globalThis.WebSocket = real;
  }
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const waitFor = async (check: () => boolean, ms = 2000): Promise<void> => {
  for (let waited = 0; !check() && waited < ms; waited += 5) await sleep(5);
};

test("resumed drops a stale socket and redials at once — no backoff", () => {
  let now = 0;
  withFakeSockets({ now: () => now, staleMs: 25_000, probeMs: 1000 }, (kiosk, sockets) => {
    kiosk.connect();
    sockets()[0].open();
    now = 30_000; // silent for 30 s (a backgrounded app)
    kiosk.resumed();
    // the corpse is closed and a new dial is out immediately
    expect(sockets()[0].readyState).toBe(3);
    expect(sockets()).toHaveLength(2);
  });
});

test("resumed keeps a fresh socket and redials mid-backoff", () => {
  let now = 0;
  withFakeSockets({ now: () => now, staleMs: 25_000, probeMs: 1000 }, (kiosk, sockets) => {
    kiosk.connect();
    sockets()[0].open();
    kiosk.resumed();
    expect(sockets()).toHaveLength(1);

    // the connection drops: a retry is scheduled with backoff —
    // coming back to the foreground first dials at once instead
    sockets()[0].close();
    kiosk.resumed();
    expect(sockets()).toHaveLength(2);
  });
});

test("the keepalive pings a quiet socket and measures the round-trip", async () => {
  let now = 0;
  let rtt: number | null = null;
  await withFakeSocketsAsync(
    { now: () => now, probeMs: 1, keepaliveMs: 20_000 },
    async (kiosk, sockets) => {
      kiosk.onKeepalive((ms) => (rtt = ms));
      kiosk.connect();
      sockets()[0].open();
      now = 20_000; // idle: the probe sends the hello
      await waitFor(() => sockets()[0].sent.some((raw) => raw.includes('"op":"hello"')));
      now = 23_000;
      sockets()[0].onmessage?.({ data: JSON.stringify({ id: 1, ok: true }) });
      expect(rtt).toBe(3000);
      // alive: no redial
      await sleep(20);
      expect(sockets()).toHaveLength(1);
    },
  );
});

test("a keepalive without a reply treats the socket as dead and redials", async () => {
  let now = 0;
  await withFakeSocketsAsync(
    { now: () => now, probeMs: 1, keepaliveMs: 20_000, replyBudgetMs: 5000 },
    async (kiosk, sockets) => {
      kiosk.connect();
      sockets()[0].open();
      now = 20_000; // the hello goes out
      await waitFor(() => sockets()[0].sent.some((raw) => raw.includes('"op":"hello"')));
      now = 26_000; // 6 s without any reply
      await waitFor(() => sockets().length === 2);
      expect(sockets()[0].readyState).toBe(3);
    },
  );
});

test("a request issued mid-backoff redials at once", async () => {
  withFakeSockets({ probeMs: 1000 }, (kiosk, sockets) => {
    kiosk.connect();
    sockets()[0].open();
    sockets()[0].close(); // retry scheduled, ~500 ms out
    void kiosk.send({ op: "hello" }).catch(() => {});
    expect(sockets()).toHaveLength(2);
  });
});

test("a reconnect counts and onOpen fires again", () => {
  withFakeSockets({ probeMs: 1000 }, (kiosk, sockets) => {
    let opens = 0;
    kiosk.onOpen(() => opens++);
    kiosk.connect();
    sockets()[0].open();
    sockets()[0].close();
    kiosk.connect();
    sockets()[1].open();
    expect(kiosk.reconnectCount).toBe(1);
    expect(opens).toBe(2);
  });
});

test("a retry gate rejecting the pairing stops the reconnect loop", async () => {
  withFakeSockets({ probeMs: 1000 }, async (kiosk, sockets) => {
    const statuses: string[] = [];
    kiosk.onStatus((status) => statuses.push(status));
    kiosk.setRetryGate(async () => false);
    kiosk.connect();
    sockets()[0].open();
    sockets()[0].close();
    await waitFor(() => statuses.includes("closed"));
    await sleep(50);
    // the loop stopped: no second dial, no more backoff
    expect(sockets()).toHaveLength(1);
    expect(kiosk.connected).toBe(false);
  });
});

test("the real gate: pairStatus with a fake fetch unpauses the loop to a stop", async () => {
  withFakeSockets({ probeMs: 1000 }, async (kiosk, sockets) => {
    const fakeFetch = async () =>
      ({ ok: true, json: async () => ({ paired: false }) }) as unknown as Response;
    const statuses: string[] = [];
    kiosk.onStatus((status) => statuses.push(status));
    kiosk.setRetryGate(async () => (await pairStatus(fakeFetch)) !== false);
    kiosk.connect();
    sockets()[0].open();
    sockets()[0].close();
    await waitFor(() => statuses.includes("closed"));
    await sleep(50);
    expect(sockets()).toHaveLength(1);
    // the tri-state probe: rejected, unreachable, accepted
    expect(await pairStatus(fakeFetch)).toBe(false);
    expect(
      await pairStatus(async () => {
        throw new Error("down");
      }),
    ).toBe(null);
    expect(
      await pairStatus(
        async () => ({ ok: true, json: async () => ({ paired: true }) }) as unknown as Response,
      ),
    ).toBe(true);
  });
});
