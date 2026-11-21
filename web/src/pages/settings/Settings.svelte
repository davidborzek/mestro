<script lang="ts">
  import { onMount } from "svelte";
  import { layout } from "../../lib/layout.svelte";
  import {
    caps,
    fieldValue,
    deviceSectionFacts,
    loadSettings,
    providerStatusText,
    sectionTitleOf,
    sectionTitle,
    settingsStore,
    skinLabel,
    skinOf,
    startSettingsFeed,
    valueTexts,
    visibleField,
  } from "../../lib/settings.svelte";
  import { maSummary } from "../../lib/maConnection";
  import { currentOutput } from "../../lib/audio";
  import { audioOutputs, refreshAudioOutputs } from "../../lib/audioOutputs.svelte";
  import {
    AUDIO_SECTION_ID,
    availableDeviceSections,
    INFO_SECTION_ID,
    MA_SECTION_ID,
    openDeviceSection,
    SYSTEM_SECTION_ID,
    type DeviceSectionId,
  } from "../../lib/settingsSections";
  import { startSystemFacts, systemSummary } from "../../lib/systemFacts.svelte";
  import { readPath, sectionSummary, sectionVisible } from "../../lib/settingsSchema";
  import { remoteState } from "../../lib/remoteState.svelte";
  import { device } from "../../lib/connection.svelte";
  import { navigate, route, scrollable } from "../../lib/router.svelte";
  import { t } from "../../lib/i18n.svelte";
  import type { Field } from "../../lib/protocol";
  import Icon, { type IconName } from "../../components/Icon.svelte";
  import Link from "../../components/Link.svelte";
  import FieldRow from "./controls/FieldRow.svelte";
  import Audio from "./Audio.svelte";
  import Connection from "./Connection.svelte";
  import Info from "./Info.svelte";
  import System from "./System.svelte";

  // the device settings, rendered from its schema: a section
  // list (icon + current-value summary) and the field controls.
  // Phones push the detail, wide landscape keeps two panes. The
  // open section (and a VU skin's options) live in the URL.

  const twoPane = $derived(layout.shape === "D");
  const sections = $derived(
    (settingsStore.schema?.sections ?? []).filter((section) =>
      sectionVisible(section, fieldValue, caps()),
    ),
  );
  const selectedId = $derived(twoPane ? (route.section ?? sections[0]?.id ?? null) : route.section);
  const selected = $derived(sections.find((section) => section.id === selectedId) ?? null);
  const texts = $derived(valueTexts());

  // the fixed device sections after the schema's, in list order;
  // a route to one this build lacks is an unknown section
  const deviceSection = $derived(openDeviceSection(route.section, deviceSectionFacts()));
  const deviceRows = $derived(
    availableDeviceSections(deviceSectionFacts()).map((id) => ({
      id,
      icon: DEVICE_SECTION_ICONS[id],
      title: sectionTitleOf(id),
      summary: deviceSummary(id),
    })),
  );

  function deviceSummary(id: DeviceSectionId): string {
    if (id === AUDIO_SECTION_ID) return currentOutput(audioOutputs.outputs)?.name ?? "";
    if (id === SYSTEM_SECTION_ID) return systemSummary();
    return device.version;
  }

  // the Audio row's summary needs the output list before its page opens
  $effect(() => {
    if (device.audioOutput) refreshAudioOutputs().catch(() => {});
  });

  const summary = (section: (typeof sections)[number]) =>
    sectionSummary(section, fieldValue, caps(), texts);

  // the Music Assistant row's dim summary: connected · player,
  // else the (localized) provider error
  const connectionSummary = $derived.by(() => {
    if (!settingsStore.state) return "";
    return maSummary(
      {
        connected: remoteState.connected,
        player: String(readPath(settingsStore.state.config, "music-assistant.player") ?? ""),
        providerStatus: providerStatusText(settingsStore.state.provider_status),
      },
      { connected: t("ma.connected"), notConnected: t("ma.disconnected") },
    );
  });

  // the OSD context tri-states repeat the base switches: a
  // heading per context says what they follow. Purely
  // presentational — the schema stays flat.
  function groupOf(field: Field): { name: string; follow: boolean } | null {
    if (field.key.startsWith("osd.now-playing.")) {
      return { name: t("settings.groupNowPlaying"), follow: true };
    }
    if (field.key.startsWith("osd.vu.")) {
      return { name: t("settings.groupVu"), follow: true };
    }
    return null;
  }

  type Row = { group: { name: string; follow: boolean } | null; field: Field };

  // the per-skin VU options live one level deeper (opened from the
  // skin's row in the cycle list); the section shows the rest
  const rows = $derived.by(() => {
    const out: Row[] = [];
    let last: Row["group"] | undefined;
    for (const field of selected?.fields ?? []) {
      if (!visibleField(field)) continue;
      const skin = skinOf(field.key);
      if (route.skin ? skin !== route.skin : skin !== null) continue;
      const group = groupOf(field);
      out.push({ group: group !== null && group.name !== last?.name ? group : null, field });
      last = group;
    }
    return out;
  });

  function openSkin(skin: string): void {
    if (selectedId) navigate(`/settings/${selectedId}/${skin}`);
  }

  // a skin left the cycle while its options were open
  $effect(() => {
    if (route.skin && route.section && rows.length === 0 && settingsStore.loaded) {
      navigate(`/settings/${route.section}`, { replace: true });
    }
  });

  onMount(() => {
    void loadSettings();
    const stopFeed = startSettingsFeed();
    const stopSystemFacts = startSystemFacts();
    return () => {
      stopFeed();
      stopSystemFacts();
    };
  });

  // schema icon names (Material Symbols) → this app's icon set
  const ICONS: Record<string, IconName> = {
    music_note: "music",
    palette: "palette",
    bedtime: "moon",
    brightness_medium: "sun",
    dashboard: "home",
    notifications: "bell",
    graphic_eq: "vu",
  };
  const sectionIcon = (icon: string): IconName => ICONS[icon] ?? "settings";
  const DEVICE_SECTION_ICONS: Record<DeviceSectionId, IconName> = {
    audio: "speaker",
    system: "settings",
    info: "remote",
  };
