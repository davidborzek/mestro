<script lang="ts">
  import type { Field } from "../../../lib/protocol";
  import {
    fieldLabel,
    fieldValue,
    isOverridden,
    resetField,
    setField,
  } from "../../../lib/settings.svelte";
  import { t } from "../../../lib/i18n.svelte";
  import SwitchControl from "./SwitchControl.svelte";
  import ChoiceControl from "./ChoiceControl.svelte";
  import SliderControl from "./SliderControl.svelte";
  import ChipListControl from "./ChipListControl.svelte";
  import TriStateControl from "./TriStateControl.svelte";
  import { hasPreviews } from "../../../lib/settingsSchema";

  // one settings entry: label + help on the left, the control on
  // the right; wide controls (sliders, chip lists) move below the
  // label. Overridden fields offer the per-key reset.

  let { field, onopenskin }: { field: Field; onopenskin?: (skin: string) => void } = $props();

  const overridden = $derived(isOverridden(field.key));
  // wide controls (sliders, chip lists, segmented) wrap below the
  // label — a segmented row never squeezes the label on a phone;
  // thumbnail choices wrap too (they render as image tiles)
  const wide = $derived(
    field.kind.type === "number" ||
      field.kind.type === "percent" ||
      field.kind.type === "ordered-subset" ||
      field.kind.type === "permutation" ||
      field.kind.type === "inherit-bool" ||
      hasPreviews(field) ||
      (field.kind.type === "choice" && field.kind.options.length <= 3),
  );

  function reset() {
    // an inherit-bool flips to Inherit immediately, the other
    // kinds show their new (default) value when the reply lands
    void resetField(field.key, field.kind.type === "inherit-bool" ? null : undefined);
  }
</script>

<div class="field" class:wide>
  <div class="texts">
    <span class="label">{fieldLabel(field)}</span>
    {#if field.help}<span class="help">{field.help}</span>{/if}
  </div>
  {#if overridden}
    <button class="reset" onclick={reset}>{t("settings.reset")}</button>
  {/if}
  <div class="control">
    {#if field.kind.type === "bool"}
      <SwitchControl
        label={fieldLabel(field)}
        value={fieldValue(field.key) === true}
        onchange={(next) => void setField(field.key, next)}
      />
    {:else if field.kind.type === "choice"}
      <ChoiceControl {field} />
    {:else if field.kind.type === "number" || field.kind.type === "percent"}
      <SliderControl {field} />
    {:else if field.kind.type === "ordered-subset" || field.kind.type === "permutation"}
      <ChipListControl {field} onopen={onopenskin} />
    {:else if field.kind.type === "inherit-bool"}
      <TriStateControl {field} />
    {/if}
  </div>
</div>

<style>
  .field {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-2) 0;
  }
  .texts {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .label {
    font-size: 15px;
    color: var(--text);
  }
  /* inline rows keep one line: the label gives way instead of
     running under the control */
  .field:not(.wide) .label {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .help {
    font-size: 12px;
    color: var(--mute);
  }
  .reset {
    flex: 0 0 auto;
    min-height: 32px;
    padding: 0 var(--sp-2);
    border-radius: var(--r-8);
    font-size: 12px;
    font-weight: 600;
    color: var(--accent-live);
  }
  .control {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    text-align: right;
  }
  /* inline: the control hugs the row's right edge, the reset
     follows it */
  .field:not(.wide) .control {
    order: 2;
  }
  .field:not(.wide) .reset {
    order: 3;
  }
  /* wide controls wrap below the label row */
  .field.wide {
    flex-wrap: wrap;
  }
  .field.wide .control {
    flex: 1 1 100%;
    /* a flex item's min-width is its content: without this a wide
       control (the VU rows) pushes past the screen edge */
    min-width: 0;
    justify-content: flex-start;
  }
</style>
