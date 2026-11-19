<script lang="ts">
  import type { Field, Option } from "../../../lib/protocol";
  import {
    fieldValue,
    setField,
    settingsStore,
    skinOf,
    valueTexts,
    visibleField,
  } from "../../../lib/settings.svelte";
  import { hasPreviews } from "../../../lib/settingsSchema";
  import { dropIndex, moveItem, shiftOffset } from "../../../lib/reorder";
  import { t } from "../../../lib/i18n.svelte";
  import Icon from "../../../components/Icon.svelte";
  import IconButton from "../../../components/IconButton.svelte";

  // ordered list editor as one reorderable row per entry: drag
  // handle, (thumbnail), name, cogwheel slot, remove. The drag
  // follows the pointer, the crossed rows slide aside, the drop
  // commits one write. ArrowUp/Down on the focused handle move
  // the row for keyboard users (announced via aria-live).

  let {
    field,
    disabled = false,
    onopen,
  }: {
    field: Field;
    disabled?: boolean;
    onopen?: (skin: string) => void;
  } = $props();

  const kind = $derived(
    field.kind.type === "ordered-subset" || field.kind.type === "permutation"
      ? field.kind
      : { type: "ordered-subset" as const, options: [] },
  );
  const removable = $derived(kind.type === "ordered-subset");

  const value = $derived(fieldValue(field.key));
  const order = $derived(Array.isArray(value) ? (value as string[]) : []);
  const inactive = $derived(
    removable ? kind.options.filter((option) => !order.includes(option.value)) : [],
  );

  const tiles = $derived(hasPreviews(field));
  const preview = (entry: string): string | undefined =>
    kind.options.find((option) => option.value === entry)?.preview;

  const label = (value: string): string =>
    valueTexts().optionLabel(
      kind.options.find((option) => option.value === value) ?? { value, label: value },
    );

  // the skins whose own options exist (and are visible) in the
  // schema: their rows get the cogwheel, the others the empty
  // slot that keeps the columns aligned
  const withOptions = $derived(
    new Set(
      (settingsStore.schema?.sections.find((section) => section.id === "vu")?.fields ?? [])
        .filter((candidate) => visibleField(candidate))
        .map((candidate) => skinOf(candidate.key))
        .filter((skin): skin is string => skin !== null),
    ),
  );

  // the live drag: which row, how far the pointer moved, the
  // measured row height and the currently crossed slot
  let drag = $state<{ from: number; dy: number; height: number; to: number } | null>(null);
  let dragStartY = 0;
  let announcement = $state("");

  function grab(event: PointerEvent, index: number) {
    if (disabled || drag || (event.pointerType === "mouse" && event.button !== 0)) return;
    const handle = event.currentTarget as HTMLElement;
    const row = handle.closest(".item") as HTMLElement | null;
    dragStartY = event.clientY;
    drag = { from: index, dy: 0, height: row?.offsetHeight ?? 0, to: index };
    // the capture routes every later move/release to the handle
    handle.setPointerCapture(event.pointerId);
    handle.focus();
    event.preventDefault();
  }

  function dragMove(event: PointerEvent) {
    if (!drag) return;
    drag.dy = event.clientY - dragStartY;
    drag.to = dropIndex(drag.from, drag.dy, drag.height, order.length);
  }

  function dragEnd() {
    if (!drag) return;
    const { from, to } = drag;
    drag = null;
    commit(from, to);
  }

  function dragCancel() {
    drag = null;
  }

  function handleKeydown(event: KeyboardEvent, index: number) {
    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    commit(index, event.key === "ArrowUp" ? index - 1 : index + 1);
    // the row's DOM node moves — Chrome can drop focus on it
    requestAnimationFrame(() => handle.focus());
  }

  function commit(from: number, to: number) {
    if (to < 0 || to >= order.length) return;
    const next = moveItem(order, from, to);
    if (next === order) return; // no-op drop, no write
    void setField(field.key, next);
    announcement = t("settings.reorderPosition", {
      position: String(to + 1),
      count: String(order.length),
    });
  }

  // the dragged row follows the pointer; the rows it crossed
  // slide one slot in the other direction
  function offset(index: number): string | undefined {
    if (!drag) return undefined;
    const dy = drag.from === index ? drag.dy : shiftOffset(index, drag.from, drag.to, drag.height);
    return dy ? `translateY(${dy}px)` : undefined;
  }

  function append(option: Option) {
    void setField(field.key, [...order, option.value]);
  }

  function drop(value: string) {
    void setField(
      field.key,
      order.filter((entry) => entry !== value),
    );
  }
</script>

