<script lang="ts">
  import { fade } from "svelte/transition";

  // the backdrop behind everything: the server's pre-blurred,
  // pre-dimmed cover (GET /cover?blur=1) painted as a plain img.
  // No CSS filter — iOS recomputes filter: blur() on every
  // repaint above it, which made the whole app feel slow. A new
  // cover crossfades (opacity only, on the img's own compositing
  // layer); no cover, no backdrop. The bottom 40 % fade into --bg
  // so text there stays readable.
  let { src }: { src: string } = $props();

  const backdropSrc = $derived(src === "" ? "" : `${src}&blur=1`);
</script>

{#if backdropSrc}
  <div class="backdrop" aria-hidden="true">
    {#key backdropSrc}
      <img src={backdropSrc} alt="" in:fade={{ duration: 600 }} out:fade={{ duration: 600 }} />
    {/key}
    <div class="fade"></div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: -1;
    overflow: hidden;
    pointer-events: none;
    /* its own world: nothing inside affects or is affected by
       the repaints above it */
    contain: strict;
  }
  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    /* the crossfade is opacity-only and composites alone */
    will-change: opacity;
  }
  .fade {
    position: absolute;
    inset: 0;
    background: linear-gradient(to bottom, transparent 60%, var(--bg) 100%);
  }
</style>
