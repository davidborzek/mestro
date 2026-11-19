import { expect, test } from "bun:test";
import {
  basePinned,
  createFieldWriter,
  createSettingsFeed,
  fieldVisible,
  formatValue,
  hasPreviews,
  percentToShown,
  readPath,
  sectionSummary,
  sectionVisible,
  shownToPercent,
  triStateValue,
  type ValueTexts,
} from "./settingsSchema";
import type { Field, Section } from "./protocol";

const texts: ValueTexts = {
  on: "On",
  off: "Off",
  inherit: "Inherit",
  optionLabel: (option) => option.label,
};

const caps = { display: true };

test("readPath walks the effective tree by dotted segments", () => {
  const tree = {
    "now-playing": { font: { "title-size": 28 }, controls: ["shuffle", "play-pause"] },
    display: { brightness: 65 },
  };
  expect(readPath(tree, "now-playing.font.title-size")).toBe(28);
  expect(readPath(tree, "now-playing.controls")).toEqual(["shuffle", "play-pause"]);
  expect(readPath(tree, "display.brightness")).toBe(65);
});

test("readPath returns undefined for missing keys and broken trees", () => {
  expect(readPath({}, "nope.missing")).toBeUndefined();
  expect(readPath(undefined, "display.brightness")).toBeUndefined();
  expect(readPath({ display: 3 }, "display.brightness")).toBeUndefined();
});

test("fieldVisible passes fields without a condition", () => {
  const field: Field = { key: "ui.rail", label: "Rail", kind: { type: "bool" } };
  expect(fieldVisible(field, () => undefined, caps)).toBe(true);
});

test("fieldVisible checks in and not_in against current values", () => {
  const dim: Field = {
    key: "clock.background.dim",
    label: "Dim",
    kind: { type: "percent" },
    visible_if: { key: "clock.background.source", not_in: ["black"] },
  };
  const interval: Field = {
    key: "clock.background.interval",
    label: "Interval",
    kind: { type: "number", min: 0, max: 120, step: 5, unit: "min" },
    visible_if: { key: "clock.background.source", in: ["folder"] },
  };
  const at = (source: string) => (key: string) =>
    key === "clock.background.source" ? source : undefined;
  expect(fieldVisible(dim, at("black"), caps)).toBe(false);
  expect(fieldVisible(dim, at("cover"), caps)).toBe(true);
  expect(fieldVisible(interval, at("folder"), caps)).toBe(true);
  expect(fieldVisible(interval, at("cover"), caps)).toBe(false);
});

test("fieldVisible gates display fields on the capability", () => {
  const field: Field = {
    key: "display.brightness",
    label: "Brightness",
    kind: { type: "number", min: 1, max: 100, step: 1 },
    visible_if: { capability: "display" },
  };
  expect(fieldVisible(field, () => undefined, { display: true })).toBe(true);
  expect(fieldVisible(field, () => undefined, { display: false })).toBe(false);
});

test("sectionVisible is true while any field shows", () => {
  const section: Section = {
    id: "display",
    title: "Display",
    icon: "sun",
    fields: [
      {
        key: "display.brightness",
        label: "Brightness",
        kind: { type: "number", min: 1, max: 100, step: 1 },
        visible_if: { capability: "display" },
      },
    ],
  };
  expect(sectionVisible(section, () => undefined, caps)).toBe(true);
  expect(sectionVisible(section, () => undefined, { display: false })).toBe(false);
});

test("percent converts between stored 0..1 and shown 0..100", () => {
  expect(percentToShown(0.35)).toBe(35);
  expect(percentToShown(1)).toBe(100);
  expect(percentToShown(null)).toBe(0);
  expect(shownToPercent(35)).toBeCloseTo(0.35);
  expect(shownToPercent(70)).toBeCloseTo(0.7);
});

