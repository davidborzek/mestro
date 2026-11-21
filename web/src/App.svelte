<script lang="ts">
  import { onMount } from "svelte";
  import { connection, device } from "./lib/connection.svelte";
  import { boot } from "./lib/boot.svelte";
  import { getLocale, t } from "./lib/i18n.svelte";
  import { remoteState, remoteStateFeed } from "./lib/remoteState.svelte";
  import { layout, watchLayout } from "./lib/layout.svelte";
  import { sectionTitleOf, selectedSectionTitle } from "./lib/settings.svelte";
  import { goBack, navigate, route } from "./lib/router.svelte";
  import { ctl } from "./lib/commands";
  import { optimistic } from "./lib/optimistic.svelte";
  import Pair from "./pages/Pair.svelte";
  import Setup from "./pages/setup/Setup.svelte";
  import Wifi from "./pages/setup/Wifi.svelte";
  import Remote from "./pages/remote/Remote.svelte";
  import Settings from "./pages/settings/Settings.svelte";
  import TopBar from "./components/TopBar.svelte";
  import PlayerTitle from "./components/PlayerTitle.svelte";
  import IconButton from "./components/IconButton.svelte";
  import Icon from "./components/Icon.svelte";
  import Backdrop from "./components/Backdrop.svelte";
  import ReconnectBanner from "./components/ReconnectBanner.svelte";

  // boot order: hash PIN → cookie check → socket. Unpaired shows
  // the pair gate, unconfigured hands over to the first-run
  // wizard, everything else is the PWA shell. There are two
  // URL-routed destinations — the remote (/) and the device
  // settings (/settings/…) — and one top bar.
  const paired = $derived(connection.paired === true);
  const configured = $derived(paired && device.ready && device.configured);
  const inSettings = $derived(configured && route.page === "settings");
  const online = $derived(connection.status === "open");
  // /setup/wifi: the first-boot portal (unpaired, AP mode) and the
  // paired network change share the page; the branch sits above
  // the pair gate so the portal never shows it
  const inWifiSetup = $derived(route.page === "setup");

  // phone settings navigation: a picked section shows as the
  // detail (two-pane layouts always show both)
  const settingsDetail = $derived(inSettings && layout.shape !== "D" && route.section !== null);
  const settingsHeading = $derived(settingsDetail ? selectedSectionTitle() : t("settings.title"));

  // the settings back button: the skin's section, the section
  // list on the phone, else the remote — history.back() when
  // the entry below is ours, the parent route on a deep link.
  // Phones show the short label: the parent's name would crowd
  // the centered title (TopBar ellipsizes what still overlaps)
  const settingsBack = $derived.by(() => {
    if (route.skin)
      return {
        target: route.section ? `/settings/${route.section}` : "/settings",
        label: sectionTitleOf(route.section),
      };
    if (settingsDetail) return { target: "/settings", label: t("settings.title") };
    return { target: "/", label: t("nav.remote") };
  });
  const backLabel = $derived(
    layout.shape === "A"
      ? t("common.back")
      : inWifiSetup
        ? t("system.section")
        : settingsBack.label,
  );

  const playerName = $derived(remoteState.player.name || device.app);
  // the power flip shows on tap; the push (or the 3 s fallback)
  // settles it
  const kioskOn = $derived(Boolean(optimistic.pick("standby-toggle", remoteState.player.powered)));
  const togglePower = () => {
    optimistic.set("standby-toggle", !kioskOn);
    ctl("standby-toggle");
  };
  const tone = $derived<"ok" | "wait" | "error">(
    !online || !remoteStateFeed.hasSnapshot ? "wait" : remoteState.connected ? "ok" : "error",
  );

  // the cover palette drives the live accent; "" keeps amber
  $effect(() => {
    document.documentElement.style.setProperty(
      "--accent-live",
      configured ? remoteState.player.accent || "" : "",
    );
  });

  $effect(() => {
    document.documentElement.lang = getLocale();
  });

  onMount(() => {
    void boot();
    return watchLayout();
  });
</script>

<Backdrop src={configured ? remoteState.player.cover : ""} />

{#if configured}
  <ReconnectBanner />
{/if}

{#if connection.paired === null}
  <TopBar>
    {#snippet left()}<PlayerTitle name={device.app} tone="wait" />{/snippet}
  </TopBar>
{:else if inWifiSetup}
  <TopBar>
    {#snippet left()}
      {#if paired}
        <button class="back" onclick={() => goBack("/settings/system")}>
          <Icon name="chevronLeft" />
          <span class="back-label">{backLabel}</span>
        </button>
      {:else}
        <PlayerTitle name={device.app} tone="wait" />
      {/if}
    {/snippet}
    {#snippet center()}{paired ? t("wifi.change") : t("wifi.setupTitle")}{/snippet}
  </TopBar>
  <Wifi />
{:else if !paired}
  <TopBar>
    {#snippet left()}<PlayerTitle name={device.app} tone="wait" />{/snippet}
  </TopBar>
  <Pair />
{:else if device.ready && !device.configured}
  <TopBar>
    {#snippet left()}<PlayerTitle name={device.app} tone="ok" />{/snippet}
  </TopBar>
  <Setup />
{:else if inSettings}
  <TopBar>
    {#snippet left()}
      <button class="back" onclick={() => goBack(settingsBack.target)}>
        <Icon name="chevronLeft" />
        <span class="back-label">{backLabel}</span>
      </button>
    {/snippet}
    {#snippet center()}{settingsHeading}{/snippet}
  </TopBar>
  <Settings />
{:else}
  <TopBar>
    {#snippet left()}<PlayerTitle name={playerName} {tone} />{/snippet}
    {#snippet right()}
      <!-- the kiosk's power: accent = on, muted = off; the push
           brings the new state back -->
      <IconButton
        icon="power"
        label={kioskOn ? t("remote.powerOff") : t("remote.powerOn")}
        tone={kioskOn ? "accent" : "mute"}
        disabled={!online || !remoteStateFeed.hasSnapshot}
        onclick={() => togglePower()}
      />
      <IconButton
        icon="settings"
        label={t("remote.settings")}
        disabled={!device.ready}
        onclick={() => navigate("/settings")}
      />
    {/snippet}
  </TopBar>
  <Remote />
{/if}

<style>
  .back {
    display: flex;
    align-items: center;
    gap: 2px;
    min-height: 44px;
    min-width: 0;
    margin-left: -8px;
    padding: 0 var(--sp-2);
    border-radius: var(--r-8);
    font-size: 15px;
    font-weight: 500;
    color: var(--text);
  }
  .back :global(svg) {
    flex: 0 0 auto;
  }
  .back-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
