// wifi setup logic: the join flow state machine plus the dBm→bars
// mapping. DOM-free and rune-free — the reactive page state lives
// in the components, this core is what the tests drive.

import type { WifiJoinedData, WifiStatusData } from "./protocol";

// 4 bars from dBm, the usual client thresholds
export function signalBars(dbm: number): number {
  if (dbm >= -55) return 4;
  if (dbm >= -67) return 3;
  if (dbm >= -75) return 2;
  return 1;
}

export type JoinPhase = "idle" | "joining" | "unconfirmed" | "success" | "failed";

export interface JoinState {
  phase: JoinPhase;
  ssid: string;
  error: string;
}

export const IDLE_JOIN: JoinState = { phase: "idle", ssid: "", error: "" };

// the backend's own join budget — the web gives up with it
export const JOIN_TIMEOUT_MS = 45_000;

// how long an unconfirmed portal join waits for the hotspot to
// come back before it reads as success: the socket returning is
// the failure (only a FAILED join brings the hotspot up again)
export const UNCONFIRMED_WAIT_MS = 60_000;

export async function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(message)), ms);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

// the wifi states that mean "this client sits on the player's own
// hotspot": the live one, or one that failed while the portal
// page was still open
export function portalState(state: WifiStatusData["state"]): boolean {
  return state === "ap" || state === "failed";
}

// the boot landing for an unpaired client: on the player's own
// hotspot it is the wifi setup, never the pair screen — a LAN
// client or an unreachable player keeps the pair screen
export function unpairedLanding(status: WifiStatusData | null): string | null {
  return status !== null && portalState(status.state) ? "/setup/wifi" : null;
}

export type JoinErrorKind = "wrongPassword" | "notFound" | "timeout" | "busy" | "unknown";

// folds the backend's raw error strings into the kinds the UI can
// phrase — anything unrecognized stays "unknown" and keeps its
// raw text as the detail line
export function classifyJoinError(error: string): JoinErrorKind {
  const text = error.toLowerCase();
  if (text === "busy" || text.includes("busy")) return "busy";
  if (text.includes("wrong password") || text.includes("incorrect password")) {
    return "wrongPassword";
  }
  if (text.includes("not found") || text.includes("no such network")) {
    return "notFound";
  }
  // "no address" is the backend's own 45 s join budget running out
  if (text.includes("timed out") || text.includes("timeout") || text.includes("no address")) {
    return "timeout";
  }
  return "unknown";
}

export interface JoinFlowDeps {
  // sends the join op — psk absent for open networks, the device
  // hashes it and never echoes it back
  send(ssid: string, psk?: string): Promise<WifiJoinedData>;
  onState(state: JoinState): void;
  // reads the backend's wifi status — the portal's only fact
  // once the hotspot is back
  status(): Promise<WifiStatusData>;
  // the portal context: there a lost connection means the
  // hotspot (and with it the socket) went away — usually because
  // the join worked. Reactive — read on every use
  portal(): boolean;
  timeoutMs?: number;
  // the unconfirmed wait (tests shrink it)
  confirmWaitMs?: number;
}

export interface JoinFlow {
  start(ssid: string, psk?: string): void;
  reset(): void;
  // the socket came back: the hotspot is up again, which only a
  // FAILED join does — read the error from the status op
  reconnected(): void;
}

// one join at a time: `start` supersedes whatever ran before, so a
// late reply from an abandoned attempt cannot settle the new one
export function createJoinFlow(deps: JoinFlowDeps): JoinFlow {
  const timeout = deps.timeoutMs ?? JOIN_TIMEOUT_MS;
  const confirmWait = deps.confirmWaitMs ?? UNCONFIRMED_WAIT_MS;
  let run = 0;
  let unconfirmed: { ssid: string; my: number } | null = null;
  let confirmTimer: ReturnType<typeof setTimeout> | null = null;

  function stopWaiting(): void {
    unconfirmed = null;
    if (confirmTimer) clearTimeout(confirmTimer);
    confirmTimer = null;
  }

  function settle(my: number, state: JoinState): void {
    if (my !== run) return;
    stopWaiting();
    deps.onState(state);
  }

  return {
    start(ssid, psk) {
      const my = ++run;
      stopWaiting();
      deps.onState({ phase: "joining", ssid, error: "" });
      withTimeout(deps.send(ssid, psk), timeout, "join timed out").then(
        (joined) => settle(my, { phase: "success", ssid: joined.ssid || ssid, error: "" }),
        (error: unknown) => {
          if (my !== run) return;
          const message = error instanceof Error ? error.message : String(error);
          // in the portal a lost connection means the hotspot
          // went away — most likely because the join worked. That
          // is UNCONFIRMED, not success: the hotspot coming back
          // is the failure, its status names the reason
          if (deps.portal() && message === "connection lost") {
            unconfirmed = { ssid, my };
            deps.onState({ phase: "unconfirmed", ssid, error: "" });
            confirmTimer = setTimeout(() => {
              if (unconfirmed?.my !== my) return;
              settle(my, { phase: "success", ssid, error: "" });
            }, confirmWait);
            return;
          }
          settle(my, { phase: "failed", ssid, error: message });
        },
      );
    },
    reconnected() {
      if (unconfirmed === null) return;
      const { ssid, my } = unconfirmed;
      stopWaiting();
      deps
        .status()
        .then((status) => {
          if (my !== run) return;
          deps.onState({
            phase: "failed",
            ssid,
            error: status.state === "failed" ? status.error : "connection lost",
          });
        })
        .catch(() => {
          if (my !== run) return;
          deps.onState({ phase: "failed", ssid, error: "connection lost" });
        });
    },
    reset() {
      run += 1; // in-flight replies land on a dead run
      stopWaiting();
      deps.onState({ ...IDLE_JOIN });
    },
  };
}
