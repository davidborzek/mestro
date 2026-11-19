<script lang="ts">
  import { t } from "../../lib/i18n.svelte";
  import type { ProbeData } from "../../lib/protocol";

  // the server URL field with its probe verdict, shared by the
  // wizard's url step and the settings connection section

  let {
    url = $bindable(""),
    probe = $bindable<ProbeData | null>(null),
    probing = false,
    oncheck,
  }: {
    url?: string;
    probe?: ProbeData | null;
    probing?: boolean;
    oncheck: () => void;
  } = $props();
</script>

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
