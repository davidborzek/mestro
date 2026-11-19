<script lang="ts">
  import { checkPaired } from "../lib/connection.svelte";
  import { startApp } from "../lib/boot.svelte";
  import { pair, normalizePin, PIN_LENGTH, PairError } from "../lib/pairing";
  import { t } from "../lib/i18n.svelte";
  import Icon from "../components/Icon.svelte";

  // the pairing gate: no cookie, no control. The kiosk QR carries
  // the PIN in the URL hash (handled on load); everything else
  // types it in here.
  let pin = $state("");
  type PairErrorKey = "pair.wrong" | "pair.limited" | "pair.unreachable";
  let errorKey = $state<PairErrorKey | "">("");
  let busy = $state(false);

  const ready = $derived(pin.length === PIN_LENGTH);

  async function submit() {
    if (!ready || busy) return;
    busy = true;
    errorKey = "";
    try {
      await pair(pin);
      await checkPaired();
      startApp();
    } catch (e) {
      errorKey = e instanceof PairError ? (`pair.${e.kind}` as PairErrorKey) : "pair.unreachable";
    } finally {
      busy = false;
    }
  }
</script>

<main class="pair">
  <div class="card">
    <div class="badge" aria-hidden="true"><Icon name="remote" size={32} /></div>
    <h1>{t("pair.title")}</h1>
    <p>{t("pair.lead")}</p>
    <label class="f">
      {t("pair.pin")}
      <input
        class="pin-input"
        bind:value={pin}
        oninput={(e) => (pin = normalizePin(e.currentTarget.value))}
        onkeydown={(e) => e.key === "Enter" && submit()}
        inputmode="text"
        enterkeyhint="done"
        autocapitalize="characters"
        autocomplete="one-time-code"
        spellcheck="false"
        placeholder="••••••••"
        aria-invalid={errorKey === "pair.wrong" ? "true" : undefined}
      />
    </label>
    {#if errorKey}
      <div class="err" role="alert"><Icon name="alert" size={16} /> {t(errorKey)}</div>
    {/if}
    <button class="primary" disabled={!ready || busy} onclick={submit}>
      {busy ? "…" : t("pair.submit")}
    </button>
  </div>
</main>

<style>
  .pair {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--sp-4) max(var(--sp-4), var(--safe-l)) calc(var(--sp-4) + var(--safe-b));
  }
  .card {
    width: min(480px, 100%);
    padding: var(--sp-7) var(--sp-6);
    gap: var(--sp-4);
    align-items: center;
    text-align: center;
  }
  h1 {
    font-size: 20px;
    font-weight: 700;
  }
  p {
    max-width: 32ch;
  }
  .f {
    width: 100%;
    align-items: center;
  }
  .pin-input {
    height: 56px;
    text-align: center;
    font-size: 22px;
    font-weight: 650;
    letter-spacing: 8px;
    text-transform: uppercase;
  }
  button {
    width: 100%;
  }
</style>
