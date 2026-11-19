// the settings page's data: the device's schema plus the
// effective state, with optimistic writes layered on top.
// Writes go through the settings set/reset ops; a settings push
// (any client's write, including this one) triggers a refetch.

import type { Field, Schema, StateData } from "./protocol";
import { device, socket } from "./connection.svelte";
import {
  createFieldWriter,
  createSettingsFeed,
  fieldVisible,
  readPath,
  type Capabilities,
  type ValueTexts,
} from "./settingsSchema";
import { t, tf } from "./i18n.svelte";
import { localizeProviderStatus, PROVIDER_DETAILS } from "./maConnection";
import { route } from "./router.svelte";

export const settingsStore = $state({
  schema: null as Schema | null,
  state: null as StateData | null,
  loaded: false,
  loading: false,
  // optimistic layer: dotted key → value being written. null is
  // a real "cleared to inherit" write, not a missing entry.
  pending: {} as Record<string, unknown>,
  toast: "",
  // the initial schema/state read failed (retry offered)
  error: "",
});

let toastTimer: ReturnType<typeof setTimeout> | null = null;

function showToast(message: string): void {
  settingsStore.toast = message;
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (settingsStore.toast = ""), 4000);
}

function reason(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function fieldValue(key: string): unknown {
  if (key in settingsStore.pending) return settingsStore.pending[key];
  return readPath(settingsStore.state?.effective, key);
}

export function isOverridden(key: string): boolean {
  return settingsStore.state?.overrides.includes(key) ?? false;
}

export function caps(): Capabilities {
  return { display: device.display };
}

export function visibleField(field: Field): boolean {
  return fieldVisible(field, fieldValue, caps());
}

// localized short values for summaries and choice rows
export function valueTexts(): ValueTexts {
  return {
    on: t("settings.on"),
    off: t("settings.off"),
    inherit: t("settings.inherit"),
    optionLabel: (option) => tf(`settings.option.${option.value}`, option.label),
  };
}

// the per-skin VU fields are dynamic (the cycle picks the skins),
// so they override via the shared settings.vu.<suffix> entries
const VU_FIELD = /^vu\.[a-z0-9-]+\.(info|glow|meters|gradient)$/;

export function fieldLabel(field: Field): string {
  const match = VU_FIELD.exec(field.key);
  return tf(match ? `settings.vu.${match[1]}` : `settings.${field.key}`, field.label);
}

export function sectionTitle(title: string, id: string): string {
  return tf(`settings.section.${id}`, title);
}

// the VU skin a per-skin field belongs to, or null
export function skinOf(key: string): string | null {
  return /^vu\.([a-z0-9-]+)\.(info|glow|meters|gradient)$/.exec(key)?.[1] ?? null;
}

/// a skin's display name from the vu.cycle options
export function skinLabel(skin: string): string {
  const cycle = settingsStore.schema?.sections
    .find((section) => section.id === "vu")
    ?.fields.find((field) => field.key === "vu.cycle");
  const option =
    cycle && cycle.kind.type === "ordered-subset"
      ? cycle.kind.options.find((candidate) => candidate.value === skin)
      : undefined;
  const label = option ? valueTexts().optionLabel(option) : skin;
  return label.charAt(0).toUpperCase() + label.slice(1);
}

// hand-built settings sections the served schema does not
// carry: the MA connection (listed first) and the device info
// (listed last) — both render fixed pages, not schema fields
export const MA_SECTION_ID = "connection";
export const INFO_SECTION_ID = "info";

// a section's localized title, falling back to the page title
export function sectionTitleOf(id: string | null): string {
  if (id === MA_SECTION_ID) return t("ma.section");
  if (id === INFO_SECTION_ID) return t("info.section");
  const section = settingsStore.schema?.sections.find((candidate) => candidate.id === id);
  return section ? sectionTitle(section.title, section.id) : t("settings.title");
}

// the top bar's settings heading: the open skin's or section's
// name, the page title otherwise
export function selectedSectionTitle(): string {
  if (route.skin) return skinLabel(route.skin);
  return sectionTitleOf(route.section);
}

async function refetchState(): Promise<void> {
  settingsStore.state = await socket.send<StateData>({ op: "settings", action: "state" });
}

async function refetchSchema(): Promise<void> {
  settingsStore.schema = await socket.send<Schema>({ op: "settings", action: "schema" });
}

// the set/reset reply is the new state payload; anything else
// (an older server) falls back to a read
async function applyReply(reply: unknown): Promise<void> {
  if (reply && typeof reply === "object" && "effective" in reply) {
    settingsStore.state = reply as StateData;
  } else {
    await refetchState();
  }
}

export async function loadSettings(): Promise<void> {
  settingsStore.loading = true;
  try {
    await Promise.all([refetchSchema(), refetchState()]);
    settingsStore.loaded = true;
    settingsStore.error = "";
  } catch (error) {
    settingsStore.error = reason(error);
    showToast(t("settings.saveFailed", { reason: settingsStore.error }));
  } finally {
    settingsStore.loading = false;
  }
}

// optimistic writes with per-key generations (see
// createFieldWriter): stale replies never revert a newer edit
const writer = createFieldWriter({
  send: (request) => socket.send<StateData>(request),
  applyReply,
  setPending: (key, value) => {
    if (value === undefined) delete settingsStore.pending[key];
    else settingsStore.pending[key] = value;
  },
  onError: (_key, message) => showToast(t("settings.saveFailed", { reason: message })),
});

// the VU section is built from the configured cycle, so a cycle
// write also refreshes the schema
function write(op: () => Promise<void>, key: string): Promise<void> {
  return op()
    .then(() => (key === "vu.cycle" ? refetchSchema() : undefined))
    .catch(() => {
      // the writer already reverted and toasted
    });
}

export function setField(key: string, value: unknown): Promise<void> {
  return write(() => writer.set(key, value), key);
}

// `optimistic` pre-shows the cleared value (inherit-bool switches
// to Inherit immediately); other kinds wait for the reply
export function resetField(key: string, optimistic?: unknown): Promise<void> {
  return write(() => writer.reset(key, optimistic), key);
}

// settings pushes carry no payload — refetch state and schema,
// coalescing bursts (set + its own push) into one round trip
let refetching = false;
let refetchQueued = false;

function scheduleRefetch(): void {
  if (refetching) {
    refetchQueued = true;
    return;
  }
  refetching = true;
  void (async () => {
    try {
      do {
        refetchQueued = false;
        await refetchState();
        await refetchSchema();
      } while (refetchQueued);
    } catch {
      // the socket retries on its own; the banner covers the gap
    } finally {
      refetching = false;
    }
  })();
}

export function startSettingsFeed(): () => void {
  // pushes cover the connected moments; every (re)connect
  // catches up on whatever was missed while the socket was down
  return createSettingsFeed(socket, scheduleRefetch);
}

/// the provider status in the UI language (known lead-ins and
/// details translated, the rest stays technical)
export function providerStatusText(status: string): string {
  const details = Object.fromEntries(
    Object.entries(PROVIDER_DETAILS).map(([raw, key]) => [raw, t(key)]),
  );
  return localizeProviderStatus(
    status,
    { unreachable: t("ma.providerUnreachable"), auth: t("ma.providerAuth") },
    details,
  );
}
