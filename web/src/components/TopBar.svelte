<script lang="ts">
  // the one top bar of every screen: 56 px + safe area,
  // transparent over the backdrop, glass only once content
  // scrolls under it. A 3-column grid (1fr auto 1fr) keeps the
  // title geometrically centered; its max-width is measured so
  // it only takes the room the side slots leave — a long title
  // ellipsizes instead of overlapping the back button (iOS
  // pattern). The sides clip, never push.

  let {
    left,
    center,
    right,
  }: {
    left?: import("svelte").Snippet;
    center?: import("svelte").Snippet;
    right?: import("svelte").Snippet;
  } = $props();

  let scrolled = $state(false);
  let bar = $state<HTMLElement | null>(null);
  let leftSlot = $state<HTMLElement | null>(null);
  let rightSlot = $state<HTMLElement | null>(null);
  // null until measured — no constraint on the very first paint
  let titleMax = $state<number | null>(null);

  $effect(() => {
    const barEl = bar;
    const leftEl = leftSlot;
    const rightEl = rightSlot;
    if (!barEl || !leftEl || !rightEl) return;
    const measure = () => {
      // scrollWidth is the side's natural content width, even
      // while the 1fr track is narrower
      const taken = leftEl.scrollWidth + rightEl.scrollWidth;
      titleMax = Math.max(0, barEl.clientWidth - taken);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(barEl);
    observer.observe(leftEl);
    observer.observe(rightEl);
    return () => observer.disconnect();
  });
</script>

<svelte:window onscroll={() => (scrolled = window.scrollY > 0)} />

<header class="bar" class:scrolled bind:this={bar}>
  <div class="side left" bind:this={leftSlot}>{@render left?.()}</div>
  {#if center}
    <div class="center" style:max-width={titleMax === null ? undefined : `${titleMax}px`}>
      {@render center()}
    </div>
  {/if}
  <div class="side right" bind:this={rightSlot}>{@render right?.()}</div>
</header>

<style>
  .bar {
    position: sticky;
    top: 0;
    z-index: 20;
    flex: 0 0 auto;
    height: var(--bar-total);
    /* 24 px lines the name up with the content edge; the right
       side is 10 px tighter so the 24 px gear glyph lines up too */
    padding: var(--safe-t) max(14px, var(--safe-r)) 0 max(var(--sp-6), var(--safe-l));
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, auto) minmax(0, 1fr);
    align-items: center;
    background: transparent;
    transition: background var(--t-layout) var(--ease);
  }
  /* opaque-ish instead of backdrop-filter: iOS recomputes that
     on every repaint that scrolls under the bar */
  .bar.scrolled {
    background: color-mix(in srgb, var(--bg) 92%, transparent);
  }
  .side {
    display: flex;
    align-items: center;
    min-width: 0;
    overflow: hidden;
  }
  /* pinned to their columns: without a title the right slot would
     otherwise flow into the middle track */
  .left {
    grid-column: 1;
  }
  .right {
    grid-column: 3;
    justify-content: flex-end;
  }
  .center {
    grid-column: 2;
    min-width: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 15px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    pointer-events: none;
  }
</style>