test("formatValue renders each kind's short text", () => {
  const bool: Field = { key: "osd.track", label: "Track", kind: { type: "bool" } };
  expect(formatValue(bool, true, texts)).toBe("On");
  expect(formatValue(bool, false, texts)).toBe("Off");

  const inherit: Field = { key: "osd.vu.track", label: "Track", kind: { type: "inherit-bool" } };
  expect(formatValue(inherit, null, texts)).toBe("Inherit");
  expect(formatValue(inherit, true, texts)).toBe("On");

  const choice: Field = {
    key: "now-playing.layout",
    label: "Layout",
    kind: {
      type: "choice",
      options: [
        { value: "side", label: "Side by side" },
        { value: "fullbleed", label: "Full bleed" },
      ],
    },
  };
  expect(formatValue(choice, "fullbleed", texts)).toBe("Full bleed");
  expect(formatValue(choice, "unknown", texts)).toBe("unknown");

  const number: Field = {
    key: "now-playing.font.title-size",
    label: "Title size",
    kind: { type: "number", min: 8, max: 48, step: 1, unit: "px" },
  };
  expect(formatValue(number, 28, texts)).toBe("28 px");

  const percent: Field = { key: "clock.background.dim", label: "Dim", kind: { type: "percent" } };
  expect(formatValue(percent, 0.35, texts)).toBe("35 %");

  const subset: Field = {
    key: "now-playing.controls",
    label: "Controls",
    kind: { type: "ordered-subset", options: [{ value: "shuffle", label: "Shuffle" }] },
  };
  expect(formatValue(subset, ["shuffle"], texts)).toBe("");
});

test("sectionSummary joins the first visible values, two at most", () => {
  const section: Section = {
    id: "theme",
    title: "Theme",
    icon: "palette",
    fields: [
      {
        key: "theme.palette",
        label: "Palette",
        kind: {
          type: "choice",
          options: [
            { value: "amber", label: "Amber" },
            { value: "cover", label: "From cover" },
          ],
        },
      },
      {
        key: "theme.variant",
        label: "Variant",
        kind: {
          type: "choice",
          options: [{ value: "tonal-spot", label: "Tonal" }],
        },
      },
      {
        key: "ui.rail",
        label: "Rail",
        kind: {
          type: "choice",
          options: [
            { value: "dark", label: "Dark" },
            { value: "glass", label: "Glass" },
          ],
        },
      },
      {
        key: "ui.auto_open_now_playing",
        label: "Auto-open",
        kind: { type: "bool" },
      },
    ],
  };
  const values: Record<string, unknown> = {
    "theme.palette": "amber",
    "theme.variant": "tonal-spot",
    "ui.rail": "glass",
    "ui.auto_open_now_playing": true,
  };
  expect(sectionSummary(section, (key) => values[key], caps, texts)).toBe("Amber · Tonal");
});

test("sectionSummary skips hidden and list fields", () => {
  const section: Section = {
    id: "vu",
    title: "VU",
    icon: "vu",
    fields: [
      {
        key: "vu.cycle",
        label: "Cycle",
        kind: { type: "ordered-subset", options: [{ value: "arc", label: "Arc" }] },
      },
      {
        key: "vu.arc.glow",
        label: "Glow",
        kind: { type: "percent" },
        visible_if: { key: "clock.background.source", in: ["folder"] },
      },
    ],
  };
  expect(sectionSummary(section, () => undefined, caps, texts)).toBe("");
  // hidden fields drop out; a visible one with no stored value
  // still summarizes (percent reads as 0 %)
  expect(
    sectionSummary(
      section,
      (key) => (key === "clock.background.source" ? "folder" : undefined),
      caps,
      texts,
    ),
  ).toBe("0 %");
});

// ── tri-state honesty: the base config may pin an inherit-bool ──

test("triStateValue maps the effective value, null means inherit", () => {
  expect(triStateValue(true)).toBe("on");
  expect(triStateValue(false)).toBe("off");
  expect(triStateValue(null)).toBe("inherit");
  expect(triStateValue(undefined)).toBe("inherit");
});

test("basePinned marks keys only the config file sets", () => {
  // set in effective, never overridden: the file pins it, a
  // reset cannot reach Inherit
  expect(basePinned(false, false)).toBe(true);
  expect(basePinned(true, false)).toBe(true);
  // the overlay wrote it — a reset is meaningful
  expect(basePinned(false, true)).toBe(false);
  // genuinely unset: already inheriting
  expect(basePinned(null, false)).toBe(false);
});

// ── optimistic writes: stale replies must not revert newer edits ──

type WriteSpy = {
  sent: unknown[];
  applied: unknown[];
  errors: { key: string; message: string }[];
  pending: Map<string, unknown>;
};

function spyWriter(behavior: (request: any, spy: WriteSpy) => Promise<unknown>) {
  const spy: WriteSpy = { sent: [], applied: [], errors: [], pending: new Map() };
  const writer = createFieldWriter({
    send: (request) => {
      spy.sent.push(request);
      return behavior(request, spy);
    },
    applyReply: (reply) => {
      spy.applied.push(reply);
    },
    setPending: (key, value) => {
      if (value === undefined) spy.pending.delete(key);
      else spy.pending.set(key, value);
    },
    onError: (key, message) => {
      spy.errors.push({ key, message });
    },
  });
  return { writer, spy };
}

