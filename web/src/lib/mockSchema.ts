// dev-only mock of the device's settings schema: the golden JSON
// the backend serves (src/settings/schema.rs), with the dynamic
// VU section rebuilt from the configured cycle like the device
// does, and the preview URLs rewritten to real renders (the mock
// has no device behind it). Plus the effective config the mock
// reports. Imported only by mock.ts (DEV builds).

import type { Field, Option, Schema, Section } from "./protocol";
import goldenJson from "./mockSchema.json";

const golden = goldenJson as Schema;

// real preview renders exported by the Rust test
// (tests/preview_export.rs → web/src/lib/mock-previews). The
// device's /vu-preview and /clock-preview endpoints cannot run
// against the mock, so the schema's URLs become asset URLs.
const previewAssets = import.meta.glob("./mock-previews/*.png", {
  eager: true,
  query: "?url",
  import: "default",
}) as Record<string, string>;

const PREVIEW_URL = /^\/(vu|clock)-preview\?(?:skin|style)=([^&]+)/;

const assetUrl = (name: string): string | undefined => previewAssets[`./mock-previews/${name}.png`];

// one option: its preview URL points at the local asset, or drops
// the preview when no render exists (no broken thumbnails)
function localize(option: Option): Option {
  const match = option.preview?.match(PREVIEW_URL);
  if (!match) return option;
  const asset = assetUrl(decodeURIComponent(match[2]));
  return asset ? { ...option, preview: asset } : { ...option, preview: undefined };
}

function localizeOptions(field: Field): Field {
  const kind = field.kind;
  if (kind.type === "choice" || kind.type === "ordered-subset" || kind.type === "permutation") {
    return { ...field, kind: { ...kind, options: kind.options.map(localize) } };
  }
  return field;
}

// one field group per skin in the configured cycle: everything
// except `vu.cycle` itself is scoped to a skin
export function mockSchema(cycle: string[]): Schema {
  const sections = golden.sections.map((section) =>
    section.id === "vu"
      ? ({
          ...section,
          fields: section.fields
            .filter((field) => {
              if (field.key === "vu.cycle") return true;
              const skin = field.key.split(".")[1];
              return cycle.includes(skin);
            })
            .map(localizeOptions),
        } satisfies Section)
      : { ...section, fields: section.fields.map(localizeOptions) },
  );
  return { sections };
}

// every skin the cycle can pick, in the golden's default order
const cycleField = golden.sections
  .find((section) => section.id === "vu")
  ?.fields.find((field) => field.key === "vu.cycle");
export const VU_SKINS: string[] =
  cycleField && cycleField.kind.type === "ordered-subset"
    ? cycleField.kind.options.map((option) => option.value)
    : [];

// the effective config the mock reports: defaults folded in,
// inherit-bools that follow the base as null
export const MOCK_DEFAULTS: Record<string, unknown> = {
  "now-playing": {
    layout: "side",
    "inline-vu": "off",
    font: {
      "title-size": 28,
      "artist-size": 16,
      "album-size": 14,
      "quality-size": 12,
      "title-weight": 700,
      "artist-weight": 500,
      "album-weight": 400,
      "quality-weight": 400,
    },
    controls: ["shuffle", "previous", "play-pause", "next", "repeat"],
    quality: ["codec", "rate", "depth", "bitrate", "channels"],
  },
  theme: { palette: "amber", variant: "tonal-spot" },
  ui: { rail: "glass", auto_open_now_playing: true, osk_layout: "de" },
  background: { blurred_cover: true },
  clock: {
    style: "digital",
    background: { source: "black", dim: 0.35, blur: true, interval: 15 },
  },
  display: { brightness: 65, "standby-brightness": 20 },
  audio: { output: "", name: "", server: "", extra: "" },
  hub: { layout: "cards", order: ["library", "search", "settings", "standby"] },
  osd: {
    style: "translucent",
    position: "top",
    "volume-position": "bottom",
    "toast-duration": 2500,
    "volume-duration": 1200,
    transport: true,
    track: true,
    queue: true,
    volume: true,
    "now-playing": { transport: true, track: true, queue: true, volume: true },
    vu: { transport: null, track: null, queue: null, volume: null },
  },
  vu: {
    cycle: VU_SKINS,
    night: { info: ["title", "artist"] },
    cream: { info: ["title", "artist"], glow: 0.4 },
    round: { info: ["title", "artist"], glow: 0.3 },
    "wall-light": { glow: 0.5 },
    spectrum: { info: ["codec", "quality"], meters: true, gradient: false },
    bars: { info: ["codec", "quality"], meters: true, gradient: false },
  },
};

// a few realistic overrides so the Reset buttons show up
export const MOCK_OVERRIDES: Record<string, unknown> = {
  "now-playing": { layout: "fullbleed" },
  osd: { style: "solid" },
  vu: { cycle: ["arc", "cream", "night", "spectrum", "wall"] },
};