</script>

{#snippet deviceBody(id: DeviceSectionId)}
  {#if id === AUDIO_SECTION_ID}
    <Audio />
  {:else if id === SYSTEM_SECTION_ID}
    <System />
  {:else if id === INFO_SECTION_ID}
    <Info />
  {/if}
{/snippet}

<main class="settings" class:twopane={twoPane}>
  {#if !settingsStore.loaded}
    {#if settingsStore.error}
      <p class="state">{t("settings.loadFailed")}</p>
      <button class="ghost" onclick={() => void loadSettings()}>
        {t("settings.retry")}
      </button>
    {:else}
      <p class="state">{t("app.loading")}</p>
    {/if}
  {:else if !twoPane && route.section === MA_SECTION_ID}
    <!-- phone: the fixed connection section, the top bar holds its title -->
    <section class="fixed" use:scrollable={"settings-fields"}>
      <Connection />
    </section>
  {:else if !twoPane && deviceSection}
    <section class="fixed" use:scrollable={"settings-fields"}>
      {@render deviceBody(deviceSection)}
    </section>
  {:else if !twoPane && selected}
    <!-- phone: the section detail, the top bar holds its title -->
    <section class="fields" use:scrollable={"settings-fields"}>
      {#each rows as row (row.field.key)}
        {#if row.group}
          <div class="group">
            <span class="group-name">{row.group.name}</span>
            {#if row.group.follow}
              <span class="group-help">{t("settings.groupFollows")}</span>
            {/if}
          </div>
        {/if}
        <FieldRow field={row.field} onopenskin={openSkin} />
      {/each}
    </section>
  {:else}
    <nav class="list" aria-label={t("settings.title")} use:scrollable={"settings-list"}>
      <Link
        class="row{MA_SECTION_ID === selectedId ? ' active' : ''}"
        href={`/settings/${MA_SECTION_ID}`}
      >
        <span class="icon"><Icon name="ma" size={22} /></span>
        <span class="meta">
          <span class="name">{t("ma.section")}</span>
          {#if connectionSummary}
            <span class="summary">{connectionSummary}</span>
          {/if}
        </span>
        <span class="chev"><Icon name="chevronRight" size={18} /></span>
      </Link>
      {#each sections as section (section.id)}
        <Link
          class="row{section.id === selectedId ? ' active' : ''}"
          href={`/settings/${section.id}`}
        >
          <span class="icon"><Icon name={sectionIcon(section.icon)} size={22} /></span>
          <span class="meta">
            <span class="name">{sectionTitle(section.title, section.id)}</span>
            {#if summary(section)}
              <span class="summary">{summary(section)}</span>
            {/if}
          </span>
          <span class="chev"><Icon name="chevronRight" size={18} /></span>
        </Link>
      {/each}
      {#each deviceRows as row (row.id)}
        <Link class="row{row.id === selectedId ? ' active' : ''}" href={`/settings/${row.id}`}>
          <span class="icon"><Icon name={row.icon} size={22} /></span>
          <span class="meta">
            <span class="name">{row.title}</span>
            {#if row.summary}
              <span class="summary">{row.summary}</span>
            {/if}
          </span>
          <span class="chev"><Icon name="chevronRight" size={18} /></span>
        </Link>
      {/each}
    </nav>
    {#if twoPane}
      {#if route.section === MA_SECTION_ID}
        <section class="fixed" use:scrollable={"settings-fields"}>
          <h2 class="pane-title">{sectionTitleOf(MA_SECTION_ID)}</h2>
          <Connection />
        </section>
      {:else if deviceSection}
        <section class="fixed" use:scrollable={"settings-fields"}>
          <h2 class="pane-title">{sectionTitleOf(deviceSection)}</h2>
          {@render deviceBody(deviceSection)}
        </section>
      {:else if selected}
        <section class="fields" use:scrollable={"settings-fields"}>
          {#if route.skin}
            <button
              class="pane-back"
              onclick={() => navigate(route.section ? `/settings/${route.section}` : "/settings")}
            >
              <Icon name="chevronLeft" size={18} />
              {sectionTitle(selected.title, selected.id)}
            </button>
            <h2 class="pane-title">{skinLabel(route.skin)}</h2>
          {:else}
            <h2 class="pane-title">{sectionTitle(selected.title, selected.id)}</h2>
          {/if}
          {#each rows as row (row.field.key)}
            {#if row.group}
              <div class="group">
                <span class="group-name">{row.group.name}</span>
                {#if row.group.follow}
                  <span class="group-help">{t("settings.groupFollows")}</span>
                {/if}
              </div>
            {/if}
            <FieldRow field={row.field} onopenskin={openSkin} />
          {/each}
        </section>
      {:else}
        <p class="state">{t("settings.chooseSection")}</p>
      {/if}
    {/if}
  {/if}
</main>

{#if settingsStore.toast}
  <div class="toast" role="status">{settingsStore.toast}</div>
{/if}

<style>
  .settings {
    width: 100%;
    max-width: 560px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
    padding: var(--sp-2) max(24px, var(--safe-l)) calc(var(--sp-7) + var(--safe-b));
  }
  .state {
    padding: var(--sp-4) 0;
    font-size: 15px;
    color: var(--dim);
    text-align: center;
  }

  /* ── section list ── */
  .list {
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }
  /* the row markup lives in the Link component, so its styles
     reach through the component boundary */
  .list :global(.row) {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    min-height: 64px;
    padding: var(--sp-2) var(--sp-4);
    border: 1px solid var(--hairline);
    border-radius: var(--r-14);
    background: var(--glass);
    text-align: left;
    transition:
      transform var(--t-press) var(--ease),
      background var(--t-press) var(--ease);
  }
  .list :global(.row:active) {
    transform: scale(0.98);
  }
  .icon {
    flex: 0 0 auto;
    width: 28px;
    display: flex;
    justify-content: center;
    color: var(--accent-live);
  }
  .chev {
    flex: 0 0 auto;
    color: var(--mute);
  }
  .meta {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .name {
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
  }
  .summary {
    font-size: 13px;
    color: var(--mute);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .list :global(.row.active) {
    background: color-mix(in srgb, var(--accent-live) 10%, transparent);
    border-color: color-mix(in srgb, var(--accent-live) 40%, transparent);
  }

  /* ── the fixed sections (connection, device): stacked cards ── */
  .fixed {
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }

  /* ── field list ── */
  .fields {
    background: var(--glass);
    border: 1px solid var(--hairline);
    border-radius: var(--r-14);
    padding: var(--sp-2) var(--sp-4);
  }
  .pane-title {
    font-size: 16px;
    font-weight: 650;
    padding: var(--sp-2) 0 var(--sp-3);
  }
  .fields > :global(.field + .field) {
    border-top: 1px solid var(--hairline);
  }
  .pane-back {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 2px;
    min-height: 36px;
    padding: 0 var(--sp-2) 0 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--accent-live);
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding-top: var(--sp-3);
    margin-top: var(--sp-2);
    border-top: 1px solid var(--hairline);
  }
  .group-name {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.8px;
    text-transform: uppercase;
    color: var(--mute);
  }
  .group-help {
    font-size: 12px;
    color: var(--mute);
  }

  /* ── wide landscape: sections left, fields right ── */
  .twopane {
    max-width: 960px;
    display: grid;
    grid-template-columns: minmax(280px, 380px) minmax(0, 560px);
    gap: var(--sp-5);
    align-items: start;
    justify-content: center;
  }

  .toast {
    position: fixed;
    left: 50%;
    bottom: calc(var(--sp-4) + var(--safe-b));
    transform: translateX(-50%);
    max-width: min(90vw, 480px);
    padding: var(--sp-3) var(--sp-4);
    border-radius: var(--r-14);
    background: var(--panel);
    border: 1px solid var(--hairline);
    font-size: 14px;
    color: var(--danger);
    z-index: 50;
  }
</style>
