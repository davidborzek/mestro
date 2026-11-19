<script lang="ts">
  import { t } from "../../../lib/i18n.svelte";
  import MaUrlField from "../../../components/ma/MaUrlField.svelte";
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
  <MaUrlField bind:url bind:probe {probing} {oncheck} />
</section>

<footer class="actions">
  <button class="primary" disabled={!url.trim() || probing} onclick={oncheck}>
    {probing ? t("url.checking") : t("url.next")}
  </button>
  <button class="ghost" onclick={onback}>{t("common.back")}</button>
</footer>
