// fire-and-forget remote commands: every control also exists in
// the pushed state, so a lost reply self-corrects on the next
// push. The Diagnose row tracks their timings.

import { socket } from "./connection.svelte";
import { trackCommand } from "./diagnostics.svelte";
import type { CtlCmd, PlayerOp } from "./protocol";

export function ctl(cmd: CtlCmd): void {
  trackCommand(
    `ctl ${cmd}`,
    socket.send({ op: "ctl", cmd }).catch(() => {}),
  );
}

export function player(action: PlayerOp): Promise<unknown> {
  const run = socket.send({ op: "player", ...action });
  trackCommand(
    `player ${action.action}`,
    run.catch(() => {}),
  );
  return run;
}

export function setSetting(key: string, value: unknown): Promise<unknown> {
  const run = socket.send({ op: "settings", action: "set", key, value });
  trackCommand(
    `set ${key}`,
    run.catch(() => {}),
  );
  return run;
}
