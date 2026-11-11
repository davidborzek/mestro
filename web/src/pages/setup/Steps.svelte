<script lang="ts">
  import { t } from "../../lib/i18n.svelte";

  let { stepIndex }: { stepIndex: number } = $props();
</script>

<ol class="steps">
  {#each [t("steps.server"), t("steps.login"), t("steps.player")] as label, i}
    <li class:on={i === stepIndex} class:done={i < stepIndex}>
      <span class="n">{i < stepIndex ? "✓" : i + 1}</span>
      <span class="t">{label}</span>
    </li>
  {/each}
</ol>

<style>
  .steps {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 12px;
    font-size: 12.5px;
    color: var(--mute);
  }
  li {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .n {
    width: 22px;
    height: 22px;
    border-radius: 11px;
    background: var(--tile);
    border: 1px solid var(--hairline);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 11px;
    flex: 0 0 auto;
  }
  li.on {
    color: var(--text);
  }
  li.on .n {
    background: var(--accent);
    color: #14120f;
    border-color: var(--accent);
  }
  li.done .n {
    color: var(--ok);
  }

  @media (max-width: 700px) {
    .steps {
      flex-direction: row;
    }
    .t {
      display: none;
    }
  }
</style>
