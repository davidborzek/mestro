// the System section's live facts, shared by the settings list (its
// summary) and the System page: the timezone picker's state and the
// wifi network the player is on

import { device, socket } from "./connection.svelte";
import { createPickerActions, zoneLabel, type PickerState, type TimezoneSend } from "./timezone";
import { systemSummaryText } from "./settingsSections";
import type { WifiStatusData } from "./protocol";

const send: TimezoneSend = (request) => socket.send(request);

export const timezonePicker = $state<PickerState>({
  zones: [],
  chosen: "",
  current: null,
  loaded: false,
  applying: false,
  error: "",
});
export const timezoneActions = createPickerActions(send, timezonePicker);

export const wifiFacts = $state({ ssid: "" });

// the current network — a status error just leaves it empty,
// nothing waits on it
async function refreshWifi(): Promise<void> {
  try {
    const status = await socket.send<WifiStatusData>({ op: "wifi", action: "status" });
    wifiFacts.ssid = status.ssid || status.ap_ssid;
  } catch {
    // the summary falls back to the timezone
  }
}

// the zone the picker shows: the explicit choice, else the one the
// player runs in
export function shownZone(): string | null {
  return timezonePicker.chosen || timezonePicker.current;
}

// the list row's summary: the wifi network, else the timezone
export function systemSummary(): string {
  const zone = shownZone();
  return systemSummaryText(device, wifiFacts.ssid, zone ? zoneLabel(zone) : null);
}

// reads the facts and keeps them fresh: a restart (timezone apply)
// ends in a dropped socket, and the reconnect frees the picker and
// reads what the new process runs in
export function startSystemFacts(): () => void {
  const read = () => {
    if (device.wifi) void refreshWifi();
  };
  read();
  if (device.timezone) void timezoneActions.load();
  return socket.onOpen(() => {
    read();
    if (device.timezone) void timezoneActions.reconnected();
  });
}
