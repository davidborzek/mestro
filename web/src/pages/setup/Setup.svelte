<script lang="ts">
  import { onMount } from "svelte";
  import { device, refreshDevice, socket } from "../../lib/connection.svelte";
  import type { Player, ProbeData, StateData } from "../../lib/protocol";
  import { t } from "../../lib/i18n.svelte";
  import { needsLogin, normalizeMaUrl } from "../../lib/maConnection";
  import Icon from "../../components/Icon.svelte";
  import Steps from "./Steps.svelte";
  import IntroStep from "./steps/IntroStep.svelte";
  import UrlStep from "./steps/UrlStep.svelte";
  import LoginStep from "./steps/LoginStep.svelte";
  import PlayerStep from "./steps/PlayerStep.svelte";
  import DoneStep from "./steps/DoneStep.svelte";

  type Step = "intro" | "url" | "login" | "player" | "done";

  let step = $state<Step>("intro");
  let url = $state("");
  let username = $state("");
  let password = $state("");
  let manualToken = $state("");
  let manualMode = $state(false);
  let probe = $state<ProbeData | null>(null);
  let probing = $state(false);
  let players = $state<Player[]>([]);
  let busy = $state(false);
  let error = $state("");
  let providerStatus = $state("");
  let detail = $state("");
  let lastDetail = $state("");
  let waiting = $state(false);

  const stepIndex = $derived(
    step === "intro" ? -1 : step === "url" ? 0 : step === "login" ? 1 : step === "player" ? 2 : 3,
  );
  // schema < 28 servers need no login — the step is skipped
  const needLogin = $derived(needsLogin(probe));
  const maUrl = $derived(url.trim() ? normalizeMaUrl(url) : device.configUrl);

  onMount(() => {
    // the kiosk state changes under us (clear connection, reset,
    // a second wizard) — re-read whenever the tab comes back, or
    // a kept-open tab keeps claiming "already set up"
    const onBack = () => {
      if (!document.hidden) refreshDevice().catch(() => {});
    };
    window.addEventListener("focus", onBack);
    document.addEventListener("visibilitychange", onBack);
    return () => {
      window.removeEventListener("focus", onBack);
      document.removeEventListener("visibilitychange", onBack);
    };
  });

  async function runProbe() {
    probing = true;
    error = "";
    try {
      probe = await socket.send<ProbeData>({
        op: "ma",
        action: "probe",
        url: normalizeMaUrl(url),
      });
    } catch (e) {
      probe = null;
      error = (e as Error).message;
    } finally {
      probing = false;
    }
  }

  async function checkAndNext() {
    if (!url.trim()) return;
    await runProbe();
    if (probe) {
      step = needLogin ? "login" : "player";
      if (!needLogin) await loadPlayers();
    }
  }

  async function doLogin() {
    busy = true;
    error = "";
    try {
      await socket.send({
        op: "ma",
        action: "login",
        url: normalizeMaUrl(url),
        username,
        password,
      });
      password = "";
      step = "player";
      await loadPlayers();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      busy = false;
    }
  }

  async function saveManualToken() {
    busy = true;
    error = "";
    try {
      await socket.send({
        op: "settings",
        action: "set",
        key: "music-assistant.url",
        value: normalizeMaUrl(url),
      });
      await socket.send({
        op: "settings",
        action: "set",
        key: "music-assistant.token",
        value: manualToken.trim(),
      });
      manualToken = "";
      step = "player";
      await loadPlayers();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      busy = false;
    }
  }

  async function loadPlayers() {
    busy = true;
    error = "";
    try {
      players = await socket.send<Player[]>({
        op: "ma",
        action: "players",
        url: normalizeMaUrl(url),
      });
    } catch (e) {
      error = (e as Error).message;
    } finally {
      busy = false;
    }
  }

  async function pickPlayer(name: string) {
    busy = true;
    error = "";
    try {
      // the full connection, not just the player — a no-login
      // server (schema < 28) never passes through the login write
      await socket.send({
        op: "settings",
        action: "set",
        key: "music-assistant.url",
        value: normalizeMaUrl(url),
      });
      await socket.send({
        op: "settings",
        action: "set",
        key: "music-assistant.player",
        value: name,
      });
      step = "done";
      // the app rewires its provider session live
      // (SourceCommand::Reconnect) — just watch the status flip
      await refreshStatus();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      busy = false;
    }
  }

  async function refreshStatus() {
    try {
      const state = await socket.send<StateData>({
        op: "settings",
        action: "state",
      });
      providerStatus = state.provider_status;
    } catch {
      // the app is (re)starting — keep waiting, no error copy
      providerStatus = "";
      detail = "";
      lastDetail = "";
      waiting = true;
      setTimeout(refreshStatus, 2000);
      return;
    }
    // surface a provider error only when it is stable across two
    // polls — the pre-restart status must not flash up
    detail = providerStatus && providerStatus === lastDetail ? providerStatus : "";
    lastDetail = providerStatus;
    waiting = providerStatus !== "";
    if (waiting) setTimeout(refreshStatus, 2000);
  }
</script>

<main class="wizard">
  <Steps {stepIndex} />

  <div class="content">
    {#if step === "intro"}
      <IntroStep
        configured={device.configured}
        configUrl={device.configUrl}
        configPlayer={device.configPlayer}
        onStart={() => (step = "url")}
      />
    {:else if step === "url"}
      <UrlStep
        bind:url
        bind:probe
        {probing}
        oncheck={checkAndNext}
        onback={() => (step = "intro")}
      />
    {:else if step === "login"}
      <LoginStep
        bind:username
        bind:password
        bind:manualToken
        bind:manualMode
        {busy}
        {maUrl}
        onlogin={doLogin}
        onsavetoken={saveManualToken}
        onback={() => (step = "url")}
      />
    {:else if step === "player"}
      <PlayerStep
        {players}
        {busy}
        onpick={pickPlayer}
        onback={() => (step = needLogin ? "login" : "url")}
      />
    {:else}
      <DoneStep {waiting} {detail} />
    {/if}

    {#if error}
      <div class="err" role="alert"><Icon name="alert" size={16} /> {error}</div>
    {/if}
  </div>
</main>

<style>
  .wizard {
    flex: 1;
    width: 100%;
    max-width: 480px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: var(--sp-4);
    padding: var(--sp-2) max(var(--sp-4), var(--safe-l)) calc(var(--sp-6) + var(--safe-b));
  }
  .content {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }
</style>
