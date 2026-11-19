<script lang="ts">
  import Icon from "../../../components/Icon.svelte";
  import { t } from "../../../lib/i18n.svelte";

  let {
    waiting,
    detail,
  }: {
    waiting: boolean;
    detail: string;
  } = $props();
</script>

<section class="card center">
  <div class="badge ok"><Icon name={waiting ? "restart" : "check"} size={32} /></div>
  <h2>{t("done.title")}</h2>
  {#if waiting}
    <p>{t("done.waiting")}</p>
    {#if detail}
      <div class="note">{detail}</div>
    {/if}
    <div class="pill"><span class="dot"></span> {waiting ? t("done.pending") : t("done.ok")}</div>
  {:else}
    <p>{t("done.ready")}</p>
    <div class="pill ok"><span class="dot"></span> {t("done.ok")}</div>
  {/if}
</section>

<footer class="actions">
  {#if !waiting}
    <div class="note">{t("done.close")}</div>
  {/if}
</footer>
