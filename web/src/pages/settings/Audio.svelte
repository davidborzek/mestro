<script lang="ts">
  import { settingsStore } from "../../lib/settings.svelte";
  import { audioTextFields } from "../../lib/audio";
  import AudioOutput from "./AudioOutput.svelte";
  import FieldRow from "./controls/FieldRow.svelte";

  // the hand-built Audio section: the output picker (the device's
  // list, it writes the audio.output setting) and a card with the
  // schema's other audio fields (name, server)
  const fields = $derived(audioTextFields(settingsStore.schema));
</script>

<AudioOutput />

{#if fields.length > 0}
  <section class="card">
    {#each fields as field (field.key)}
      <FieldRow {field} />
    {/each}
  </section>
{/if}
