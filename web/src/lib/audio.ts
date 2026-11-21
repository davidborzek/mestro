// the audio output picker's logic: DOM-free, the page state lives
// in the component. `send` is the socket's request function.

import type { AudioOp, AudioOutput, AudioOutputsData } from "./protocol";

export type AudioSend = (request: { op: "audio" } & AudioOp) => Promise<unknown>;

export function currentOutput(outputs: AudioOutput[]): AudioOutput | null {
  return outputs.find((output) => output.current) ?? null;
}

// the row's value: the device in use, else the placeholder (the
// env file names a device that is not plugged in)
export function outputLabel(outputs: AudioOutput[], placeholder: string): string {
  return currentOutput(outputs)?.name ?? placeholder;
}

export async function loadOutputs(send: AudioSend): Promise<AudioOutput[]> {
  const data = (await send({ op: "audio", action: "outputs" })) as AudioOutputsData;
  return data.outputs;
}

export interface SwitchResult {
  // the list as the device reports it after the attempt
  outputs: AudioOutput[];
  // the device's error text, "" on success
  error: string;
}

// switch and re-read: after a failure the device has rolled back, so
// the list shows what really plays
export async function switchOutput(send: AudioSend, id: string): Promise<SwitchResult> {
  let error = "";
  try {
    await send({ op: "audio", action: "set", output: id });
  } catch (cause) {
    error = cause instanceof Error ? cause.message : String(cause);
  }
  return { outputs: await loadOutputs(send), error };
}
