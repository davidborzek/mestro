<script lang="ts">
  // renders the bar count lib/wifi's signalBars derives from dBm
  let { bars }: { bars: number } = $props();

  const HEIGHTS = ["4px", "7px", "10px", "13px"];
</script>

<span class="bars" aria-hidden="true">
  {#each HEIGHTS as height, i}
    <span class="bar" class:on={i < bars} style:height></span>
  {/each}
</span>

<style>
  .bars {
    flex: 0 0 auto;
    display: flex;
    align-items: flex-end;
    gap: 2px;
    color: var(--accent-live);
  }
  .bar {
    width: 3px;
    border-radius: 1px;
    background: color-mix(in srgb, currentColor 28%, transparent);
  }
  .bar.on {
    background: currentColor;
  }
</style>
