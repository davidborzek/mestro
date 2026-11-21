<script lang="ts">
  import { onMount } from "svelte";
  import { connection, socket } from "../../lib/connection.svelte";
  import { goBack } from "../../lib/router.svelte";
  import {
    classifyJoinError,
    createJoinFlow,
    IDLE_JOIN,
    portalState,
    withTimeout,
    type JoinState,
  } from "../../lib/wifi";
  import type { WifiNetwork, WifiStatusData } from "../../lib/protocol";
  import { t } from "../../lib/i18n.svelte";
  import Icon from "../../components/Icon.svelte";
  import NetworkList from "./wifi/NetworkList.svelte";
  import JoinSheet from "./wifi/JoinSheet.svelte";

  // /setup/wifi serves two contexts: the first-boot captive portal
  // (unpaired, the player is its own hotspot — the backend grants
  // unpaired sockets wifi ops there) and the paired "change
  // network" trip from the settings Info page. Both run the same
  // list + join flow; the difference is the lead and the success
  // wording. The page is stateless on purpose — captive mini
  // browsers have no localStorage and die on a network change.
  let status = $state<WifiStatusData | null>(null);
  let statusError = $state("");
  let networks = $state<WifiNetwork[]>([]);
  let scanning = $state(false);
  let scanError = $state("");
  let sheet = $state<{ ssid: string; manual: boolean } | null>(null);
  let sheetOpen = $state(false);
  let join = $state<JoinState>({ ...IDLE_JOIN });

  // the portal context: the player is (or was just now) the setup
  // hotspot — its reply is the only fact the page trusts
  const portal = $derived(status !== null && portalState(status.state));
  const unavailable = $derived(statusError === "wifi setup not available");
  // the network the player is already on — not offered as a join
  // target in change mode (the portal has none)
  const current = $derived(status !== null && !portal ? status.ssid : "");

  // known failures get a plain user sentence; everything unknown
  // keeps the raw backend text as a small dim detail line
  const joinFailure = $derived.by(() => {
    if (join.phase !== "failed") return null;
    switch (classifyJoinError(join.error)) {
      case "wrongPassword":
        return { title: t("wifi.err.password"), detail: "" };
      case "notFound":
        return { title: t("wifi.err.notFound"), detail: "" };
      case "timeout":
        return { title: t("wifi.err.timeout"), detail: "" };
      case "busy":
        return { title: t("wifi.busy"), detail: "" };
      default:
        return {
          title: t("wifi.err.generic", { ssid: join.ssid }),
          detail: join.error,
        };
    }
  });

  const flow = createJoinFlow({
    send: (ssid, psk) => socket.send({ op: "wifi", action: "join", ssid, psk }),
    status: () => socket.send<WifiStatusData>({ op: "wifi", action: "status" }),
    portal: () => portal,
    onState: (next) => {
      join = { ...next };
      // a wrong password goes straight back into the sheet for
      // another attempt — the banner explains why behind it
      if (next.phase === "failed" && classifyJoinError(next.error) === "wrongPassword") {
        const known = networks.find((network) => network.ssid === next.ssid && network.secured);
        openSheet(next.ssid, !known);
      }
    },
  });

  $effect(() => {
    if (!sheetOpen) sheet = null;
  });

  function openSheet(ssid: string, manual: boolean): void {
    sheet = { ssid, manual };
    sheetOpen = true;
  }

  function onpick(network: WifiNetwork): void {
    if (network.secured) openSheet(network.ssid, false);
    else flow.start(network.ssid);
  }

  function onjoin(ssid: string, psk?: string): void {
    flow.start(ssid, psk);
  }

  // the portal's mini browser dies with the hotspot — a close
  // attempt is all there is to "Fertig"; the paired change returns
  // to the Info page it came from
  function finish(): void {
    if (connection.paired === true) goBack("/settings/system");
    else window.close();
  }

  async function scan(): Promise<void> {
    if (scanning) return;
    scanning = true;
    scanError = "";
    try {
      const reply = await socket.send<{ networks: WifiNetwork[] }>({
        op: "wifi",
        action: "scan",
      });
      networks = reply.networks ?? [];
    } catch (error) {
      scanError = (error as Error).message;
    } finally {
      scanning = false;
    }
  }

  async function refresh(): Promise<void> {
    try {
      // an unpaired socket outside AP mode never opens — give the
      // probe up instead of spinning forever
      status = await withTimeout(
        socket.send<WifiStatusData>({ op: "wifi", action: "status" }),
        8000,
        "status timed out",
      );
      statusError = "";
    } catch (error) {
      status = null;
      statusError = (error as Error).message;
      return;
    }
    if (join.phase === "idle") await scan();
  }

  onMount(() => {
    void refresh();
    // every (re)connect repeats the probe — a join that carried
    // the player to another network is the interesting case. The
    // initial open is already covered by the mount refresh
    // (status still null), so it is not probed twice.
    const offOpen = socket.onOpen(() => {
      // the hotspot came back = the join failed: the flow reads
      // the error from the status op
      flow.reconnected();
      if (status !== null && join.phase !== "joining") void refresh();
    });
    return () => {
      offOpen();
    };
  });
