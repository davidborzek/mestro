// real-device numbers for the Settings → Info → Diagnose row:
// socket health, the keepalive round-trip and per-command timings
// (time to the reply, time until a push confirmed the change).
// Dev-facing by design — no external libs, values stay raw.

import { socket } from "./connection.svelte";

export interface CommandTiming {
  label: string;
  sentAt: number;
  // ms to the reply, null while it is out
  replyMs: number | null;
  // ms to the first state push after the command — the device's
  // confirmation of the change
  confirmMs: number | null;
  error: string;
  // pushes seen before the command went out (the confirm marker)
  pushesBefore: number;
}

export const diagnostics = $state({
  reconnects: 0,
  keepaliveMs: null as number | null,
  commands: [] as CommandTiming[],
});

// pushes seen since boot — a command is confirmed by any push
// that arrives after it went out
let pushes = 0;
const HISTORY = 5;

export function startDiagnostics(): void {
  socket.onOpen(() => (diagnostics.reconnects = socket.reconnectCount));
  socket.onKeepalive((ms) => (diagnostics.keepaliveMs = ms));
  socket.onPush(() => {
    pushes += 1;
    const now = Date.now();
    for (const command of diagnostics.commands) {
      if (command.confirmMs === null && pushes > command.pushesBefore) {
        command.confirmMs = now - command.sentAt;
      }
    }
  });
}

// wraps one fire-and-forget command: reply time always, the
// confirming push's arrival on top
export function trackCommand(label: string, run: Promise<unknown>): void {
  const command: CommandTiming = {
    label,
    sentAt: Date.now(),
    replyMs: null,
    confirmMs: null,
    error: "",
    pushesBefore: pushes,
  };
  diagnostics.commands.unshift(command);
  if (diagnostics.commands.length > HISTORY) diagnostics.commands.pop();
  const sentAt = command.sentAt;
  run.then(
    () => {
      command.replyMs = Date.now() - sentAt;
    },
    (error: unknown) => {
      command.replyMs = Date.now() - sentAt;
      command.error = error instanceof Error ? error.message : String(error);
    },
  );
}
