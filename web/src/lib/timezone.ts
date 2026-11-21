// the timezone picker's logic: DOM-free, the page state lives in
// the component. `send` is the socket's request function.

import type { TimezoneOp, TimezoneZonesData } from "./protocol";

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
