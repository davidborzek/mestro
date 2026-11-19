<script lang="ts">
  import { t } from "../../lib/i18n.svelte";
  import Icon from "../../components/Icon.svelte";

  let { stepIndex }: { stepIndex: number } = $props();
</script>

<ol class="steps" aria-label="Setup">
  {#each [t("steps.server"), t("steps.login"), t("steps.player")] as label, i}
    <li class:on={i === stepIndex} class:done={i < stepIndex}>
      <span class="n">
        {#if i < stepIndex}<Icon name="check" size={14} />{:else}{i + 1}{/if}
      </span>
      <span class="t">{label}</span>
    </li>
  {/each}
</ol>

<style>
  .steps {
    list-style: none;
    display: flex;
    justify-content: space-between;
    gap: var(--sp-2);
    font-size: 12px;
    color: var(--mute);
  }
  li {
    display: flex;
    gap: var(--sp-2);
    align-items: center;
    min-width: 0;
  }
  .t {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .n {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    background: var(--pill);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 12px;
    flex: 0 0 auto;
  }
  li.on {
    color: var(--text);
  }
  li.on .n {
    background: var(--accent-live);
    color: var(--accent-ink);
  }
  li.done .n {
    color: var(--ok);
  }
</style>
