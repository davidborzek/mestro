<script lang="ts">
  import { onMount } from "svelte";
  import { connect, device } from "./lib/connection.svelte";
  import { getLocale, toggleLocale, t } from "./lib/i18n.svelte";
  import Setup from "./pages/setup/Setup.svelte";
  import Update from "./pages/update/Update.svelte";

  // the web app is the device setup — a dedicated wizard frame,
  // nothing else lives here (settings stay on the kiosk)
  let error = $state("");
  let view = $state<"setup" | "update">("setup");

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
  {#if device.ota}
    <nav class="nav">
      <button class:active={view === "setup"} onclick={() => (view = "setup")}>
        {t("nav.setup")}
      </button>
      <button class:active={view === "update"} onclick={() => (view = "update")}>
        {t("nav.update")}
      </button>
    </nav>
  {/if}
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
      {#if view === "update" && device.ota}
        <Update />
      {:else}
        <Setup />
      {/if}
    </div>
  {/if}
</main>
