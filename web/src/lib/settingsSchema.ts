// schema-driven settings helpers: pure functions the settings
// store and controls share — effective-value lookup, visibility,
// short value formatting. No runes here so bun can test it all.

import type { Field, Option, Request, Section } from "./protocol";
import type { KioskSocket } from "./ws";

// reads the current value of any dotted key (the store layers
// optimistic writes on top)
export type ValueOf = (key: string) => unknown;
export type Capabilities = { display: boolean };

export type ValueTexts = {
  on: string;
  off: string;
  inherit: string;
  // localized option label (falls back to the schema's English)
  optionLabel: (option: Option) => string;
};

// dotted keys address the effective config's nested tree:
// "now-playing.font.title-size" → ["now-playing"]["font"]["title-size"]
export function readPath(tree: unknown, key: string): unknown {
  let node = tree;
  for (const part of key.split(".")) {
    if (node === null || typeof node !== "object") return undefined;
    node = (node as Record<string, unknown>)[part];
  }
  return node;
}

export function fieldVisible(field: Field, valueOf: ValueOf, caps: Capabilities): boolean {
  const cond = field.visible_if;
  if (!cond) return true;
  if ("capability" in cond) return cond.capability !== "display" || caps.display;
  const value = String(valueOf(cond.key));
  if (cond.in && !cond.in.includes(value)) return false;
  if (cond.not_in && cond.not_in.includes(value)) return false;
  return true;
}

// a section shows while any of its fields does (the display
// section is one capability-gated field set)
export function sectionVisible(section: Section, valueOf: ValueOf, caps: Capabilities): boolean {
  return section.fields.some((field) => fieldVisible(field, valueOf, caps));
}

// choice and ordered-subset options may carry thumbnails (the
// VU cycle, the clock styles): those render as image tiles and
// always wrap below the label (a segmented row would squeeze them)
export function hasPreviews(field: Field): boolean {
  if (field.kind.type !== "choice" && field.kind.type !== "ordered-subset") return false;
  return field.kind.options.some((option) => !!option.preview);
}

export function percentToShown(value: unknown): number {
  const stored = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(stored)) return 0;
  return Math.round(Math.min(1, Math.max(0, stored)) * 100);
}

export function shownToPercent(shown: number): number {
  return Math.round(shown) / 100;
}

// the short current-value text: section summaries and choice rows
export function formatValue(field: Field, value: unknown, texts: ValueTexts): string {
  const kind = field.kind;
  switch (kind.type) {
    case "bool":
      return value === true ? texts.on : texts.off;
    case "inherit-bool":
      return value === true ? texts.on : value === false ? texts.off : texts.inherit;
    case "choice": {
      const option = kind.options.find((o) => o.value === value);
      return option ? texts.optionLabel(option) : String(value ?? "");
    }
    case "number": {
      if (typeof value !== "number") return "";
      return `${value}${kind.unit ? ` ${kind.unit}` : ""}`;
    }
    case "percent":
      return `${percentToShown(value)} %`;
    default:
      // ordered lists have no short text
      return "";
  }
}

// the dim summary a section row shows: current values of the
// first visible fields, two at most
export function sectionSummary(
  section: Section,
  valueOf: ValueOf,
  caps: Capabilities,
  texts: ValueTexts,
): string {
  const parts: string[] = [];
  for (const field of section.fields) {
    if (parts.length === 2) break;
    if (!fieldVisible(field, valueOf, caps)) continue;
    const text = formatValue(field, valueOf(field.key), texts);
    if (text) parts.push(text);
  }
  return parts.join(" · ");
}

// the honest tri-state: the effective value decides, null (or
// absent) means inherit. A key the base config file pins shows
// as On/Off — never a pretend Inherit.
export function triStateValue(value: unknown): "inherit" | "on" | "off" {
  if (value === true) return "on";
  if (value === false) return "off";
  return "inherit";
}

// set in the effective config while the settings overlay never
// wrote it: the config file pins the key, a reset cannot reach
// Inherit
export function basePinned(value: unknown, overridden: boolean): boolean {
  return triStateValue(value) !== "inherit" && !overridden;
}

// ── optimistic writes ──
// Responses can arrive out of order (the server processes
// settings ops on a blocking pool), so every write of a key
// bumps a generation. A reply — success or error — may only
// clear the optimistic value while it is still the key's latest
// write; a stale one must not revert a newer edit.

export type FieldWriterTransport = {
  send: (request: Request) => Promise<unknown>;
  // apply the set/reset reply (the new state payload)
  applyReply: (reply: unknown) => void | Promise<void>;
  // undefined clears the key's optimistic entry
  setPending: (key: string, value: unknown | undefined) => void;
  onError: (key: string, message: string) => void;
};

export function createFieldWriter(transport: FieldWriterTransport) {
  const generations = new Map<string, number>();

  async function run(
    key: string,
    generation: number,
    request: Request,
    optimistic: unknown | undefined,
  ): Promise<void> {
    if (optimistic !== undefined) transport.setPending(key, optimistic);
    try {
      const reply = await transport.send(request);
      if (generations.get(key) !== generation) return;
      transport.setPending(key, undefined);
      await transport.applyReply(reply);
    } catch (error) {
      // a stale failure belongs to a newer write's story — no
      // revert, no toast — but the call itself still failed
      if (generations.get(key) === generation) {
        transport.setPending(key, undefined);
        transport.onError(key, error instanceof Error ? error.message : String(error));
      }
      throw error;
    }
  }

  return {
    set(key: string, value: unknown): Promise<void> {
      const generation = (generations.get(key) ?? 0) + 1;
      generations.set(key, generation);
      return run(key, generation, { op: "settings", action: "set", key, value }, value);
    },
    reset(key: string, optimistic?: unknown): Promise<void> {
      const generation = (generations.get(key) ?? 0) + 1;
      generations.set(key, generation);
      return run(key, generation, { op: "settings", action: "reset", key }, optimistic);
    },
  };
}

// ── the settings feed ──
// Settings pushes carry no payload, and a socket that was down
// missed some — so pushes AND every (re)connect trigger the
// same refetch. The page unsubscribes on unmount.
export function createSettingsFeed(
  socket: Pick<KioskSocket, "onSettingsPush" | "onOpen">,
  refetch: () => void,
): () => void {
  const offPush = socket.onSettingsPush(refetch);
  const offOpen = socket.onOpen(refetch);
  return () => {
    offPush();
    offOpen();
  };
}
