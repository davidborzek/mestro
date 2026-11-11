<script lang="ts">
  import { t } from "../../../lib/i18n.svelte";
  import type { ProbeData } from "../../../lib/protocol";

  let {
    url = $bindable(""),
    probe = $bindable<ProbeData | null>(null),
    probing,
    oncheck,
    onback,
  }: {
    url?: string;
    probe?: ProbeData | null;
    probing: boolean;
    oncheck: () => void;
    onback: () => void;
  } = $props();
</script>

<section class="card">
  <h2>{t("url.title")}</h2>
  <p>{t("url.lead")}</p>
  <label class="f">
    {t("url.label")}
    <input
      placeholder="http://ma.local:8095"
      bind:value={url}
      oninput={() => (probe = null)}
      onkeydown={(e) => e.key === "Enter" && oncheck()}
    />
  </label>
  {#if probing}
    <div class="pill"><span class="dot"></span> {t("url.probing")}</div>
  {:else if probe}
    <div class="pill ok">
      <span class="dot"></span>
      {t("url.ok", { version: probe.version, schema: String(probe.schema) })}
    </div>
  {/if}
</section>

<footer class="actions">
  <button class="primary" disabled={!url.trim() || probing} onclick={oncheck}>
    {probing ? t("url.checking") : t("url.next")}
  </button>
  <button class="ghost" onclick={onback}>{t("common.back")}</button>
</footer>
