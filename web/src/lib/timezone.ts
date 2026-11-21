// the timezone picker's logic: DOM-free, the page state lives in
// the component. `send` is the socket's request function.

import type { TimezoneOp, TimezoneSetData, TimezoneZonesData } from "./protocol";

export type TimezoneSend = (request: { op: "timezone" } & TimezoneOp) => Promise<unknown>;

// "America/New_York" → "America/New York"
export function zoneLabel(zone: string): string {
  return zone.replace(/_/g, " ");
}

// the picker's search: case-insensitive substring over the label
export function filterZones(zones: string[], query: string): string[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return zones;
  return zones.filter((zone) => zoneLabel(zone).toLowerCase().includes(needle));
}

export async function loadZones(send: TimezoneSend): Promise<TimezoneZonesData> {
  return (await send({ op: "timezone", action: "zones" })) as TimezoneZonesData;
}

// the browser's IANA zone ("" when it has none)
export function browserZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone ?? "";
  } catch {
    return "";
  }
}

// offers the browser's zone to the player once per page load. The
// device adopts it only while nobody ever chose a zone (a phone
// in another timezone must not flip the player later), so a
// repeated offer is harmless — but one is enough. A refusal (not
// paired yet, a zone the device does not know) is swallowed and
// retried at the next connect: the page must not break over it.
export function createAdopter(
  send: TimezoneSend,
  zone: () => string = browserZone,
): () => Promise<void> {
  let done = false;
  return async () => {
    if (done) return;
    const name = zone();
    if (!name) {
      done = true;
      return;
    }
    try {
      await send({ op: "timezone", action: "adopt", zone: name });
      done = true;
    } catch {
      // retried at the next connect
    }
  };
}

// what the picker row shows; the component keeps it as `$state`
export interface PickerState {
  zones: string[];
  // the explicit choice ("" = never chosen)
  chosen: string;
  // the zone the player runs in
  current: string | null;
  loaded: boolean;
  // a zone was sent and the player is restarting to apply it
  applying: boolean;
  error: string;
}

const errorText = (cause: unknown) => (cause instanceof Error ? cause.message : String(cause));

// the picker's actions on its state: DOM-free so bun test drives them
export function createPickerActions(send: TimezoneSend, state: PickerState) {
  async function load(): Promise<void> {
    try {
      const data = await loadZones(send);
      state.zones = data.zones;
      state.chosen = data.chosen;
      state.current = data.current;
    } catch (cause) {
      state.error = errorText(cause);
    } finally {
      state.loaded = true;
    }
  }

  return {
    load,

    async pick(zone: string): Promise<void> {
      if (zone === (state.chosen || state.current)) return;
      state.applying = true;
      state.error = "";
      try {
        const reply = (await send({ op: "timezone", action: "set", zone })) as TimezoneSetData;
        state.chosen = reply.zone;
        // the player restarts when the running zone changed — the
        // pill stays up until the socket comes back (`reconnected`)
        if (!reply.restarting) state.applying = false;
      } catch (cause) {
        state.error = errorText(cause);
        state.applying = false;
      }
    },

    // the socket opened again: the restarted player runs in the new
    // zone, so the busy state ends and the list is read afresh
    async reconnected(): Promise<void> {
      state.applying = false;
      state.error = "";
      await load();
    },
  };
}
