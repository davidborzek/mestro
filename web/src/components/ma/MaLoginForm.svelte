<script lang="ts">
  import Icon from "../Icon.svelte";
  import { t } from "../../lib/i18n.svelte";
  import { maProfileUrl } from "../../lib/maConnection";

  // the MA login form (username/password plus the manual token
  // fallback), shared by the wizard's login step and the
  // settings connection section. The submit/cancel/manual-toggle
  // buttons stay with the call sites — their layouts differ.

  let {
    username = $bindable(""),
    password = $bindable(""),
    manualToken = $bindable(""),
    manualMode = $bindable(false),
    busy = false,
    maUrl,
    onlogin,
    onsavetoken,
  }: {
    username?: string;
    password?: string;
    manualToken?: string;
    manualMode?: boolean;
    busy?: boolean;
    maUrl: string;
    onlogin: () => void;
    onsavetoken: () => void;
  } = $props();

  const profileUrl = $derived(maProfileUrl(maUrl));
</script>

<label class="f">
  {t("login.user")}
  <input bind:value={username} autocomplete="username" />
</label>
<label class="f">
  {t("login.pass")}
  <input type="password" bind:value={password} autocomplete="current-password" />
</label>
<a class="link-tile" href={profileUrl} target="_blank" rel="noopener">
  <Icon name="external" size={20} />
  <span>
    <b>{t("login.manualLink")}</b>
    <small>{t("login.manualHint")}</small>
  </span>
</a>
{#if manualMode}
  <label class="f">
    {t("login.token")}
    <input bind:value={manualToken} placeholder={t("login.tokenPlaceholder")} />
  </label>
  <button class="primary" disabled={busy || !manualToken.trim()} onclick={onsavetoken}>
    {t("login.saveToken")}
  </button>
{/if}