<div class="rows" role="list">
  {#each order as entry, index (entry)}
    <div
      class="item"
      class:tiles
      class:dragging={drag?.from === index}
      role="listitem"
      style:transform={offset(index)}
    >
      <button
        class="handle"
        aria-label={t("settings.dragReorder")}
        title={t("settings.dragReorder")}
        {disabled}
        onpointerdown={(event) => grab(event, index)}
        onpointermove={dragMove}
        onpointerup={dragEnd}
        onpointercancel={dragCancel}
        onkeydown={(event) => handleKeydown(event, index)}
      >
        <Icon name="drag" size={18} />
      </button>
      {#if tiles}
        <span class="thumb">
          {#if preview(entry)}
            <img src={preview(entry)} alt="" loading="lazy" />
          {/if}
        </span>
      {/if}
      <!-- the cogwheel rides right after the name: skins without
           options simply have none, no empty column -->
      <span class="name-cell">
        <span class="name">{label(entry)}</span>
        {#if tiles && onopen && withOptions.has(entry)}
          <IconButton
            icon="settings"
            label={t("settings.skinOptions", { skin: label(entry) })}
            box={32}
            size={17}
            tone="mute"
            {disabled}
            onclick={() => onopen(entry)}
          />
        {/if}
      </span>
      {#if removable}
        <IconButton
          icon="close"
          label={t("settings.remove")}
          box={36}
          size={18}
          tone="mute"
          {disabled}
          onclick={() => drop(entry)}
        />
      {:else}
        <span class="slot" aria-hidden="true"></span>
      {/if}
    </div>
  {/each}

  {#if inactive.length}
    <div class="pool">
      <span class="hint">{t("settings.available")}</span>
      {#each inactive as option (option.value)}
        <button class="item pool" class:tiles {disabled} onclick={() => append(option)}>
          <span class="slot" aria-hidden="true"></span>
          {#if tiles}
            <span class="thumb">
              {#if option.preview}
                <img src={option.preview} alt="" loading="lazy" />
              {/if}
            </span>
          {/if}
          <span class="name-cell">
            <span class="name">{valueTexts().optionLabel(option)}</span>
          </span>
          <span class="plus"><Icon name="plus" size={18} /></span>
        </button>
      {/each}
    </div>
  {/if}
</div>

<p class="sr-only" aria-live="polite">{announcement}</p>

<style>
  .rows {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    width: 100%;
    min-width: 0;
  }

  /* one line, fixed columns: [handle 36] [thumb 64] [name ⚙] [remove 36] */
  .item {
    position: relative;
    display: grid;
    grid-template-columns: 36px minmax(0, 1fr) 36px;
    align-items: center;
    column-gap: var(--sp-2);
    min-height: 48px;
    padding: 0 var(--sp-2);
    border-radius: var(--r-14);
    background: var(--glass);
    border: 1px solid var(--hairline);
    /* the crossed rows glide into place; the dragged one follows
         the pointer directly (see .dragging). Under
         prefers-reduced-motion the theme zeroes every transition */
    transition: transform 150ms var(--ease);
  }
  .item.tiles {
    grid-template-columns: 36px 64px minmax(0, 1fr) 36px;
    min-height: 56px;
  }
  /* narrow phones: the name gets the room — smaller thumbnail,
     tighter gaps (the buttons keep their 36 px hit area) */
  @media (max-width: 400px) {
    .item {
      column-gap: var(--sp-1);
      padding: 0 var(--sp-1) 0 var(--sp-2);
    }
    .item.tiles {
      grid-template-columns: 32px 52px minmax(0, 1fr) 36px;
    }
    .item.tiles .thumb {
      width: 52px;
    }
    .item.tiles .name {
      padding-left: var(--sp-1);
    }
  }
  .item.dragging {
    transition: none;
    z-index: 1;
    border-color: transparent;
    background: var(--panel);
    box-shadow: 0 10px 30px rgb(0 0 0 / 40%);
  }

  .handle {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    margin-left: -8px;
    color: var(--mute);
    /* the drag owns the gesture — no scroll, no text selection */
    touch-action: none;
    user-select: none;
    -webkit-user-select: none;
    cursor: grab;
  }
  .handle:active {
    cursor: grabbing;
  }
  .handle:disabled {
    cursor: default;
    opacity: 0.35;
  }
  @media (hover: hover) {
    .handle:hover:not(:disabled) {
      background: var(--pill);
      border-radius: 50%;
    }
  }

  /* the empty stand-ins keep every column the same width */
  .slot {
    width: 36px;
    height: 36px;
  }

  .thumb {
    width: 64px;
    aspect-ratio: 16 / 10;
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

  /* the name shrinks (ellipsis) before the cogwheel does */
  .name-cell {
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .name-cell :global(.icon-btn) {
    flex: 0 0 auto;
  }
  .name {
    min-width: 0;
    flex: 0 1 auto;
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* without thumbnails the name keeps its chip look */
  .item:not(.tiles) .name {
    flex: 1 1 auto;
    display: flex;
    align-items: center;
    min-height: 36px;
    padding: 0 var(--sp-3);
    border-radius: 18px;
    background: var(--pill);
    font-size: 14px;
  }

  /* ── the available pool: same grid, dashed, plus to append ── */
  .pool {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    padding-top: var(--sp-2);
  }
  .hint {
    font-size: 11px;
    letter-spacing: 0.8px;
    text-transform: uppercase;
    color: var(--mute);
  }
  .item.pool {
    width: 100%;
    background: none;
    border-style: dashed;
    text-align: left;
  }
  .item.pool .name {
    color: var(--dim);
    font-weight: 500;
  }
  .item.pool:disabled {
    opacity: 0.4;
  }
  .plus {
    display: flex;
    justify-content: center;
    color: var(--mute);
  }
</style>