</script>

<main class="wifi">
  {#if join.phase === "joining" || join.phase === "unconfirmed"}
    <section class="card center">
      <span class="badge" aria-hidden="true"><Icon name="wifi" size={28} /></span>
      <h2>{t("wifi.joining", { ssid: join.ssid })}</h2>
      <p>
        {#if join.phase === "unconfirmed"}
          {t("wifi.joiningUnconfirmedHint")}
        {:else}
          {t("wifi.joiningHint")}
        {/if}
      </p>
    </section>
  {:else if join.phase === "success"}
    <section class="card center">
      <span class="badge ok" aria-hidden="true"><Icon name="check" size={28} /></span>
      <h2>{t("wifi.joined", { ssid: join.ssid })}</h2>
      <p>{portal ? t("wifi.joinedPortal") : t("wifi.joinedChange")}</p>
      <button class="primary" onclick={finish}>{t("wifi.done")}</button>
    </section>
  {:else if unavailable}
    <section class="card center">
      <p>{t("wifi.unavailable")}</p>
    </section>
  {:else if status === null && statusError !== ""}
    <section class="card center">
      <p>{t("wifi.unreachable")}</p>
      <button class="ghost" onclick={() => void refresh()}>{t("settings.retry")}</button>
    </section>
  {:else}
    {#if portal}
      <p class="lead">{t("wifi.lead")}</p>
    {:else if status !== null}
      <div class="callout">
        <Icon name="alert" size={16} />
        <span>{t("wifi.changeWarning")}</span>
      </div>
    {/if}
    {#if joinFailure !== null}
      <div class="banner" role="alert">
        <Icon name="alert" size={16} />
        <span>
          {joinFailure.title}
          {#if joinFailure.detail !== ""}
            <small>{joinFailure.detail}</small>
          {/if}
        </span>
      </div>
    {/if}
    {#if scanError !== ""}
      <div class="banner" role="alert">
        <Icon name="alert" size={16} />
        {t("wifi.scanFailed", { reason: scanError })}
      </div>
    {/if}
    <NetworkList
      {networks}
      {scanning}
      {current}
      {onpick}
      onother={() => openSheet("", true)}
      onrefresh={() => void scan()}
    />
  {/if}

  <JoinSheet
    bind:open={sheetOpen}
    ssid={sheet?.ssid ?? ""}
    manual={sheet?.manual ?? false}
    {onjoin}
  />
</main>

<style>
  .wifi {
    width: 100%;
    max-width: 560px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
    padding: var(--sp-2) max(24px, var(--safe-l)) calc(var(--sp-7) + var(--safe-b));
  }
  .lead {
    font-size: 15px;
    line-height: 1.5;
    color: var(--dim);
  }
  .banner {
    display: flex;
    align-items: flex-start;
    gap: var(--sp-2);
    padding: var(--sp-3) var(--sp-4);
    border: 1px solid color-mix(in srgb, var(--danger) 35%, transparent);
    border-radius: var(--r-14);
    background: color-mix(in srgb, var(--danger) 8%, transparent);
    font-size: 13px;
    line-height: 1.5;
    color: var(--danger);
  }
  .banner :global(svg) {
    flex: 0 0 auto;
    margin-top: 2px;
  }
  .banner small {
    display: block;
    font-size: 12px;
    color: var(--dim);
  }
  .callout {
    display: flex;
    align-items: flex-start;
    gap: var(--sp-2);
    padding: var(--sp-3) var(--sp-4);
    border: 1px solid color-mix(in srgb, var(--accent) 35%, transparent);
    border-radius: var(--r-14);
    background: color-mix(in srgb, var(--accent) 8%, transparent);
    font-size: 13px;
    line-height: 1.5;
    color: var(--dim);
  }
  .callout :global(svg) {
    flex: 0 0 auto;
    margin-top: 2px;
    color: var(--accent);
  }
  .card.center {
    gap: var(--sp-2);
    padding: var(--sp-6) var(--sp-4);
  }
</style>
