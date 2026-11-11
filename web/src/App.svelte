<script lang="ts">
  import { onMount } from "svelte";
  import { connect, device } from "./lib/connection.svelte";
  import { getLocale, toggleLocale, t } from "./lib/i18n.svelte";
  import Setup from "./pages/setup/Setup.svelte";

  // the web app is the device setup — a dedicated wizard frame,
  // nothing else lives here (settings stay on the kiosk)
  let error = $state("");

  onMount(() => {
    connect().catch((e: Error) => (error = `${t("common.errorConnect")} (${e.message})`));
  });
</script>

<header class="topbar">
  <div class="brand">
    <div class="logo">&#9835;</div>
    <div>
      <div class="name">{device.app}</div>
      <div class="sub">{t("brand.sub")}</div>
    </div>
  </div>
  <div class="spacer"></div>
  <button class="lang" onclick={toggleLocale}>
    {getLocale().toUpperCase()}
  </button>
</header>

{#if error}
  <div class="err">⚠ {error}</div>
{/if}

<main class="page">
  {#if !device.ready}
    <div class="loading">
      <span class="pill"><span class="dot pulse"></span> {t("app.loading")}</span>
    </div>
  {:else}
    <div class="setup-frame">
      <Setup />
    </div>
  {/if}
</main>
