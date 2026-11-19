<script lang="ts">
  // the bool toggle: pill track, the knob slides, accent when on

  let {
    label,
    value,
    disabled = false,
    onchange,
  }: {
    label: string;
    value: boolean;
    disabled?: boolean;
    onchange: (value: boolean) => void;
  } = $props();
</script>

<button
  class="switch"
  class:on={value}
  role="switch"
  aria-checked={value}
  aria-label={label}
  {disabled}
  onclick={() => onchange(!value)}
>
  <span class="knob" aria-hidden="true"></span>
</button>

<style>
  .switch {
    flex: 0 0 auto;
    width: 52px;
    height: 32px;
    border-radius: 16px;
    background: var(--track);
    position: relative;
    transition: background var(--t-press) var(--ease);
  }
  .knob {
    position: absolute;
    left: 3px;
    top: 3px;
    width: 26px;
    height: 26px;
    border-radius: 50%;
    background: var(--mute);
    transition:
      transform var(--t-press) var(--ease),
      background var(--t-press) var(--ease);
  }
  .switch.on {
    background: var(--accent-live);
  }
  .switch.on .knob {
    transform: translateX(20px);
    background: var(--accent-ink);
  }
  .switch:active:not(:disabled) .knob {
    transform: scale(0.92);
  }
  .switch.on:active:not(:disabled) .knob {
    transform: translateX(20px) scale(0.92);
  }
  .switch:disabled {
    opacity: 0.4;
  }
</style>
