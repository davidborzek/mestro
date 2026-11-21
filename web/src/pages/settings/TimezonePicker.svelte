<script lang="ts">
  import { onMount } from "svelte";
  import { socket } from "../../lib/connection.svelte";
  import { t } from "../../lib/i18n.svelte";
  import { filterZones, loadZones, zoneLabel, type TimezoneSend } from "../../lib/timezone";
  import type { TimezoneSetData } from "../../lib/protocol";
  import Icon from "../../components/Icon.svelte";
  import Sheet from "../../components/Sheet.svelte";

  // the timezone row of the Info section: the zone the player runs
  // in, a searchable list of the device's zoneinfo. Choosing one
  // writes it on the device, which restarts the player to apply it
  // (no unsafe tzset) — the reconnect banner covers the gap.

  const send: TimezoneSend = (request) => socket.send(request);

  let zones = $state<string[]>([]);
  let chosen = $state("");
  let current = $state<string | null>(null);
  let loaded = $state(false);
  let open = $state(false);
  let query = $state("");
  let applying = $state(false);
  let error = $state("");

  const shown = $derived(filterZones(zones, query));
  // the explicit choice, else the zone the player runs in (automatic)
  const label = $derived(
    chosen ? zoneLabel(chosen) : current ? zoneLabel(current) : t("timezone.unknown"),
  );

  onMount(() => {
    loadZones(send)
      .then((data) => {
        zones = data.zones;
        chosen = data.chosen;
        current = data.current;
      })
      .catch((cause: unknown) => (error = cause instanceof Error ? cause.message : String(cause)))
      .finally(() => (loaded = true));
  });

  async function pick(zone: string) {
    open = false;
    if (zone === (chosen || current)) return;
    applying = true;
    error = "";
    try {
      const reply = (await send({ op: "timezone", action: "set", zone })) as TimezoneSetData;
      chosen = reply.zone;
      // the player restarts when the running zone changed — keep the
      // pill up until the connection drops and comes back
      if (!reply.restarting) applying = false;
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
      applying = false;
    }
  }
</script>

{#if loaded && zones.length > 0}
  <button class="changerow" disabled={applying} onclick={() => ((query = ""), (open = true))}>
    <span class="label">{t("timezone.row")}</span>
    <span class="value">
      {#if applying}
        <span class="dot pulse"></span> {t("timezone.applying")}
      {:else}
        {label}
        {#if !chosen}<span class="auto">{t("timezone.auto")}</span>{/if}
      {/if}
    </span>
    <Icon name="chevronRight" size={18} />
  </button>
{/if}
{#if error}
  <div class="err" role="alert"><Icon name="alert" size={16} /> {error}</div>
{/if}

<Sheet bind:open title={t("timezone.title")}>
  <input
    class="search"
    type="search"
    placeholder={t("timezone.search")}
    aria-label={t("timezone.search")}
    bind:value={query}
  />
  <div class="options">
    {#each shown as zone (zone)}
      <button
        class="option"
        class:active={zone === (chosen || current)}
        onclick={() => void pick(zone)}
      >
        <span class="option-label">{zoneLabel(zone)}</span>
        {#if zone === (chosen || current)}
          <Icon name="check" size={20} />
        {/if}
      </button>
    {:else}
      <p class="hint">{t("timezone.none")}</p>
    {/each}
  </div>
  <p class="hint">{t("timezone.hint")}</p>
</Sheet>

<style>
  .changerow {
    min-height: 44px;
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding-top: var(--sp-2);
    border-top: 1px solid var(--hairline);
    font-size: 15px;
    color: var(--text);
  }
  .label {
    flex: 0 0 auto;
    color: var(--dim);
  }
  .value {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: var(--sp-2);
    font-weight: 600;
    text-align: right;
    overflow-wrap: anywhere;
  }
  .auto {
    font-size: 12px;
    font-weight: 500;
    color: var(--mute);
  }
  .changerow :global(svg) {
    flex: 0 0 auto;
    color: var(--mute);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 4px;
    background: var(--accent-live);
  }
  .dot.pulse {
    animation: tz-pulse 1.2s ease-in-out infinite;
  }
  @keyframes tz-pulse {
    50% {
      opacity: 0.3;
    }
  }
  .search {
    width: 100%;
    min-height: 44px;
    margin-bottom: var(--sp-2);
    padding: 0 var(--sp-3);
    border: 1px solid var(--hairline);
    border-radius: var(--r-8);
    background: var(--pill);
    font: inherit;
    font-size: 16px;
    color: var(--text);
  }
  .options {
    display: flex;
    flex-direction: column;
    max-height: 50vh;
    overflow-y: auto;
  }
  .option {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
    min-height: 44px;
    padding: 0 var(--sp-2);
    border-radius: var(--r-8);
    font-size: 15px;
    color: var(--text);
    text-align: left;
  }
  .option.active {
    color: var(--accent-live);
    font-weight: 650;
  }
  .option:active {
    background: var(--pill);
  }
  .hint {
    margin-top: var(--sp-3);
    font-size: 13px;
    color: var(--dim);
  }
</style>
