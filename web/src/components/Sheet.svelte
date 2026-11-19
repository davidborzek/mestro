<script lang="ts">
  // bottom sheet: dim backdrop fades in, the panel slides up,
  // swipe down on the handle (or tap it) to dismiss. Focus lands
  // on the panel so a screen reader announces it; Escape closes.
  let {
    open = $bindable(false),
    title,
    actions,
    children,
  }: {
    open?: boolean;
    title: string;
    // trailing slot of the title row (count, clear button, …)
    actions?: import("svelte").Snippet;
    children?: import("svelte").Snippet;
  } = $props();

  const CLOSE_DISTANCE = 100;

  let panel: HTMLDivElement | null = $state(null);
  let dragY = $state(0);
  let dragging = $state(false);
  let lastY = 0;
  let moved = false;

  $effect(() => {
    if (!open) return;
    dragY = 0;
    const wasFocused = document.activeElement as HTMLElement | null;
    const previousOverflow = document.documentElement.style.overflow;
    panel?.focus();
    document.documentElement.style.overflow = "hidden";
    return () => {
      document.documentElement.style.overflow = previousOverflow;
      wasFocused?.focus();
    };
  });

  function close() {
    open = false;
  }

  function onkeydown(event: KeyboardEvent) {
    if (open && event.key === "Escape") close();
  }

  function startDrag(event: PointerEvent) {
    dragging = true;
    moved = false;
    lastY = event.clientY;
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }

  function moveDrag(event: PointerEvent) {
    if (!dragging) return;
    dragY = Math.max(0, dragY + event.clientY - lastY);
    if (dragY > 4) moved = true;
    lastY = event.clientY;
  }

  function endDrag() {
    if (!dragging) return;
    dragging = false;
    if (dragY > CLOSE_DISTANCE || !moved) close();
    else dragY = 0;
  }
</script>

<svelte:window {onkeydown} />

{#if open}
  <div class="dim" onclick={close} aria-hidden="true"></div>
  <div
    class="sheet"
    class:dragging
    style:transform={dragY > 0 ? `translateY(${dragY}px)` : null}
    role="dialog"
    aria-modal="true"
    aria-label={title}
    tabindex="-1"
    bind:this={panel}
  >
    <button
      class="grab"
      type="button"
      aria-label={title}
      onpointerdown={startDrag}
      onpointermove={moveDrag}
      onpointerup={endDrag}
      onpointercancel={endDrag}
    >
      <span class="handle" aria-hidden="true"></span>
    </button>
    <header class="head">
      <h2>{title}</h2>
      {@render actions?.()}
    </header>
    <div class="body">{@render children?.()}</div>
  </div>
{/if}

<style>
  .dim {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 40;
    animation: fade-in var(--t-layout) var(--ease);
  }
  .sheet {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 41;
    width: min(100%, 560px);
    margin: 0 auto;
    max-height: 85dvh;
    display: flex;
    flex-direction: column;
    background: var(--panel);
    border: 1px solid var(--hairline);
    border-bottom: 0;
    border-radius: var(--r-20) var(--r-20) 0 0;
    padding: 0 max(var(--sp-4), var(--safe-l)) var(--safe-b);
    animation: slide-up var(--t-layout) var(--ease);
    outline: none;
  }
  .sheet.dragging {
    transition: none;
  }
  .grab {
    flex: 0 0 auto;
    height: 28px;
    margin: 0 calc(-1 * var(--sp-4));
    display: flex;
    align-items: center;
    justify-content: center;
    touch-action: none;
    cursor: grab;
  }
  .handle {
    width: 36px;
    height: 4px;
    border-radius: 2px;
    background: var(--mute);
    opacity: 0.6;
  }
  .head {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    min-height: 44px;
  }
  h2 {
    flex: 1;
    font-size: 17px;
    font-weight: 700;
  }
  .body {
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding-bottom: var(--sp-4);
  }
  @keyframes fade-in {
    from {
      opacity: 0;
    }
  }
  @keyframes slide-up {
    from {
      transform: translateY(100%);
    }
  }
</style>
