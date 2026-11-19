<script lang="ts">
  import type { Field } from "../../../lib/protocol";
  import {
    fieldLabel,
    fieldValue,
    isOverridden,
    resetField,
    setField,
  } from "../../../lib/settings.svelte";
  import { basePinned, triStateValue } from "../../../lib/settingsSchema";
  import { t } from "../../../lib/i18n.svelte";
  import Segmented from "../../../components/Segmented.svelte";

  // inherit-bool: unset (null) follows the base value, On/Off pin
  // it. The state reads honestly from the effective config — a
  // key the config file pins shows as On/Off with a hint, never a
  // pretend Inherit that snaps back on the next reply.

  let {
    field,
    disabled = false,
  }: {
    field: Field;
    disabled?: boolean;
  } = $props();

  const value = $derived(fieldValue(field.key));
  const overridden = $derived(isOverridden(field.key));
  const pinned = $derived(basePinned(value, overridden));
  const current = $derived(triStateValue(value));

  function select(next: string) {
    if (next === "inherit") {
      // a pinned key cannot reach Inherit — the reset changes
      // nothing, so the control stays on the base value
      void resetField(field.key, pinned ? undefined : null);
    } else {
      void setField(field.key, next === "on");
    }
  }
</script>

<div class="tri">
  <Segmented
    label={fieldLabel(field)}
    items={[
      { value: "inherit", label: t("settings.inherit") },
      { value: "on", label: t("settings.on") },
      { value: "off", label: t("settings.off") },
    ]}
    active={[current]}
    {disabled}
    onselect={select}
  />
  {#if pinned}
    <p class="hint">{t("settings.configPinned")}</p>
  {/if}
</div>

<style>
  .tri {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--sp-1);
    width: 100%;
  }
  .hint {
    font-size: 12px;
    color: var(--mute);
  }
</style>
