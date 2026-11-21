<script lang="ts">
  import { connection, device } from "../../lib/connection.svelte";
  import { diagnostics } from "../../lib/diagnostics.svelte";
  import { downloadDiagnostics } from "../../lib/diagnosticsDownload";
  import { remoteState } from "../../lib/remoteState.svelte";
  import { getLocale, setLocale, t, type Locale } from "../../lib/i18n.svelte";
  import Update from "../update/Update.svelte";
  import Icon from "../../components/Icon.svelte";
  import Segmented from "../../components/Segmented.svelte";

  // the hand-built Info section (the schema serves no device
  // facts): version, player, update, the app's language and the
  // diagnostics. Where the player lives (wifi, timezone) and the
  // restart/reset are in the System section.

  const playerName = $derived(remoteState.player.name || device.configPlayer);

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
  </dl>
</section>

{#if device.ota}
  <Update />
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
</style>
