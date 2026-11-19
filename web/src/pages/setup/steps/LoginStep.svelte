<script lang="ts">
  import { t } from "../../../lib/i18n.svelte";
  import MaLoginForm from "../../../components/ma/MaLoginForm.svelte";

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
</script>

<section class="card">
  <h2>{t("login.title")}</h2>
  <p>{t("login.lead")}</p>
  <MaLoginForm
    bind:username
    bind:password
    bind:manualToken
    bind:manualMode
    {busy}
    {maUrl}
    {onlogin}
    {onsavetoken}
  />
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
