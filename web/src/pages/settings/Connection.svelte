<script lang="ts">
  import { readPath } from "../../lib/settingsSchema";
  import { providerStatusText, setField, settingsStore } from "../../lib/settings.svelte";
  import { remoteState } from "../../lib/remoteState.svelte";
  import { refreshDevice, socket } from "../../lib/connection.svelte";
  import { t } from "../../lib/i18n.svelte";
  import Icon from "../../components/Icon.svelte";
  import StatusDot from "../../components/StatusDot.svelte";
  import Sheet from "../../components/Sheet.svelte";
  import ConfirmSheet from "../../components/ConfirmSheet.svelte";
  import MaUrlField from "../../components/ma/MaUrlField.svelte";
  import MaLoginForm from "../../components/ma/MaLoginForm.svelte";
  import MaPlayerList from "../../components/ma/MaPlayerList.svelte";
  import { needsLogin, normalizeMaUrl } from "../../lib/maConnection";
  import type { Player, ProbeData } from "../../lib/protocol";

  // the hand-built Music Assistant section: the wizard's flow as
  // inline edits on the live settings. The schema carries no
  // music-assistant fields — the connection is not a plain
  // settings vocabulary — and the token never crosses the wire,
  // only `token-set` does.

  const url = $derived(String(readPath(settingsStore.state?.config, "music-assistant.url") ?? ""));
  const player = $derived(
    String(readPath(settingsStore.state?.config, "music-assistant.player") ?? ""),
  );
  const tokenSet = $derived(
    readPath(settingsStore.state?.config, "music-assistant.token-set") === true,
  );
  const providerStatus = $derived(settingsStore.state?.provider_status ?? "");
  const connected = $derived(remoteState.connected);
  // the device relays the provider's English error verbatim —
  // the known lead-ins get a localized form (see maConnection)
  const providerDetail = $derived(providerStatusText(providerStatus));

  let editing = $state(false);
  let loginOpen = $state(false);
  let editUrl = $state("");
  let probe = $state<ProbeData | null>(null);
  let probing = $state(false);
  let busy = $state(false);
  let error = $state("");

  let username = $state("");
  let password = $state("");
  let manualToken = $state("");
  let manualMode = $state(false);

  // the form's target: the URL being edited, else the stored one
  const loginUrl = $derived(editing ? normalizeMaUrl(editUrl) : url);

  function reason(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
  }

  function startEdit(): void {
    editUrl = url;
    probe = null;
    error = "";
    editing = true;
    loginOpen = false;
  }

  function startLogin(): void {
    username = "";
    password = "";
    manualToken = "";
    manualMode = false;
    probe = null;
    error = "";
    loginOpen = true;
    editing = false;
  }

  function closeForms(): void {
    editing = false;
    loginOpen = false;
    probe = null;
    error = "";
    manualMode = false;
  }

  async function check(): Promise<void> {
    if (!editUrl.trim() || probing) return;
    probing = true;
    error = "";
    try {
      probe = await socket.send<ProbeData>({
        op: "ma",
        action: "probe",
        url: normalizeMaUrl(editUrl),
      });
    } catch (e) {
      probe = null;
      error = reason(e);
    } finally {
      probing = false;
    }
    if (!probe) return;
    if (needsLogin(probe)) {
      loginOpen = true;
    } else {
      // a no-login server rewires on the url write alone
      await setField("music-assistant.url", normalizeMaUrl(editUrl));
      editing = false;
    }
  }

  async function doLogin(): Promise<void> {
    busy = true;
    error = "";
    try {
      // the op stores url AND token (a failed write stops halfway)
      await socket.send({
        op: "ma",
        action: "login",
        url: loginUrl,
        username,
        password,
      });
      password = "";
      username = "";
      manualToken = "";
      manualMode = false;
      loginOpen = false;
      editing = false;
      probe = null;
    } catch (e) {
      error = reason(e);
    } finally {
      busy = false;
    }
  }

  async function saveManualToken(): Promise<void> {
    busy = true;
    error = "";
    try {
      await setField("music-assistant.url", loginUrl);
      await setField("music-assistant.token", manualToken.trim());
      manualToken = "";
      manualMode = false;
      loginOpen = false;
      editing = false;
      probe = null;
    } catch (e) {
      error = reason(e);
    } finally {
      busy = false;
    }
  }

  // ── the player picker ──

  let pickerOpen = $state(false);
  let players = $state<Player[]>([]);
  let loading = $state(false);

  // the sheet's list does not outlive an open — players change
  // server-side and the url may have changed underneath
  $effect(() => {
    if (!pickerOpen) players = [];
  });

  async function openPicker(): Promise<void> {
    pickerOpen = true;
    if (loading) return;
    loading = true;
    error = "";
    try {
      // the stored token rides along for the stored server
      players = await socket.send<Player[]>({ op: "ma", action: "players", url });
    } catch (e) {
      error = reason(e);
      pickerOpen = false;
    } finally {
      loading = false;
    }
  }

  async function pick(name: string): Promise<void> {
    pickerOpen = false;
    await setField("music-assistant.player", name);
  }

  // ── the danger zone ──

  let confirming = $state(false);

  async function disconnect(): Promise<void> {
    // "" on all three: the kiosk returns to first run and the app
    // swaps to the setup flow
    await setField("music-assistant.url", "");
    await setField("music-assistant.token", "");
    await setField("music-assistant.player", "");
    await refreshDevice();
  }
</script>

