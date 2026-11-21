<script lang="ts">
  import { onMount } from "svelte";
  import { socket } from "../../lib/connection.svelte";
  import { t } from "../../lib/i18n.svelte";
  import { loadOutputs, outputLabel, switchOutput, type AudioSend } from "../../lib/audio";
  import type { AudioOutput } from "../../lib/protocol";
  import Icon from "../../components/Icon.svelte";
  import Sheet from "../../components/Sheet.svelte";

  // the audio card of the Info section: the ALSA playback devices
  // the player offers, the one in use marked. Choosing one makes the
  // device restart squeezelite; a device that cannot start is rolled
  // back by the player and comes back here as the error text.

  const send: AudioSend = (request) => socket.send(request);

  let outputs = $state<AudioOutput[]>([]);
  let loaded = $state(false);
  let open = $state(false);
  let switching = $state(false);
  let error = $state("");

  onMount(() => {
    loadOutputs(send)
      .then((list) => (outputs = list))
      .catch((cause: unknown) => (error = cause instanceof Error ? cause.message : String(cause)))
      .finally(() => (loaded = true));
  });

  async function pick(id: string) {
    open = false;
    if (outputs.find((output) => output.current)?.id === id) return;
    switching = true;
    error = "";
    const result = await switchOutput(send, id).catch((cause: unknown) => ({
      outputs,
      error: cause instanceof Error ? cause.message : String(cause),
    }));
    outputs = result.outputs;
    error = result.error;
    switching = false;
  }
</script>

<section class="card">
  <h2>{t("audio.section")}</h2>
  {#if !loaded}
    <p class="muted">{t("audio.loading")}</p>
  {:else if outputs.length === 0}
    <p class="muted">{t("audio.empty")}</p>
  {:else}
    <button class="select" disabled={switching} onclick={() => (open = true)}>
      <span class="label">{t("audio.output")}</span>
      <span class="value">
        {#if switching}
          <span class="dot pulse"></span> {t("audio.switching")}
        {:else}
          {outputLabel(outputs, t("audio.unknown"))}
        {/if}
      </span>
      <Icon name="chevronDown" size={18} />
    </button>
  {/if}
  {#if error}
    <div class="err" role="alert"><Icon name="alert" size={16} /> {error}</div>
  {/if}
</section>

<Sheet bind:open title={t("audio.output")}>
  <div class="options">
    {#each outputs as output (output.id)}
      <button class="option" class:active={output.current} onclick={() => void pick(output.id)}>
        <span class="option-label">{output.name}</span>
        {#if output.current}
          <Icon name="check" size={20} />
        {/if}
      </button>
    {/each}
  </div>
  <p class="hint">{t("audio.hint")}</p>
</Sheet>

<style>
  .muted {
    font-size: 15px;
    color: var(--dim);
  }
  .select {
    min-height: 44px;
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    font-size: 15px;
    color: var(--text);
  }
  .label {
    flex: 0 0 auto;
    color: var(--dim);
  }
  .value {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: var(--sp-2);
    font-weight: 600;
    text-align: right;
    overflow-wrap: anywhere;
  }
  .select :global(svg) {
    flex: 0 0 auto;
    color: var(--mute);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 4px;
    background: var(--accent-live);
  }
  .dot.pulse {
    animation: audio-pulse 1.2s ease-in-out infinite;
  }
  @keyframes audio-pulse {
    50% {
      opacity: 0.3;
    }
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
    text-align: left;
  }
  .option.active {
    color: var(--accent-live);
    font-weight: 650;
  }
  .option:active {
    background: var(--pill);
  }
  .hint {
    margin-top: var(--sp-3);
    font-size: 13px;
    color: var(--dim);
  }
</style>
