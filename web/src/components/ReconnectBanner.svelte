<script lang="ts">
  import { connection } from "../lib/connection.svelte";
  import { t } from "../lib/i18n.svelte";

  // socket not open: a slim accent line under the top bar right
  // away, and a toast only once the outage lasts (a quick blip
  // should not flash a banner). Controls elsewhere disable on
  // `connection.status !== "open"`.
  const TOAST_DELAY_MS = 2000;

  const down = $derived(connection.status === "reconnecting" || connection.status === "connecting");
  let toast = $state(false);

  $effect(() => {
    if (!down) {
      toast = false;
      return;
    }
    const timer = setTimeout(() => (toast = true), TOAST_DELAY_MS);
    return () => clearTimeout(timer);
  });
</script>

{#if down}
  <div class="line" aria-hidden="true"><span></span></div>
{/if}
{#if toast}
  <div class="toast" role="status">
    <span class="dot" aria-hidden="true"></span>
    {t("remote.reconnecting")}
  </div>
{/if}

<style>
  .line {
    position: fixed;
    top: var(--bar-total);
    left: 0;
    right: 0;
    height: 2px;
    z-index: 25;
    overflow: hidden;
  }
  .line span {
    position: absolute;
    inset: 0 auto 0 0;
    width: 40%;
    background: var(--accent-live);
    animation: sweep 1.4s var(--ease) infinite;
  }
  .toast {
    position: fixed;
    top: calc(var(--bar-total) + var(--sp-3));
    left: 50%;
    transform: translateX(-50%);
    z-index: 30;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-4);
    border-radius: 20px;
    background: var(--panel-hover);
    border: 1px solid var(--hairline);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
    font-size: 13px;
    font-weight: 600;
    white-space: nowrap;
    animation: drop var(--t-layout) var(--ease);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--danger);
    animation: pulse 1.2s ease-in-out infinite;
  }
  @keyframes sweep {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(250%);
    }
  }
  @keyframes drop {
    from {
      opacity: 0;
      transform: translate(-50%, -8px);
    }
  }
</style>