{#snippet loginBlock()}
  <div class="block">
    <MaLoginForm
      bind:username
      bind:password
      bind:manualToken
      bind:manualMode
      {busy}
      maUrl={loginUrl}
      onlogin={() => void doLogin()}
      onsavetoken={() => void saveManualToken()}
    />
    <div class="block-actions">
      <button
        class="primary"
        disabled={busy || !username || !password}
        onclick={() => void doLogin()}
      >
        {t("login.submit")}
      </button>
      <button class="ghost" onclick={closeForms}>{t("common.cancel")}</button>
    </div>
    <button class="textlink" onclick={() => (manualMode = !manualMode)}>
      {manualMode ? t("login.manualHide") : t("ma.tokenInstead")}
    </button>
  </div>
{/snippet}

<section class="card">
  <div class="row">
    <div class="texts">
      <span class="label">{t("ma.status")}</span>
      <span class="value" class:ok={connected} class:bad={!connected}>
        {connected ? t("ma.connected") : t("ma.disconnected")}
      </span>
    </div>
    <StatusDot tone={connected ? "ok" : "error"} />
  </div>
  {#if providerDetail && !connected}
    <p class="detail">{providerDetail}</p>
  {/if}

  <div class="row">
    <div class="stack">
      <span class="label">{t("ma.server")}</span>
      <span class="value nowrap">{url || "—"}</span>
    </div>
    {#if !editing}
      <button class="link" onclick={startEdit}>
        <span class="link-text">{t("ma.change")}</span>
      </button>
    {/if}
  </div>
  {#if editing}
    <div class="block">
      <MaUrlField bind:url={editUrl} bind:probe {probing} oncheck={() => void check()} />
      <div class="block-actions">
        <button class="primary" disabled={!editUrl.trim() || probing} onclick={() => void check()}>
          {probing ? t("url.checking") : t("ma.check")}
        </button>
        <button class="ghost" onclick={closeForms}>{t("common.cancel")}</button>
      </div>
      {#if loginOpen}
        {@render loginBlock()}
      {/if}
    </div>
  {/if}

  <div class="row">
    <div class="stack">
      <span class="label">{t("ma.player")}</span>
      <span class="value nowrap">{player || t("ma.auto")}</span>
    </div>
    <button
      class="link"
      disabled={!url}
      aria-label={t("ma.pick")}
      onclick={() => void openPicker()}
    >
      <Icon name="chevronDown" size={18} />
    </button>
  </div>

  <div class="row">
    <div class="stack">
      <span class="label">{t("ma.login")}</span>
      <span class="value nowrap">{tokenSet ? t("ma.signedIn") : t("ma.noToken")}</span>
    </div>
    <button class="link collapses" aria-label={t("ma.relogin")} onclick={startLogin}>
      <span class="link-text">{t("ma.relogin")}</span>
      <span class="link-icon"><Icon name="restart" size={18} /></span>
    </button>
  </div>
  {#if loginOpen && !editing}
    {@render loginBlock()}
  {/if}
</section>

{#if error}
  <div class="err" role="alert"><Icon name="alert" size={16} /> {error}</div>
{/if}

<section class="card">
  <button class="danger" onclick={() => (confirming = true)}>
    <Icon name="close" size={20} />
    {t("ma.disconnect")}
  </button>
</section>

<Sheet bind:open={pickerOpen} title={t("ma.pick")}>
  <MaPlayerList
    {players}
    busy={loading}
    autoLabel={t("ma.auto")}
    active={player}
    onpick={(name) => void pick(name)}
  />
</Sheet>

<ConfirmSheet
  bind:open={confirming}
  title={t("ma.disconnect")}
  message={t("ma.disconnectConfirm")}
  confirmLabel={t("ma.disconnect")}
  danger
  onconfirm={() => void disconnect()}
/>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-2) 0;
  }
  .row {
    border-top: 1px solid var(--hairline);
  }
  .row:first-child {
    border-top: 0;
  }
  .stack {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .texts {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--sp-3);
  }
  .label {
    font-size: 15px;
    color: var(--text);
    white-space: nowrap;
  }
  .value {
    font-size: 15px;
    font-weight: 600;
    color: var(--dim);
    text-align: right;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .value.nowrap {
    text-align: left;
  }
  .value.ok {
    color: var(--ok);
  }
  .value.bad {
    color: var(--danger);
  }
  .detail {
    font-size: 13px;
    line-height: 1.5;
    color: var(--danger);
    overflow-wrap: anywhere;
  }
  .link {
    flex: 0 0 auto;
    min-height: 32px;
    padding: 0 var(--sp-2);
    /* the padding is hit area, not layout: the glyphs end on the
       same right edge as the status dot */
    margin-right: calc(-1 * var(--sp-2));
    border-radius: var(--r-8);
    font-size: 13px;
    font-weight: 600;
    color: var(--accent-live);
  }
  .link:disabled {
    opacity: 0.45;
  }
  .link-icon {
    display: none;
  }
  /* the narrowest phones: the action shrinks to its icon so the
     value beside it stays whole */
  @media (max-width: 340px) {
    .link.collapses .link-text {
      display: none;
    }
    .link.collapses .link-icon {
      display: flex;
    }
  }
  .block {
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
    padding: var(--sp-2) 0;
    border-top: 1px solid var(--hairline);
  }
  .block-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-2);
  }
  .textlink {
    min-height: 44px;
    font-size: 14px;
    font-weight: 600;
    color: var(--accent-live);
    text-align: center;
  }
  .danger {
    width: 100%;
    min-height: 48px;
    display: flex;
    align-items: center;
    justify-content: flex-start;
    gap: var(--sp-2);
    font-size: 15px;
    font-weight: 600;
    color: var(--danger);
  }
</style>
