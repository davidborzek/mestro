<script lang="ts">
  import { t } from "../lib/i18n.svelte";
  import Sheet from "./Sheet.svelte";

  // "are you sure" as a sheet: one sentence, cancel + confirm
  let {
    open = $bindable(false),
    title,
    message,
    confirmLabel,
    danger = false,
    onconfirm,
  }: {
    open?: boolean;
    title: string;
    message: string;
    confirmLabel: string;
    danger?: boolean;
    onconfirm: () => void;
  } = $props();

  function confirm() {
    open = false;
    onconfirm();
  }
</script>

<Sheet bind:open {title}>
  <p class="message">{message}</p>
  <div class="buttons">
    <button class="ghost" onclick={() => (open = false)}>{t("common.cancel")}</button>
    <button class={danger ? "danger" : "primary"} onclick={confirm}>{confirmLabel}</button>
  </div>
</Sheet>

<style>
  .message {
    font-size: 15px;
    line-height: 1.5;
    color: var(--dim);
    padding-bottom: var(--sp-4);
  }
  .buttons {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--sp-3);
  }
  .danger {
    min-height: 48px;
    border-radius: var(--r-14);
    font-size: 15px;
    font-weight: 650;
    background: var(--danger);
    color: #fff;
  }
</style>
