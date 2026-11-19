<script lang="ts">
  import { t } from "../lib/i18n.svelte";
  import { confirmMatches } from "../lib/confirmWord";
  import Sheet from "./Sheet.svelte";

  // the factory-reset confirmation: what goes is spelled out, the
  // optional wifi wipe is a checkbox and the button only unlocks
  // once the locale's word is typed (case-insensitive)
  let {
    open = $bindable(false),
    onconfirm,
  }: {
    open?: boolean;
    onconfirm: (wifiToo: boolean) => void;
  } = $props();

  let word = $state("");
  let wifiToo = $state(false);

  $effect(() => {
    if (open) {
      word = "";
      wifiToo = false;
    }
  });

  const expected = $derived(t("reset.word"));
  const ready = $derived(confirmMatches(word, expected));

  function submit() {
    if (!ready) return;
    open = false;
    onconfirm(wifiToo);
  }
</script>

<Sheet bind:open title={t("reset.title")}>
  <ul class="lost">
    <li>{t("reset.lost.settings")}</li>
    <li>{t("reset.lost.ma")}</li>
    <li>{t("reset.lost.pairing")}</li>
  </ul>
  <label class="check">
    <input type="checkbox" bind:checked={wifiToo} />
    {t("reset.wifiToo")}
  </label>
  <label class="field">
    {t("reset.confirmHint", { word: expected })}
    <input
      class="confirm-input"
      bind:value={word}
      autocapitalize="characters"
      autocorrect="off"
      autocomplete="off"
      spellcheck="false"
    />
  </label>
  <div class="buttons">
    <button class="ghost" onclick={() => (open = false)}>{t("common.cancel")}</button>
    <button class="danger" disabled={!ready} onclick={submit}>
      {t("reset.confirm")}
    </button>
  </div>
</Sheet>

<style>
  .lost {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    padding: var(--sp-3) var(--sp-4);
    border-radius: var(--r-8);
    background: color-mix(in srgb, var(--danger) 8%, transparent);
    font-size: 14px;
    line-height: 1.5;
    color: var(--dim);
  }
  .lost li {
    display: flex;
    gap: var(--sp-2);
  }
  .lost li::before {
    content: "–";
    color: var(--danger);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    width: 100%;
    font-size: 13px;
    color: var(--dim);
  }
  .check {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    min-height: 44px;
    font-size: 15px;
    color: var(--text);
  }
  .check input {
    width: 20px;
    height: 20px;
    flex: 0 0 auto;
    accent-color: var(--accent-live);
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
  .danger:disabled {
    background: var(--pill);
    color: var(--mute);
  }
</style>
