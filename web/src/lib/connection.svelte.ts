// the app's connection to mestro plus the device facts every page
// can read (app name, build capabilities, setup state)

import { attachLifecycle, KioskSocket, socketUrl, type SocketStatus } from "./ws";
import type { HelloData, StateData } from "./protocol";
import { pairStatus, isPaired } from "./pairing";

export const socket = new KioskSocket(socketUrl());

// mutable reactive object — property updates are tracked in
// templates (Svelte forbids reassigning exported $state bindings,
// a const state object is fine)
export const device = $state({
  app: "mestro",
  version: "",
  // the device accepts system-image uploads (build feature `ota`)
  ota: false,
  // the kiosk screen brightness is controllable (hello capability)
  display: false,
  // the wifi setup lane runs on this device (hello capabilities,
  // feature `wifi-setup`): /setup/wifi, wifi ops, factory reset
  wifi: false,
  factoryReset: false,
  // the audio output picker works here (hello capability
  // `audio_output`: the build has the feature and squeezelite.env exists)
  audioOutput: false,
  remoteUrl: "",
  configured: false,
  configUrl: "",
  configPlayer: "",
  // false until the first state read lands — pages/nav show a
  // loading state instead of guessing
  ready: false,
});

export const connection = $state({
  // pairing gate: null = still checking, true = cookie accepted
  paired: null as boolean | null,
  // socket lifecycle, drives the reconnect banner
  status: "idle" as SocketStatus,
});

export async function checkPaired(): Promise<void> {
  connection.paired = await isPaired();
}

// hello + the settings read that make up `device`
export async function runHandshake(): Promise<void> {
  const hello = await socket.send<HelloData>({ op: "hello" });
  device.app = hello.app;
  device.version = hello.version;
  device.ota = hello.capabilities?.ota ?? hello.ota;
  device.display = hello.capabilities?.display ?? false;
  device.wifi = hello.capabilities?.wifi ?? false;
  device.factoryReset = hello.capabilities?.factory_reset ?? false;
  device.audioOutput = hello.capabilities?.audio_output ?? false;
  device.remoteUrl = hello.remote_url;
  await refreshDevice();
}

// brings the socket up once and keeps `connection.status` in
// sync. EVERY successful (re)connect repeats the handshake: a
// kiosk that rebooted or lost the first attempt re-learns its
// version/config on the next open; failures just wait for it.
export function startConnection(): void {
  socket.onStatus((status) => (connection.status = status));
  socket.onOpen(() => {
    void runHandshake().catch(() => {});
  });
  // a factory reset (from any phone) kills the cookie: the /ws
  // handshake then rejects with 401, which only ever shows up as
  // a closed socket. One cheap cookie-only probe per retry
  // decides — an explicit paired:false stops the loop and lands
  // the app on the pair screen. A never-paired session (the
  // setup portal) keeps dialing: its socket reopens when the
  // hotspot does
  socket.setRetryGate(async () => {
    if (connection.paired !== true) return true;
    if ((await pairStatus()) === false) {
      connection.paired = false;
      device.ready = false;
      return false;
    }
    return true;
  });
  attachLifecycle(socket);
  socket.connect();
}

export async function refreshDevice(): Promise<void> {
  let state: StateData;
  try {
    state = await socket.send<StateData>({ op: "settings", action: "state" });
  } catch (error) {
    // a paired session that lost its cookie — the kiosk
    // factory-reset itself, from any phone — lands back on the
    // pair screen
    if (error instanceof Error && error.message === "pairing required") {
      connection.paired = false;
      device.ready = false;
    }
    throw error;
  }
  const ma = (state.config["music-assistant"] ?? {}) as Record<string, unknown>;
  device.configUrl = String(ma.url ?? "");
  device.configPlayer = String(ma.player ?? "");
  // the kiosk's own truth (first-run latch): only a server URL
  // counts as "configured" — a lone token/player does not
  device.configured = device.configUrl !== "";
  device.ready = true;
}
