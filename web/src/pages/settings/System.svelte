<script lang="ts">
  import { device, socket } from "../../lib/connection.svelte";
  import { t } from "../../lib/i18n.svelte";
  import { systemBlocks } from "../../lib/settingsSections";
  import { wifiFacts } from "../../lib/systemFacts.svelte";
  import Icon from "../../components/Icon.svelte";
  import ConfirmSheet from "../../components/ConfirmSheet.svelte";
  import ResetSheet from "../../components/ResetSheet.svelte";
  import { navigate } from "../../lib/router.svelte";
  import TimezonePicker from "./TimezonePicker.svelte";

  // the hand-built System section: where the player is (timezone,
  // wifi) and the two things that restart or wipe it. The facts
  // behind the list summary live in lib/systemFacts.

  const blocks = $derived(systemBlocks(device));

  let confirming = $state(false);
  let restarting = $state(false);
  let resetSheet = $state(false);
  let resetting = $state(false);

  async function restart() {
    restarting = true;
    try {
      await socket.send({ op: "settings", action: "restart" });
    } catch {
      // the socket drops while the kiosk reboots — the banner
      // covers the gap and the socket reconnects on its own
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

{#if blocks.network}
  <section class="card">
    <h2>{t("system.network")}</h2>
    {#if blocks.wifi}
      <dl>
        <div class="line">
          <dt>{t("wifi.title")}</dt>
          <dd>{wifiFacts.ssid}</dd>
        </div>
      </dl>
      <button class="changerow" onclick={() => navigate("/setup/wifi")}>
        {t("wifi.change")}
        <Icon name="chevronRight" size={18} />
      </button>
    {/if}
    {#if blocks.timezone}
      <TimezonePicker />
    {/if}
  </section>
{/if}

<section class="card">
  {#if restarting}
    <span class="pill"><span class="dot pulse"></span> {t("system.restarting")}</span>
  {:else}
    <button class="restart" onclick={() => (confirming = true)}>
      <Icon name="restart" size={20} />
      {t("system.restart")}
    </button>
  {/if}
</section>

{#if blocks.factoryReset}
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
  title={t("system.restart")}
  message={t("system.restartConfirm")}
  confirmLabel={t("system.restart")}
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
  /* the first row under the title needs no rule above it */
  h2 + :global(.changerow) {
    margin-top: 0;
    padding-top: 0;
    border-top: 0;
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
  .zone {
    color: var(--danger);
  }
  .dangerzone {
    border-color: color-mix(in srgb, var(--danger) 40%, transparent);
  }
</style>
