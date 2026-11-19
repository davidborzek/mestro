// optimistic overrides: a tap shows its effect immediately, the
// confirming push (or a 3 s fallback) settles it back to the
// pushed truth. iOS paints a tap's feedback only when it comes
// from local state — a round-trip over the wire reads as lag.
//
// The keys are the ctl commands (plus "vu-index" for the skin
// cycle): a push confirms an override when the pushed value it
// maps to equals it (see confirmOptimistic).

import type { RemoteState } from "./protocol";

const FALLBACK_MS = 3000;

class Optimistic {
  private values = $state<Record<string, unknown>>({});
  private deadlines = new Map<string, ReturnType<typeof setTimeout>>();

  // the effective value: the override while one runs, else the
  // pushed truth
  pick(key: string, pushed: unknown): unknown {
    return Object.prototype.hasOwnProperty.call(this.values, key) ? this.values[key] : pushed;
  }

  set(key: string, value: unknown): void {
    this.values[key] = value;
    const timer = this.deadlines.get(key);
    if (timer) clearTimeout(timer);
    // the pushed truth wins again when no confirmation arrives
    this.deadlines.set(
      key,
      setTimeout(() => this.clear(key), FALLBACK_MS),
    );
  }

  // a push confirms the override when the pushed truth matches it
  confirm(key: string, pushed: unknown): void {
    if (Object.prototype.hasOwnProperty.call(this.values, key) && this.values[key] === pushed)
      this.clear(key);
  }

  clear(key: string): void {
    const timer = this.deadlines.get(key);
    if (timer) clearTimeout(timer);
    this.deadlines.delete(key);
    delete this.values[key];
  }
}

export const optimistic = new Optimistic();

// which pushed value confirms which override — called from the
// push feed on every state frame
export function confirmOptimistic(state: RemoteState): void {
  optimistic.confirm("now-playing", state.view.now_playing);
  optimistic.confirm("queue", state.view.queue);
  optimistic.confirm("vu", state.view.vu);
  optimistic.confirm("standby-toggle", state.player.powered);
  optimistic.confirm("vu-index", state.view.vu_index);
}
