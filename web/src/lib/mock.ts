// dev-only fake kiosk (`?mock`): a WebSocket that answers the
// protocol and pushes RemoteState, plus /pair endpoints that
// always say "paired". Never part of a production build — main.ts
// imports this only behind import.meta.env.DEV.

import type { CtlCmd, RemoteState, Request, StateData, WifiNetwork } from "./protocol";
import { MOCK_DEFAULTS, MOCK_OVERRIDES, mockSchema } from "./mockSchema";
import { readPath } from "./settingsSchema";
import { setLocale } from "./i18n.svelte";

const COVER =
  "data:image/svg+xml," +
  encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512">
<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
<stop offset="0" stop-color="#6e3b1f"/><stop offset="1" stop-color="#241733"/>
</linearGradient></defs>
<rect width="512" height="512" fill="url(#g)"/>
<circle cx="150" cy="130" r="90" fill="#e8955a" opacity="0.35"/>
<circle cx="400" cy="380" r="140" fill="#4a2f66" opacity="0.6"/>
</svg>`,
  );

// dev-only query switches, so every screen state can be screenshotted:
//   ?mock&long        long title + artist
//   &state=off|ma|nothing
//   &vu               VU view active (with the skin row)
//   &pair             start unpaired
//   &ap               the setup hotspot: unpaired AP mode — /setup/wifi
//                     serves the captive portal (wifi ops, one open
//                     network, a failing join, busy on a second join)
//   &lang=de|en       force the locale (headless shots)
//   &pin              fill the pair input so the primary button
//                     shows its enabled state
// pages are opened by their path: /settings/connection,
// /settings/info, /settings/vu/night, …
const params = new URLSearchParams(location.search);
const LONG_TITLE = "Everything In Its Right Place (Live From The Basement, Extended Version)";
const LONG_ARTIST = "Tom Yorke, Jonny Greenwood and the London Contemporary Orchestra";

const MA_URL = "http://ma.local:8095";
const MA_PLAYERS = [
  { id: "p1", name: "Wohnzimmer" },
  { id: "p2", name: "Küche" },
  { id: "p3", name: "Terrasse" },
];
// `state=ma` plays a configured but unreachable server
const MA_DOWN = params.get("state") === "ma";

// the setup hotspot: unpaired AP mode, wifi ops served
const AP_MODE = params.has("ap");
let resetDone = false;
let joinBusy = false;
// Alt-Router always rejects — the failure path back to the list
const MOCK_NETWORKS: WifiNetwork[] = [
  { ssid: "HomeNet", signal: -52, secured: true },
  { ssid: "HomeNet-Gast", signal: -63, secured: false },
  { ssid: "Nachbars Netz", signal: -71, secured: true },
  { ssid: "Alt-Router", signal: -84, secured: true },
];

// the timezone picker: a short zoneinfo, the player runs in Berlin
const MOCK_ZONES = ["America/New_York", "Asia/Tokyo", "Europe/Berlin", "Europe/London", "UTC"];
let tzChosen = "";
let tzCurrent = "Europe/Berlin";

// the audio picker: three devices, the last one never starts
const BROKEN_OUTPUT = "hw:CARD=DAC2";
const MOCK_OUTPUTS = [
  { id: "hw:CARD=Headphones", name: "bcm2835 Headphones" },
  { id: "hw:CARD=S3", name: "Topping S3" },
  { id: BROKEN_OUTPUT, name: "Broken DAC (opens never)" },
];
let audioCurrent = "hw:CARD=Headphones";
const mockOutputs = () =>
  MOCK_OUTPUTS.map((output) => ({ ...output, current: output.id === audioCurrent }));

const state: RemoteState = {
  connected: !MA_DOWN,
  ma_url: MA_URL,
  player: {
    name: "Wohnzimmer",
    powered: params.get("state") !== "off",
    playing: params.get("state") === null,
    loading: false,
    has_media: params.get("state") !== "nothing",
    volume: 42,
    title: params.has("long") ? LONG_TITLE : "Nights",
    artist: params.has("long") ? LONG_ARTIST : "Frank Ocean",
    cover: params.get("state") === "nothing" ? "" : COVER,
    accent: "#c98a5b",
  },
  view: {
    now_playing: !params.has("vu"),
    queue: false,
    vu: params.has("vu"),
    standby: params.get("state") === "off",
    vu_index: 3,
    vu_count: 10,
    vu_skins: [
      "night",
      "cream",
      "round",
      "wall",
      "wall-light",
      "arc",
      "orbit",
      "modern",
      "spectrum",
      "bars",
    ],
  },
  display: { supported: true, brightness: 65 },
};

function push(): void {
  instance?.reply({
    event: "state",
    data: structuredClone(state),
  });
}

// the mock's settings file: defaults are folded into `effective`,
// the overlay is what set/reset edit and what `overrides` lists.
// The MA token stays server-side — the state reports `token-set`.
const overlay: Record<string, unknown> = structuredClone(MOCK_OVERRIDES);
let maTokenSet = true;

function maConfig(): Record<string, unknown> {
  return {
    url: String(readPath(overlay, "music-assistant.url") ?? MA_URL),
    player: String(readPath(overlay, "music-assistant.player") ?? "Wohnzimmer"),
    token: null,
    "token-set": maTokenSet,
  };
}

function deepMerge(base: unknown, over: unknown): unknown {
  if (over === null || typeof over !== "object" || Array.isArray(over)) return over;
  if (base === null || typeof base !== "object" || Array.isArray(base)) return over;
  const merged: Record<string, unknown> = { ...(base as Record<string, unknown>) };
  for (const [key, value] of Object.entries(over as Record<string, unknown>)) {
    merged[key] = deepMerge(merged[key], value);
  }
  return merged;
}

function flatten(tree: unknown, prefix = ""): string[] {
  if (tree === null || typeof tree !== "object" || Array.isArray(tree)) {
    return prefix ? [prefix] : [];
  }
  return Object.entries(tree as Record<string, unknown>).flatMap(([key, value]) =>
    flatten(value, prefix ? `${prefix}.${key}` : key),
  );
}

function applyFlat(key: string, value: unknown): void {
  const parts = key.split(".");
  let node = overlay;
  for (const part of parts.slice(0, -1)) {
    const next = node[part];
    if (next === null || typeof next !== "object" || Array.isArray(next)) {
      node[part] = {};
    }
    node = node[part] as Record<string, unknown>;
  }
  node[parts[parts.length - 1]] = value;
}

function deleteFlat(key: string): void {
  const parts = key.split(".");
  let node: Record<string, unknown> = overlay;
  for (const part of parts.slice(0, -1)) {
    const next = node[part];
    if (next === null || typeof next !== "object") return;
    node = next as Record<string, unknown>;
  }
  delete node[parts[parts.length - 1]];
}

function stateData(): StateData {
  return {
    config: deepMerge({ "music-assistant": maConfig() }, overlay) as Record<string, unknown>,
    effective: deepMerge(MOCK_DEFAULTS, overlay) as Record<string, unknown>,
    overrides: flatten(overlay),
    base_path: null,
    provider_status: MA_DOWN ? "Music Assistant unreachable: connect timeout" : "",
    remote_url: "http://192.168.1.50:8787",
  };
}

function pushSettings(): void {
  instance?.reply({ event: "settings" });
}

class MockSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  readyState = MockSocket.CONNECTING;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;

  constructor(_url: string) {
    setTimeout(() => {
      this.readyState = MockSocket.OPEN;
      this.onopen?.();
      push();
    }, 60);
  }

  reply(frame: object): void {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }

  send(raw: string): void {
    const request = JSON.parse(raw) as Request & Record<string, unknown>;
    const respond = (ok: boolean, data?: unknown) =>
      this.reply({ id: request.id, ok, ...(ok ? { data } : { error: data }) });
    switch (request.op) {
      case "hello":
        respond(true, {
          app: "mestro",
          version: "0.1.0-mock",
          remote_url: "http://192.168.1.50:8787",
          ota: true,
          capabilities: {
            ota: true,
            display: true,
            wifi: true,
            factory_reset: true,
            audio_output: true,
            timezone: true,
          },
        });
        return;
      case "wifi": {
        if (request.action === "status") {
          respond(
            true,
            AP_MODE
              ? { state: "ap", ap_ssid: "mestro-a1b2", ssid: "", error: "" }
              : { state: "sta", ap_ssid: "mestro-a1b2", ssid: "HomeNet", error: "" },
          );
        } else if (request.action === "scan") {
          respond(true, { networks: MOCK_NETWORKS });
        } else if (request.action === "join" && typeof request.ssid === "string") {
          // a second concurrent join answers busy, like the device
          if (joinBusy) {
            respond(false, "busy");
            return;
          }
          joinBusy = true;
          const fail = request.ssid === "Alt-Router";
          setTimeout(
            () => {
              joinBusy = false;
              if (fail) respond(false, `could not join ${request.ssid}: wrong password`);
              else respond(true, { ssid: request.ssid, state: "sta" });
            },
            // slow enough to watch the joining state, faster than
            // the 45 s budget
            fail ? 1500 : 6000,
          );
        }
        return;
      }
      case "timezone": {
        if (request.action === "zones") {
          respond(true, { zones: MOCK_ZONES, chosen: tzChosen, current: tzCurrent });
        } else if (typeof request.zone === "string") {
          const zone = request.zone;
          if (!MOCK_ZONES.includes(zone)) {
            respond(false, `unknown timezone ${zone}`);
            return;
          }
          // adopt: only while nothing was ever chosen; set: always
          const taken = request.action === "set" || tzChosen === "";
          if (taken) tzChosen = zone;
          // the mock keeps the page alive on adopt (the headless browser
          // is rarely in Berlin); only a user choice restarts
          const restarting = taken && request.action === "set" && zone !== tzCurrent;
          respond(true, { zone, taken, restarting });
          // the restart: the socket drops and reconnects
          if (restarting) {
            tzCurrent = zone;
            setTimeout(() => instance?.close(), 800);
          }
        }
        return;
      }
      case "audio": {
        if (request.action === "outputs") {
          respond(true, {
            outputs: mockOutputs(),
            current: audioCurrent,
          });
        } else if (request.action === "set" && typeof request.output === "string") {
          const output = request.output;
          // BrokenDAC stands for a device squeezelite cannot open:
          // the player rolls back and names it
          setTimeout(() => {
            if (output === BROKEN_OUTPUT) {
              respond(
                false,
                `squeezelite could not start on ${output} (squeezelite stopped right after the start) — back on the previous output`,
              );
            } else {
              audioCurrent = output;
              respond(true, { output });
            }
          }, 1200);
        }
        return;
      }
      case "settings": {
        if (resetDone) {
          respond(false, "pairing required");
          return;
        }
        if (request.action === "factory-reset") {
          respond(true, { restarting: true });
          // the restart: the socket drops, the pin is gone — the
          // reconnect's handshake lands on the pair screen
          setTimeout(() => {
            resetDone = true;
            instance?.close();
          }, 1000);
          return;
        }
        if (request.action === "schema") {
          const cycle = readPath(deepMerge(MOCK_DEFAULTS, overlay), "vu.cycle");
          respond(true, mockSchema(Array.isArray(cycle) ? (cycle as string[]) : []));
          return;
        }
        if (request.action === "state") {
          respond(true, stateData());
          return;
        }
        if (request.action === "set" && typeof request.key === "string") {
          applyFlat(request.key, request.value);
          if (request.key === "music-assistant.token") {
            maTokenSet = request.value !== "";
            deleteFlat("music-assistant.token");
          }
          if (request.key === "display.brightness") {
            state.display.brightness = Number(request.value);
          }
        } else if (request.action === "reset" && typeof request.key === "string") {
          deleteFlat(request.key);
        } else if (request.action === "reset-all") {
          for (const key of Object.keys(overlay)) delete overlay[key];
        }
        respond(true, stateData());
        pushSettings();
        return;
      }
      case "ma": {
        // the wizard/connection flows: a canned server that wants
        // a login and serves three players
        if (request.action === "probe") {
          respond(true, { version: "2.8.0", schema: 28, auth: "login" });
        } else if (request.action === "login") {
          applyFlat("music-assistant.url", String(request.url));
          maTokenSet = true;
          respond(true, stateData());
          pushSettings();
        } else if (request.action === "players") {
          respond(true, MA_PLAYERS);
        } else {
          respond(false, "unknown action");
        }
        return;
      }
      case "ctl": {
        const cmd = request.cmd as CtlCmd;
        if (cmd === "play-pause") state.player.playing = !state.player.playing;
        else if (cmd === "now-playing")
          state.view = { ...state.view, now_playing: true, queue: false, vu: false };
        else if (cmd === "queue")
          state.view = { ...state.view, now_playing: false, queue: true, vu: false };
        else if (cmd === "vu")
          state.view = { ...state.view, now_playing: false, queue: false, vu: true };
        else if (cmd === "standby-toggle") {
          state.player.powered = !state.player.powered;
          state.view.standby = !state.player.powered;
          if (!state.player.powered) state.player.playing = false;
        } else if (cmd === "cycle-forward" && state.view.vu)
          state.view.vu_index = (state.view.vu_index + 1) % state.view.vu_count;
        else if (cmd === "cycle-back" && state.view.vu)
          state.view.vu_index =
            (state.view.vu_index - 1 + state.view.vu_count) % state.view.vu_count;
        respond(true, {});
        push();
        return;
      }
      case "player": {
        if (request.action === "volume") state.player.volume = request.value as number;
        respond(true, {});
        push();
        return;
      }
      default:
        respond(false, { error: "unknown op" });
    }
  }

  close(): void {
    this.readyState = 3;
    this.onclose?.();
  }
}

let instance: MockSocket | null = null;

const RealWebSocket = globalThis.WebSocket;

function patchedWebSocket(url: string): WebSocket {
  const socket = new MockSocket(url) as unknown as WebSocket;
  // pushes go to the newest socket — a reconnect replaces it
  instance = socket as unknown as MockSocket;
  return socket;
}

Object.defineProperty(patchedWebSocket, "OPEN", { value: RealWebSocket.OPEN });
Object.defineProperty(patchedWebSocket, "CONNECTING", {
  value: RealWebSocket.CONNECTING,
});

const realFetch = globalThis.fetch.bind(globalThis);

async function patchedFetch(input: RequestInfo | URL, init?: RequestInit) {
  const path = typeof input === "string" ? input : input.toString();
  if (path === "/pair/status") {
    return new Response(JSON.stringify({ paired: !params.has("pair") && !AP_MODE && !resetDone }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }
  if (path === "/diagnostics") {
    // the device's report, shortened: secrets are already out of it
    const report = {
      report: "mestro-diagnostics",
      format: 1,
      version: "0.1.0-mock",
      state: { effective: { "music-assistant": { token: "[redacted]" } } },
      logs: { "/tmp/mestro.log": ["INFO mock: nothing to see"] },
    };
    return new Response(JSON.stringify(report, null, 2), {
      status: 200,
      headers: {
        "content-type": "application/json",
        "content-disposition": 'attachment; filename="mestro-diagnostics-mock.json"',
      },
    });
  }
  if (path === "/pair" && init?.method === "POST") {
    return new Response(null, { status: 204 });
  }
  return realFetch(input, init);
}

export function installMock(): void {
  (globalThis as any).WebSocket = patchedWebSocket;
  globalThis.fetch = patchedFetch as typeof fetch;
  // screenshot runs pick the locale explicitly — the headless
  // browser always speaks English
  const lang = params.get("lang");
  if (lang === "de" || lang === "en") setLocale(lang);
}

// dev-only screen-state helper: `?pin` fills the pair input so
// the primary button shows its enabled state
export function fillPinInput(): void {
  if (params.has("pin")) {
    const pinTimer = setInterval(() => {
      const input = document.querySelector<HTMLInputElement>(".pin-input");
      if (!input) return;
      clearInterval(pinTimer);
      input.value = "ABCD2345";
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }, 100);
  }
}

// dev-only: `?measure` writes bounding boxes into <title> so a
// headless `--dump-dom` run can verify the layout numerically
export function installMeasureProbe(): void {
  if (!params.has("measure")) return;
  const selectors = [
    ".remote",
    ".np",
    ".power-row",
    ".kiosk",
    ".tiles",
    ".settings",
    ".pair .card",
  ];
  setTimeout(() => {
    const report: Record<string, [number, number, number, number]> = {};
    for (const selector of selectors) {
      const el = document.querySelector(selector);
      if (!el) continue;
      const r = el.getBoundingClientRect();
      report[selector] = [
        Math.round(r.x),
        Math.round(r.y),
        Math.round(r.width),
        Math.round(r.height),
      ];
    }
    document.title = JSON.stringify({
      ...report,
      viewport: [window.innerWidth, window.innerHeight],
    });
  }, 600);
}
