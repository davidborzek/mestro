<script lang="ts" generics="V extends string">
  import Icon, { type IconName } from "./Icon.svelte";

  // pill segmented control: 36 px tall, the active segment is a
  // lighter pill; the hit area reaches 44 px (see ::before). Icon
  // segments are square, text segments size to their label.
  type Item = { value: V; label: string; icon?: IconName };

  let {
    items,
    active,
    label,
    disabled = false,
    onselect,
  }: {
    items: Item[];
    // the highlighted values: one for a radio, several for the
    // kiosk views (a toggle can be on without being "current")
    active: V[];
    label: string;
    disabled?: boolean;
    onselect: (value: V) => void;
  } = $props();
</script>

<div class="seg" role="group" aria-label={label}>
  {#each items as item (item.value)}
    <button
      class="segment"
      class:on={active.includes(item.value)}
      class:icon={!!item.icon}
      aria-label={item.label}
      aria-pressed={active.includes(item.value)}
      title={item.label}
      {disabled}
      onclick={() => onselect(item.value)}
    >
      {#if item.icon}<Icon name={item.icon} size={20} />{:else}{item.label}{/if}
    </button>
  {/each}
</div>

<style>
  .seg {
    display: inline-flex;
    height: 36px;
    padding: 2px;
    gap: 2px;
    border-radius: 18px;
    background: var(--pill);
  }
  .segment {
    position: relative;
    height: 32px;
    min-width: 40px;
    padding: 0 var(--sp-4);
    border-radius: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 14px;
    font-weight: 600;
    color: var(--mute);
    transition:
      background var(--t-press) var(--ease),
      color var(--t-press) var(--ease),
      transform var(--t-press) var(--ease);
  }
  .segment.icon {
    padding: 0;
    width: 44px;
  }
  .segment::before {
    content: "";
    position: absolute;
    inset: -6px 0;
  }
  .segment.on {
    background: var(--pill-active);
    color: var(--text);
  }
  .segment:active:not(:disabled) {
    transform: scale(0.94);
  }
  .segment:disabled {
    opacity: 0.4;
  }
</style>
