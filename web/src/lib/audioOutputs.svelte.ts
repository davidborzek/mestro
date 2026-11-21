// the device's audio outputs, shared by the settings list (its
// Audio summary) and the picker block

import { socket } from "./connection.svelte";
import { loadOutputs } from "./audio";
import type { AudioOutput } from "./protocol";

export const audioOutputs = $state({
  outputs: [] as AudioOutput[],
  // the first read finished, successfully or not
  loaded: false,
});

// re-reads the list; a failed read rejects and keeps the last list
export async function refreshAudioOutputs(): Promise<void> {
  try {
    audioOutputs.outputs = await loadOutputs((request) => socket.send(request));
  } finally {
    audioOutputs.loaded = true;
  }
}