test("a late reply never reverts a newer edit of the same key", async () => {
  let releaseFirst: (() => void) | null = null;
  const { writer, spy } = spyWriter((request) => {
    if (request.value === "A") {
      return new Promise((resolve) => {
        releaseFirst = () => resolve({ effective: { k: "A" } });
      });
    }
    return Promise.resolve({ effective: { k: "B" } });
  });

  const first = writer.set("k", "A");
  const second = writer.set("k", "B");
  expect(spy.pending.get("k")).toBe("B");

  await second;
  expect(spy.applied).toEqual([{ effective: { k: "B" } }]);

  // A's reply arrives after B's — stale, it must not clear B's
  // pending value nor regress the applied state
  releaseFirst!();
  await first;
  expect(spy.applied).toEqual([{ effective: { k: "B" } }]);
  expect(spy.pending.has("k")).toBe(false);
  expect(spy.errors).toEqual([]);
});

test("a stale error leaves the newer edit untouched", async () => {
  let rejectFirst: ((error: Error) => void) | null = null;
  const { writer, spy } = spyWriter((request) => {
    if (request.value === "A") {
      return new Promise((_resolve, reject) => {
        rejectFirst = reject;
      });
    }
    return Promise.resolve({ effective: { k: "B" } });
  });

  const first = writer.set("k", "A");
  const second = writer.set("k", "B");
  await second;
  expect(spy.pending.has("k")).toBe(false);

  rejectFirst!(new Error("too late"));
  await expect(first).rejects.toThrow("too late");
  // the stale failure must not revert or toast — B owns the key
  expect(spy.pending.has("k")).toBe(false);
  expect(spy.errors).toEqual([]);
  expect(spy.applied).toEqual([{ effective: { k: "B" } }]);
});

test("the latest write's own error reverts and reports", async () => {
  const { writer, spy } = spyWriter(() => Promise.reject(new Error("nope")));
  await expect(writer.set("k", "A")).rejects.toThrow("nope");
  expect(spy.pending.has("k")).toBe(false);
  expect(spy.errors).toEqual([{ key: "k", message: "nope" }]);
});

// ── the settings feed: pushes and reconnects both refetch ──

test("the settings feed refetches on pushes and on reconnect", () => {
  const pushHandlers = new Set<() => void>();
  const openHandlers = new Set<() => void>();
  const fakeSocket = {
    onSettingsPush: (handler: () => void) => {
      pushHandlers.add(handler);
      return () => pushHandlers.delete(handler);
    },
    onOpen: (handler: () => void) => {
      openHandlers.add(handler);
      return () => openHandlers.delete(handler);
    },
  };
  const refetches: string[] = [];
  const stop = createSettingsFeed(fakeSocket, () => refetches.push("refetch"));

  // a settings push while connected
  for (const handler of pushHandlers) handler();
  expect(refetches).toHaveLength(1);

  // a reconnect: pushes missed while offline are caught up
  for (const handler of openHandlers) handler();
  expect(refetches).toHaveLength(2);

  // unsubscribed: neither trigger reaches the page anymore
  stop();
  for (const handler of pushHandlers) handler();
  for (const handler of openHandlers) handler();
  expect(refetches).toHaveLength(2);
});

test("hasPreviews detects thumbnail options", () => {
  const withPreview: Field = {
    key: "vu.cycle",
    label: "Cycle",
    kind: {
      type: "ordered-subset",
      options: [
        { value: "round", label: "round", preview: "/vu-preview?skin=round&v=1" },
        { value: "night", label: "night" },
      ],
    },
  };
  expect(hasPreviews(withPreview)).toBe(true);
  const bare: Field = {
    key: "now-playing.quality",
    label: "Quality parts",
    kind: { type: "ordered-subset", options: [{ value: "codec", label: "Codec" }] },
  };
  expect(hasPreviews(bare)).toBe(false);
  const clock: Field = {
    key: "clock.style",
    label: "Clock style",
    kind: {
      type: "choice",
      options: [
        { value: "digital", label: "digital", preview: "/clock-preview?style=digital&v=1" },
      ],
    },
  };
  expect(hasPreviews(clock)).toBe(true);
  const number: Field = {
    key: "display.brightness",
    label: "Brightness",
    kind: { type: "number", min: 1, max: 100, step: 1, unit: "%" },
  };
  expect(hasPreviews(number)).toBe(false);
});
