<script lang="ts">
  import type { Field, Option } from "../../../lib/protocol";
  import { fieldLabel, fieldValue, setField, valueTexts } from "../../../lib/settings.svelte";
  import { formatValue, hasPreviews } from "../../../lib/settingsSchema";
  import Segmented from "../../../components/Segmented.svelte";
  import Sheet from "../../../components/Sheet.svelte";
  import Icon from "../../../components/Icon.svelte";

  // choice: up to three options fit the segmented control, the
  // rest open a bottom sheet with the option list. Options with
  // thumbnails (clock styles) render as image tiles instead.

  let {
    field,
    disabled = false,
  }: {
    field: Field;
    disabled?: boolean;
  } = $props();

  const kind = $derived(field.kind.type === "choice" ? field.kind : { options: [] });
  const options = $derived(kind.options);
  const current = $derived(fieldValue(field.key));
  const text = $derived(formatValue(field, current, valueTexts()));
  const tiles = $derived(hasPreviews(field));
  const segmented = $derived(!tiles && options.length <= 3);

  let open = $state(false);

  function pick(value: string) {
    open = false;
    void setField(field.key, value);
  }

  function label(option: Option): string {
    return valueTexts().optionLabel(option);
  }
</script>

{#if tiles}
  <div class="tiles" role="listbox" aria-label={fieldLabel(field)}>
    {#each options as option (option.value)}
      <button
        class="tile"
        class:active={option.value === current}
        {disabled}
        role="option"
        aria-selected={option.value === current}
        onclick={() => pick(option.value)}
      >
        <span class="thumb">
          {#if option.preview}
            <img src={option.preview} alt={label(option)} loading="lazy" />
          {/if}
          {#if option.value === current}
            <span class="check"><Icon name="check" size={16} /></span>
          {/if}
        </span>
        <span class="name">{label(option)}</span>
      </button>
    {/each}
  </div>
{:else if segmented}
  <Segmented
    label={fieldLabel(field)}
    items={options.map((option) => ({
      value: option.value,
      label: valueTexts().optionLabel(option),
    }))}
    active={[String(current ?? "")]}
    {disabled}
    onselect={(value) => void setField(field.key, value)}
  />
{:else}
  <button class="select" {disabled} onclick={() => (open = true)}>
    <span class="value">{text}</span>
    <Icon name="chevronDown" size={18} />
  </button>
  <Sheet bind:open title={fieldLabel(field)}>
    <div class="options">
      {#each options as option (option.value)}
        <button
          class="option"
          class:active={option.value === current}
          onclick={() => pick(option.value)}
        >
          <span class="option-label">{valueTexts().optionLabel(option)}</span>
          {#if option.value === current}
            <Icon name="check" size={20} />
          {/if}
        </button>
      {/each}
    </div>
  </Sheet>
{/if}

<style>
  .select {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    min-height: 36px;
    padding: 0 var(--sp-2);
    border-radius: var(--r-8);
    font-size: 14px;
    font-weight: 600;
    color: var(--text);
  }
  .select .value {
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .select :global(svg) {
    color: var(--mute);
  }
  .options {
    display: flex;
    flex-direction: column;
  }
  .option {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
    min-height: 48px;
    padding: 0 var(--sp-2);
    border-radius: var(--r-8);
    font-size: 15px;
    color: var(--text);
  }
  .option.active {
    color: var(--accent-live);
    font-weight: 650;
  }
  .option:active {
    background: var(--pill);
  }

  /* ── thumbnail tiles (clock styles): 5:3 boxes, lazily filled ── */
  .tiles {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: var(--sp-2);
    width: 100%;
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    padding: 0;
    background: none;
    text-align: center;
  }
  /* the fixed aspect box reserves the tile's space before the
     image arrives — no layout shift while the previews load */
  .thumb {
    position: relative;
    display: block;
    width: 100%;
    aspect-ratio: 5 / 3;
    border-radius: var(--r-8);
    border: 1px solid var(--hairline);
    background: var(--pill);
    overflow: hidden;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .tile.active .thumb {
    border-color: var(--accent-live);
    box-shadow: inset 0 0 0 1px var(--accent-live);
  }
  .check {
    position: absolute;
    right: 6px;
    bottom: 6px;
    width: 22px;
    height: 22px;
    border-radius: 11px;
    background: var(--accent-live);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--panel);
  }
  .name {
    font-size: 13px;
    color: var(--dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tile.active .name {
    color: var(--accent-live);
    font-weight: 650;
  }
  .tile:disabled {
    opacity: 0.4;
  }
</style>
