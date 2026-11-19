<script lang="ts">
  import Icon, { type IconName } from "./Icon.svelte";

  // every icon-only button: the box is the hit area (≥ 44 px),
  // the glyph can be smaller. `dot` marks an "on" toggle, `badge`
  // overlays a short mark (repeat-one).
  let {
    icon,
    label,
    box = 44,
    size = 24,
    tone = "text",
    dot = false,
    badge = "",
    disabled = false,
    onclick,
  }: {
    icon: IconName;
    label: string;
    box?: number;
    size?: number;
    tone?: "text" | "mute" | "accent" | "danger";
    dot?: boolean;
    badge?: string;
    disabled?: boolean;
    onclick?: (e: MouseEvent) => void;
  } = $props();
</script>

<button
  class="icon-btn {tone}"
  style="--box: {box}px"
  {disabled}
  aria-label={label}
  title={label}
  {onclick}
>
  <Icon name={icon} {size} />
  {#if badge}<span class="badge-mark" aria-hidden="true">{badge}</span>{/if}
  {#if dot}<span class="dot" aria-hidden="true"></span>{/if}
</button>

<style>
  .icon-btn {
    position: relative;
    flex: 0 0 auto;
    width: var(--box);
    height: var(--box);
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    transition:
      transform var(--t-press) var(--ease),
      color var(--t-accent) var(--ease),
      background var(--t-press) var(--ease);
  }
  .icon-btn:active:not(:disabled) {
    transform: scale(0.94);
  }
  .icon-btn:disabled {
    opacity: 0.35;
  }
  .text {
    color: var(--text);
  }
  .mute {
    color: var(--mute);
  }
  .accent {
    color: var(--accent-live);
  }
  .danger {
    color: var(--danger);
  }
  @media (hover: hover) {
    .icon-btn:hover:not(:disabled) {
      background: var(--pill);
    }
  }
  .dot {
    position: absolute;
    left: 50%;
    bottom: 3px;
    width: 4px;
    height: 4px;
    margin-left: -2px;
    border-radius: 50%;
    background: var(--accent-live);
  }
  .badge-mark {
    position: absolute;
    right: 50%;
    top: 50%;
    margin: -4px -15px 0 0;
    font-size: 9px;
    font-weight: 800;
    line-height: 1;
    color: var(--accent-ink);
    background: var(--accent-live);
    border-radius: 6px;
    padding: 2px 4px;
  }
</style>
