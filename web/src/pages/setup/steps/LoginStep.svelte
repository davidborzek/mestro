<script lang="ts">
  import { t } from "../../../lib/i18n.svelte";

  let {
    username = $bindable(""),
    password = $bindable(""),
    manualToken = $bindable(""),
    manualMode = $bindable(false),
    busy,
    maUrl,
    onlogin,
    onsavetoken,
    onback,
  }: {
    username?: string;
    password?: string;
    manualToken?: string;
    manualMode?: boolean;
    busy: boolean;
    maUrl: string;
    onlogin: () => void;
    onsavetoken: () => void;
    onback: () => void;
  } = $props();

  const profileUrl = $derived(`${maUrl.replace(/\/+$/, "")}/#/settings/profile`);
</script>

<section class="card">
  <h2>{t("login.title")}</h2>
  <p>{t("login.lead")}</p>
  <label class="f">
    {t("login.user")}
    <input bind:value={username} autocomplete="username" />
  </label>
  <label class="f">
    {t("login.pass")}
    <input type="password" bind:value={password} autocomplete="current-password" />
  </label>
  <a class="link-tile" href={profileUrl} target="_blank" rel="noopener">
    <span class="ic">&#9906;</span>
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
</section>

<footer class="actions">
  <button class="primary" disabled={busy || !username || !password} onclick={onlogin}>
    {t("login.submit")}
  </button>
  <button class="ghost" onclick={onback}>{t("common.back")}</button>
  <button class="ghost" onclick={() => (manualMode = !manualMode)}>
    {manualMode ? t("login.manualHide") : t("login.manualForm")}
  </button>
</footer>
