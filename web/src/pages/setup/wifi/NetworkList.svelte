<script lang="ts">
  import type { WifiNetwork } from "../../../lib/protocol";
  import { signalBars } from "../../../lib/wifi";
  import { t } from "../../../lib/i18n.svelte";
  import Icon from "../../../components/Icon.svelte";
  import SignalBars from "../../../components/wifi/SignalBars.svelte";

  // the scanned networks: strongest first, one row per SSID. A
  // secured one opens the password sheet, an open one joins
  // straight away; hidden SSIDs go through the manual sheet. The
  // network the player is already on is marked and not offered.
  let {
    networks,
    scanning,
    current = "",
    onpick,
    onother,
    onrefresh,
  }: {
    networks: WifiNetwork[];
    scanning: boolean;
    current?: string;
    onpick: (network: WifiNetwork) => void;
    onother: () => void;
    onrefresh: () => void;
  } = $props();
</script>

<section class="card">
  <header class="head">
    <h2>{t("wifi.networks")}</h2>
    <button class="refresh" onclick={onrefresh} disabled={scanning} aria-label={t("wifi.refresh")}>
      <Icon name="restart" size={18} />
    </button>
  </header>
  {#if scanning}
    <p class="state"><span class="dot pulse" aria-hidden="true"></span> {t("wifi.scanning")}</p>
  {:else if networks.length === 0}
    <p class="state">{t("wifi.empty")}</p>
  {:else}
    <ul class="nets">
      {#each networks as network (network.ssid)}
        <li>
          <button class="net" disabled={network.ssid === current} onclick={() => onpick(network)}>
            <SignalBars bars={signalBars(network.signal)} />
            <span class="ssid">{network.ssid}</span>
            {#if network.ssid === current}
              <span class="connected">{t("wifi.connected")}</span>
            {:else if network.secured}
              <span class="lock"><Icon name="lock" size={16} /></span>
            {/if}
          </button>
        </li>
      {/each}
    </ul>
  {/if}
  <button class="other" onclick={onother} disabled={scanning}>
    <Icon name="plus" size={18} />
    {t("wifi.other")}
  </button>
</section>

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-2);
    min-height: 24px;
  }
  .head h2 {
    flex: 1;
  }
  .refresh {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    border-radius: var(--r-8);
    color: var(--dim);
  }
  .refresh:disabled {
    color: var(--mute);
  }
  .state {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    min-height: 48px;
    font-size: 15px;
    color: var(--dim);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent-live);
  }
  .dot.pulse {
    animation: pulse 1.2s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  .nets {
    list-style: none;
    display: flex;
    flex-direction: column;
  }
  .nets li + li {
    border-top: 1px solid var(--hairline);
  }
  .net,
  .other {
    width: 100%;
    min-height: 52px;
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    text-align: left;
  }
  .net:active,
  .other:active {
    opacity: 0.7;
  }
  .net:disabled {
    cursor: default;
  }
  .net:disabled .ssid {
    color: var(--dim);
  }
  .net:disabled:active {
    opacity: 1;
  }
  .ssid {
    flex: 1;
    min-width: 0;
    font-size: 15px;
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .lock {
    flex: 0 0 auto;
    color: var(--dim);
  }
  .connected {
    flex: 0 0 auto;
    padding: 3px var(--sp-2);
    border-radius: 20px;
    background: var(--pill);
    color: var(--dim);
    font-size: 11px;
    font-weight: 600;
  }
  .other {
    margin-top: var(--sp-2);
    padding-top: var(--sp-3);
    border-top: 1px solid var(--hairline);
    color: var(--accent-live);
    font-size: 15px;
    font-weight: 600;
  }
</style>
