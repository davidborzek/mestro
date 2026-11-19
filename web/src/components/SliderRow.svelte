<script lang="ts">
  import { t } from "../lib/i18n.svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  // optimistic slider row: the knob follows the finger immediately,
  // sends are throttled (trailing) and the final value goes out on
  // release. The optimistic value clears on the server echo, and
  // on any kiosk-side change after a grace period.
  let {
    icon,
    label,
    value,
    min = 0,
    disabled,
    throttleMs = 150,
    oncommit,
  }: {
    // a function reacts to the shown value (volume goes mute at 0)
    icon: IconName | ((value: number) => IconName);
    label: string;
    value: number;
    min?: number;
    disabled: boolean;
    throttleMs?: number;
    oncommit: (value: number) => void;
  } = $props();

  const ECHO_TIMEOUT_MS = 3000;

  let optimistic = $state<number | null>(null);
  let lastSent = $state(-1);
  let timer: ReturnType<typeof setTimeout> | null = null;

  const shown = $derived(optimistic ?? value);
  const fill = $derived(Math.max(0, Math.min(100, shown)));
  const glyph = $derived(typeof icon === "function" ? icon(fill) : icon);

  // the echo of our own command confirms the optimistic value;
  // anything else (kiosk buttons) overrides after the grace period
  $effect(() => {
    if (value === lastSent) {
      optimistic = null;
      return;
    }
    if (optimistic !== null) {
      const fallback = setTimeout(() => (optimistic = null), ECHO_TIMEOUT_MS);
      return () => clearTimeout(fallback);
    }
  });

  function send(next: number) {
    lastSent = next;
    oncommit(next);
  }

  function input(event: Event) {
    const next = Number((event.target as HTMLInputElement).value);
    optimistic = next;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      send(next);
    }, throttleMs);
  }

  function release(event: Event) {
    if (optimistic === null) return;
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
    send(Number((event.target as HTMLInputElement).value));
  }
</script>

<div class="slider-row">
  <span class="lead" aria-hidden="true"><Icon name={glyph} size={20} /></span>
  <input
    type="range"
    {min}
    max="100"
    step="1"
    value={shown}
    {disabled}
    aria-label={label}
    aria-valuetext="{fill} %"
    style="--fill: {fill}%"
    oninput={input}
    onchange={release}
  />
  <span class="value">{fill}</span>
</div>

<style>
  .slider-row {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    height: 44px;
  }
  .lead {
    display: flex;
    flex: 0 0 auto;
    width: 20px;
    color: var(--dim);
  }
  .value {
    flex: 0 0 auto;
    min-width: 3ch;
    text-align: right;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    color: var(--mute);
  }
  input {
    --range-bg: linear-gradient(
      to right,
      var(--accent-live) 0 var(--fill, 0%),
      var(--track) var(--fill, 0%) 100%
    );
  }
</style>
