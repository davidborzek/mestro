<script lang="ts">
  import type { Field } from "../../../lib/protocol";
  import { fieldLabel, fieldValue, setField } from "../../../lib/settings.svelte";

  // text: the value goes out once, when the field loses focus or
  // Enter is pressed — never per keystroke (every write may restart
  // something on the device). Empty means "automatic".

  let {
    field,
    disabled = false,
  }: {
    field: Field;
    disabled?: boolean;
  } = $props();

  const stored = $derived(String(fieldValue(field.key) ?? ""));
  let draft = $state<string | null>(null);
  const shown = $derived(draft ?? stored);

  function commit() {
    if (draft === null) return;
    const next = draft.trim();
    draft = null;
    if (next !== stored) void setField(field.key, next);
  }
</script>

<input
  type="text"
  value={shown}
  {disabled}
  autocomplete="off"
  autocapitalize="off"
  spellcheck="false"
  aria-label={fieldLabel(field)}
  oninput={(event) => (draft = (event.target as HTMLInputElement).value)}
  onblur={commit}
  onkeydown={(event) => event.key === "Enter" && (event.target as HTMLInputElement).blur()}
/>
