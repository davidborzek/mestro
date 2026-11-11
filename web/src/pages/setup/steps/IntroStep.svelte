<script lang="ts">
  import { t } from "../../../lib/i18n.svelte";

  let {
    configured,
    configUrl,
    configPlayer,
    onStart,
  }: {
    configured: boolean;
    configUrl: string;
    configPlayer: string;
    onStart: () => void;
  } = $props();
</script>

<section class="card center">
  <div class="check">&#9835;</div>
  <h2>{t("intro.title")}</h2>
  {#if configured}
    <p>{t("intro.configured")}</p>
    <div class="pill">
      <span class="dot"></span>
      {configUrl}{configPlayer ? ` · ${configPlayer}` : ""}
    </div>
  {:else}
    <p>{t("intro.fresh")}</p>
  {/if}
</section>

<footer class="actions">
  <button class="primary" onclick={onStart}>
    {configured ? t("intro.restart") : t("intro.start")}
  </button>
</footer>
