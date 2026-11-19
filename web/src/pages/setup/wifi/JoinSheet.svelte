<script lang="ts">
  import { t } from "../../../lib/i18n.svelte";
  import Sheet from "../../../components/Sheet.svelte";

  // the join dialog: a picked secured network (password required)
  // or a manual SSID for hidden networks (password optional — it
  // may be open). Open networks never open it.
  let {
    open = $bindable(false),
    ssid = "",
    manual = false,
    onjoin,
  }: {
    open?: boolean;
    // the picked network's SSID — empty in manual mode
    ssid?: string;
    manual?: boolean;
    onjoin: (ssid: string, psk?: string) => void;
  } = $props();

  let manualSsid = $state("");
  let psk = $state("");
  let show = $state(false);

  $effect(() => {
    if (open) {
      manualSsid = ssid;
      psk = "";
      show = false;
    }
  });

  const title = $derived(manual ? t("wifi.other") : ssid);
  const target = $derived(manual ? manualSsid.trim() : ssid);
  // a picked network is secured by definition — its password is
  // the point of the sheet
  const needsPsk = $derived(!manual);
  const ready = $derived(target !== "" && (!needsPsk || psk !== ""));

  function submit() {
    if (!ready) return;
    open = false;
    onjoin(target, needsPsk ? psk : psk || undefined);
  }
</script>

<Sheet bind:open {title}>
  {#if manual}
    <label class="field">
      {t("wifi.ssid")}
      <input
        bind:value={manualSsid}
        autocapitalize="none"
        autocorrect="off"
        autocomplete="off"
        spellcheck="false"
        enterkeyhint="next"
      />
    </label>
  {/if}
  <label class="field">
    {t("wifi.password")}
    <span class="pass">
      <input
        type={show ? "text" : "password"}
        bind:value={psk}
        onkeydown={(e) => e.key === "Enter" && submit()}
        autocapitalize="none"
        autocorrect="off"
        autocomplete="off"
        spellcheck="false"
        enterkeyhint="done"
      />
      <button type="button" class="reveal" onclick={() => (show = !show)}>
        {show ? t("wifi.hide") : t("wifi.show")}
      </button>
    </span>
  </label>
  {#if manual}
    <p class="hint">{t("wifi.manualHint")}</p>
  {/if}
  <button class="primary" disabled={!ready} onclick={submit}>
    {t("wifi.join")}
  </button>
</Sheet>

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    width: 100%;
    font-size: 13px;
    color: var(--dim);
  }
  .pass {
    position: relative;
    display: block;
  }
  .pass input {
    padding-right: 72px;
  }
  .reveal {
    position: absolute;
    right: var(--sp-2);
    top: 50%;
    transform: translateY(-50%);
    padding: var(--sp-1) var(--sp-2);
    border-radius: var(--r-8);
    font-size: 13px;
    font-weight: 600;
    color: var(--accent-live);
  }
  .hint {
    font-size: 13px;
    color: var(--mute);
  }
</style>
