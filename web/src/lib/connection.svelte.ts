// the wizard's connection to mestro plus the device facts every
// page can read (app name, wizard URL, current setup state)

import { WizardSocket, wizardUrl } from "./ws";
import type { HelloData, StateData } from "./protocol";

export const socket = new WizardSocket(wizardUrl());

// mutable reactive object — property updates are tracked in
// templates (Svelte forbids reassigning exported $state bindings,
// a const state object is fine)
export const device = $state({
  app: "mestro",
  version: "",
  // the device accepts system-image uploads (build feature `ota`)
  ota: false,
  remoteUrl: "",
  configured: false,
  configUrl: "",
  configPlayer: "",
  // false until the first state read lands — pages/nav show a
  // loading state instead of guessing
  ready: false,
});

export async function connect(): Promise<void> {
  await socket.connect();
  const hello = await socket.send<HelloData>({ op: "hello" });
  device.app = hello.app;
  device.version = hello.version;
  device.ota = hello.ota;
  device.remoteUrl = hello.remote_url;
  await refreshDevice();
}

export async function refreshDevice(): Promise<void> {
  const state = await socket.send<StateData>({ op: "settings", action: "state" });
  const ma = (state.config["music-assistant"] ?? {}) as Record<string, unknown>;
  device.configUrl = String(ma.url ?? "");
  device.configPlayer = String(ma.player ?? "");
  // the kiosk's own truth (first-run latch): only a server URL
  // counts as "configured" — a lone token/player does not
  device.configured = device.configUrl !== "";
  device.ready = true;
}
