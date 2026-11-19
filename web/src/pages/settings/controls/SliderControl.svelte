<script lang="ts">
  import type { Field } from "../../../lib/protocol";
  import { fieldLabel, fieldValue, setField } from "../../../lib/settings.svelte";
  import { percentToShown, shownToPercent } from "../../../lib/settingsSchema";

  // number/percent slider: the knob follows the finger locally,
  // the value goes out once on release. `percent` fields are
  // stored 0..1 and shown 0..100 %.

  let {
    field,
    disabled = false,
  }: {
    field: Field;
    disabled?: boolean;
  } = $props();

  const kind = $derived(
    field.kind.type === "number" ? field.kind : { min: 0, max: 100, step: 1, unit: undefined },
  );
  const percent = $derived(field.kind.type === "percent");
  const min = $derived(percent ? 0 : kind.min);
  const max = $derived(percent ? 100 : kind.max);
  const step = $derived(percent ? 1 : kind.step);
  const unit = $derived(percent ? "%" : (kind.unit ?? ""));
  const stored = $derived(fieldValue(field.key));

  const settled = $derived(
    percent ? percentToShown(stored) : typeof stored === "number" ? stored : min,
  );
  const fill = $derived(((settled - min) / (max - min)) * 100);

  // local while dragging so the store only sees committed values
  let dragging = $state<number | null>(null);
  const shown = $derived(dragging ?? settled);

  function input(event: Event) {
    dragging = Number((event.target as HTMLInputElement).value);
  }

  function release(event: Event) {
    if (dragging === null) return;
    const next = Number((event.target as HTMLInputElement).value);
    dragging = null;
    void setField(field.key, percent ? shownToPercent(next) : next);
  }
</script>

<div class="slider">
  <input
    type="range"
    {min}
    {max}
    {step}
    value={shown}
    {disabled}
    aria-label={fieldLabel(field)}
    aria-valuetext="{shown}{unit ? ` ${unit}` : ''}"
    style="--fill: {fill}%"
    oninput={input}
    onchange={release}
  />
  <span class="value">{shown}{unit ? ` ${unit}` : ""}</span>
</div>

<style>
  .slider {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    width: 100%;
  }
  input {
    flex: 1 1 auto;
    min-width: 0;
    --range-bg: linear-gradient(
      to right,
      var(--accent-live) 0 var(--fill, 0%),
      var(--track) var(--fill, 0%) 100%
    );
  }
  .value {
    flex: 0 0 auto;
    min-width: 5ch;
    text-align: right;
    font-size: 13px;
    font-variant-numeric: tabular-nums;
    color: var(--dim);
  }
</style>
