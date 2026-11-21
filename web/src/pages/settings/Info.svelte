<script lang="ts">
  import { onMount } from "svelte";
  import { connection, device, socket } from "../../lib/connection.svelte";
  import { diagnostics } from "../../lib/diagnostics.svelte";
  import { downloadDiagnostics } from "../../lib/diagnosticsDownload";
  import { remoteState } from "../../lib/remoteState.svelte";
  import { getLocale, setLocale, t, type Locale } from "../../lib/i18n.svelte";
  import type { WifiStatusData } from "../../lib/protocol";
  import Update from "../update/Update.svelte";
  import AudioOutput from "./AudioOutput.svelte";
  import TimezonePicker from "./TimezonePicker.svelte";
  import Icon from "../../components/Icon.svelte";
  import Segmented from "../../components/Segmented.svelte";
  import ConfirmSheet from "../../components/ConfirmSheet.svelte";
  import ResetSheet from "../../components/ResetSheet.svelte";
  import { navigate } from "../../lib/router.svelte";

  // the hand-built Info section (the schema serves no device
  // facts): version, player, wifi, update, the app's language and
  // the restart — the former System hub, folded into settings.

  const playerName = $derived(remoteState.player.name || device.configPlayer);

  let confirming = $state(false);
  let restarting = $state(false);
  let wifiSsid = $state("");
  let resetSheet = $state(false);
  let resetting = $state(false);
  let diagOpen = $state(false);
  let downloading = $state(false);
  let downloadError = $state("");

  // the Diagnose row: raw numbers, no i18n — a device-side
  // latency reading is the same in every language
  const keepalive = $derived(
    diagnostics.keepaliveMs === null ? "—" : `${diagnostics.keepaliveMs} ms`,
  );
  const commandLines = $derived(
    diagnostics.commands.map(
      (command) =>
        `${command.label}: reply ${command.replyMs ?? "…"} ms · push ${command.confirmMs ?? "…"} ms${command.error ? ` · ${command.error}` : ""}`,
    ),
  );

  onMount(() => {
    if (!device.wifi) return;
    // the wifi row's current network — a status error just leaves
    // the row empty, nothing here blocks the page
    socket
      .send<WifiStatusData>({ op: "wifi", action: "status" })
      .then((status) => (wifiSsid = status.ssid || status.ap_ssid))
      .catch(() => {});
  });

  async function restart() {
    restarting = true;
    try {
      await socket.send({ op: "settings", action: "restart" });
    } catch {
      // the socket drops while the kiosk reboots — the banner
      // covers the gap and the socket reconnects on its own
    }
  }

  // the bug-report file: the player collects it, the browser saves it
  async function downloadReport() {
    downloading = true;
    downloadError = "";
    try {
      const file = await downloadDiagnostics();
      const url = URL.createObjectURL(file.blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = file.name;
      link.click();
      URL.revokeObjectURL(url);
    } catch (error) {
      downloadError = error instanceof Error ? error.message : String(error);
    } finally {
      downloading = false;
    }
  }

  async function factoryReset(wifiToo: boolean) {
    resetting = true;
    try {
      await socket.send({ op: "settings", action: "factory-reset", wifi: wifiToo });
    } catch {
      // the socket drops while the kiosk restarts; the reconnect's
      // handshake sees the cookie gone and lands on the pair screen
    }
  }
</script>

<section class="card">
  <h2>{t("info.section")}</h2>
  <dl>
    <div class="line">
      <dt>{t("info.version")}</dt>
      <dd>{device.version}</dd>
    </div>
    {#if playerName}
      <div class="line">
        <dt>{t("info.player")}</dt>
        <dd>{playerName}</dd>
      </div>
    {/if}
    {#if device.wifi}
      <div class="line">
        <dt>{t("wifi.title")}</dt>
        <dd>{wifiSsid}</dd>
      </div>
    {/if}
  </dl>
  {#if device.wifi}
    <button class="changerow" onclick={() => navigate("/setup/wifi")}>
      {t("wifi.change")}
      <Icon name="chevronRight" size={18} />
    </button>
  {/if}
  {#if device.timezone}
    <TimezonePicker />
  {/if}
</section>

{#if device.ota}
  <Update />
{/if}

{#if device.audioOutput}
  <AudioOutput />
{/if}

<section class="card">
  <h2>{t("info.app")}</h2>
  <Segmented
    label={t("info.language")}
    active={[getLocale()]}
    onselect={(locale: Locale) => setLocale(locale)}
    items={[
      { value: "de", label: "Deutsch" },
      { value: "en", label: "English" },
    ]}
  />
</section>

<section class="card">
  {#if restarting}
    <span class="pill"><span class="dot pulse"></span> {t("info.restarting")}</span>
  {:else}
    <button class="restart" onclick={() => (confirming = true)}>
      <Icon name="restart" size={20} />
      {t("info.restart")}
    </button>
  {/if}
</section>

<section class="card">
  <button class="diag" aria-expanded={diagOpen} onclick={() => (diagOpen = !diagOpen)}>
    {t("info.diag")}
    <Icon name={diagOpen ? "chevronDown" : "chevronRight"} size={18} />
  </button>
  <button class="download" disabled={downloading} onclick={() => void downloadReport()}>
    <Icon name="download" size={18} />
    {downloading ? t("info.diagDownloading") : t("info.diagDownload")}
  </button>
  <p class="diag-help">{t("info.diagDownloadHelp")}</p>
  {#if downloadError}
    <div class="err" role="alert">
      <Icon name="alert" size={16} />
      {t("info.diagDownloadFailed", { reason: downloadError })}
    </div>
  {/if}
  {#if diagOpen}
    <dl class="diag-data">
      <div class="line">
        <dt>{t("info.diagSocket")}</dt>
        <dd>{connection.status}</dd>
      </div>
      <div class="line">
        <dt>{t("info.diagReconnects")}</dt>
        <dd>{diagnostics.reconnects}</dd>
      </div>
      <div class="line">
        <dt>{t("info.diagKeepalive")}</dt>
        <dd>{keepalive}</dd>
      </div>
    </dl>
    {#if commandLines.length > 0}
      <pre class="diag-commands">{commandLines.join("\n")}</pre>
    {:else}
      <p class="diag-empty">{t("info.diagNoCommands")}</p>
    {/if}
  {/if}
</section>

{#if device.factoryReset}
  <section class="card dangerzone">
    <h2 class="zone">{t("reset.zone")}</h2>
    {#if resetting}
      <span class="pill"><span class="dot pulse"></span> {t("reset.running")}</span>
    {:else}
      <button class="reset" onclick={() => (resetSheet = true)}>
        <Icon name="restart" size={20} />
        {t("reset.title")}
      </button>
    {/if}
  </section>
{/if}

<ConfirmSheet
  bind:open={confirming}
  title={t("info.restart")}
  message={t("info.restartConfirm")}
  confirmLabel={t("info.restart")}
  danger
  onconfirm={restart}
/>

<ResetSheet bind:open={resetSheet} onconfirm={factoryReset} />

<style>
  dl {
    display: flex;
    flex-direction: column;
  }
  .line {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
    min-height: 44px;
  }
  dt {
    font-size: 15px;
    color: var(--dim);
  }
  dd {
    font-size: 15px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    text-align: right;
    overflow-wrap: anywhere;
  }
  .changerow {
    min-height: 44px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-2);
    margin-top: calc(-1 * var(--sp-1));
    padding-top: var(--sp-2);
    border-top: 1px solid var(--hairline);
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
  }
  .changerow :global(svg) {
    color: var(--mute);
  }
  .restart,
  .reset {
    min-height: 48px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--sp-2);
    font-size: 15px;
    font-weight: 600;
    color: var(--danger);
  }
  .diag {
    min-height: 48px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-2);
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
  }
  .diag :global(svg) {
    color: var(--mute);
  }
  .diag-data {
    margin-top: var(--sp-1);
  }
  .download {
    min-height: 44px;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
    border-top: 1px solid var(--hairline);
  }
  .download :global(svg) {
    color: var(--mute);
  }
  .download:disabled {
    color: var(--mute);
  }
  .diag-help {
    font-size: 13px;
    color: var(--dim);
  }
  .diag-commands {
    margin: var(--sp-2) 0 0;
    padding: var(--sp-2);
    border: 1px solid var(--hairline);
    border-radius: var(--r-8);
    background: color-mix(in srgb, var(--pill) 60%, transparent);
    font-family: ui-monospace, monospace;
    font-size: 12px;
    line-height: 1.6;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .diag-empty {
    margin-top: var(--sp-2);
    font-size: 13px;
    color: var(--dim);
  }
  .zone {
    color: var(--danger);
  }
  .dangerzone {
    border-color: color-mix(in srgb, var(--danger) 40%, transparent);
  }
</style>
