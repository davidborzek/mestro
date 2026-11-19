import { describe, expect, test } from "bun:test";
import {
  classifyJoinError,
  createJoinFlow,
  IDLE_JOIN,
  portalState,
  signalBars,
  unpairedLanding,
  withTimeout,
  type JoinFlowDeps,
  type JoinState,
} from "./wifi";
import type { WifiJoinedData, WifiStatusData } from "./protocol";

// ── the dBm → bars mapping ──

describe("signalBars", () => {
  test("maps dBm onto 4 bars at the usual client thresholds", () => {
    expect(signalBars(-30)).toBe(4);
    expect(signalBars(-55)).toBe(4);
    expect(signalBars(-56)).toBe(3);
    expect(signalBars(-67)).toBe(3);
    expect(signalBars(-68)).toBe(2);
    expect(signalBars(-75)).toBe(2);
    expect(signalBars(-76)).toBe(1);
    expect(signalBars(-90)).toBe(1);
  });
});

// ── the unpaired boot landing ──

const hotspotStatus = (state: WifiStatusData["state"]): WifiStatusData => ({
  state,
  ap_ssid: "mestro-6BEC",
  ssid: "FritzBox",
  error: "",
});

describe("unpairedLanding", () => {
  test("a hotspot status lands on the wifi setup — both portal states", () => {
    expect(unpairedLanding(hotspotStatus("ap"))).toBe("/setup/wifi");
    expect(unpairedLanding(hotspotStatus("failed"))).toBe("/setup/wifi");
    expect(portalState("ap")).toBe(true);
    expect(portalState("failed")).toBe(true);
  });

  test("a LAN status or no answer keeps the pair screen", () => {
    expect(unpairedLanding(hotspotStatus("sta"))).toBe(null);
    expect(unpairedLanding(hotspotStatus("joining"))).toBe(null);
    expect(unpairedLanding(null)).toBe(null);
    expect(portalState("sta")).toBe(false);
    expect(portalState("joining")).toBe(false);
  });
});

// ── the join flow state machine ──

function recording() {
  const states: JoinState[] = [];
  return { states, onState: (state: JoinState) => states.push({ ...state }) };
}

const last = (states: JoinState[]) => states[states.length - 1];
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

// the join flow's non-send side: a LAN change-trip by default,
// the portal tests flip it
const depsOf = (
  extra: Partial<JoinFlowDeps> & Pick<JoinFlowDeps, "send" | "onState">,
): JoinFlowDeps => ({
  status: () => Promise.resolve(hotspotStatus("failed")),
  portal: () => false,
  ...extra,
});

// ── the raw join errors → user-facing kinds ──

describe("classifyJoinError", () => {
  test("known backend strings fold into their kind", () => {
    expect(classifyJoinError("could not join Alt-Router: wrong password")).toBe("wrongPassword");
    expect(classifyJoinError("could not join Home: incorrect password")).toBe("wrongPassword");
    expect(classifyJoinError("busy")).toBe("busy");
    expect(classifyJoinError("could not join Home: network not found")).toBe("notFound");
    expect(classifyJoinError("no such network: Home")).toBe("notFound");
    expect(classifyJoinError("could not join Home: no address")).toBe("timeout");
    expect(classifyJoinError("join timed out")).toBe("timeout");
  });

  test("anything else stays unknown and keeps its raw detail", () => {
    expect(classifyJoinError("wpa_supplicant did not come up")).toBe("unknown");
    expect(classifyJoinError("connection lost")).toBe("unknown");
    expect(classifyJoinError("")).toBe("unknown");
  });
});

describe("createJoinFlow", () => {
  test("a successful join settles on success with the reply's ssid", async () => {
    const seen: Array<{ ssid?: string; psk?: string }> = [];
    const { states, onState } = recording();
    const flow = createJoinFlow(
      depsOf({
        send: async (ssid, psk) => {
          seen.push({ ssid, psk });
          return { ssid: ssid.toUpperCase(), state: "sta" } satisfies WifiJoinedData;
        },
        onState,
      }),
    );
    flow.start("homenet", "geheim");
    await withTimeout(Promise.resolve(), 10, "never");
    expect(seen).toEqual([{ ssid: "homenet", psk: "geheim" }]);
    expect(states).toEqual([
      { phase: "joining", ssid: "homenet", error: "" },
      { phase: "success", ssid: "HOMENET", error: "" },
    ]);
  });

  test("an open network joins without a psk — the key stays absent", async () => {
    const seen: Array<{ ssid: string; psk?: string }> = [];
    const { states, onState } = recording();
    const flow = createJoinFlow(
      depsOf({
        send: async (ssid, psk) => {
          seen.push({ ssid, psk });
          return { ssid, state: "sta" } satisfies WifiJoinedData;
        },
        onState,
      }),
    );
    flow.start("Gastnetz");
    await withTimeout(Promise.resolve(), 10, "never");
    expect(seen).toEqual([{ ssid: "Gastnetz", psk: undefined }]);
    expect(last(states)).toEqual({ phase: "success", ssid: "Gastnetz", error: "" });
  });

  test("a rejected join carries the backend's error back to the list", async () => {
    const { states, onState } = recording();
    const flow = createJoinFlow(
      depsOf({
        send: () => Promise.reject(new Error("could not join Alt-Router: wrong password")),
        onState,
      }),
    );
    flow.start("Alt-Router", "falsch");
    await withTimeout(Promise.resolve(), 10, "never");
    expect(last(states)).toEqual({
      phase: "failed",
      ssid: "Alt-Router",
      error: "could not join Alt-Router: wrong password",
    });
  });

  test("a join that never answers fails after the budget", async () => {
    const { states, onState } = recording();
    const flow = createJoinFlow(
      depsOf({
        send: () => new Promise(() => {}),
        onState,
        timeoutMs: 15,
      }),
    );
    flow.start("HomeNet", "geheim");
    await sleep(40);
    expect(last(states)).toEqual({
      phase: "failed",
      ssid: "HomeNet",
      error: "join timed out",
    });
  });

  test("a second start supersedes the first — its late reply changes nothing", async () => {
    const { states, onState } = recording();
    let release: (value: WifiJoinedData) => void = () => {};
    const flow = createJoinFlow(
      depsOf({
        send: () =>
          new Promise((resolve) => {
            release = resolve;
          }),
        onState,
      }),
    );
    flow.start("Alt-Router", "falsch");
    const stale = release;
    flow.start("HomeNet", "geheim");
    stale({ ssid: "Alt-Router", state: "sta" });
    await sleep(10);
    expect(states).toEqual([
      { phase: "joining", ssid: "Alt-Router", error: "" },
      { phase: "joining", ssid: "HomeNet", error: "" },
    ]);
  });

  test("reset returns to idle and ignores in-flight replies", async () => {
    const { states, onState } = recording();
    let release: (value: WifiJoinedData) => void = () => {};
    const flow = createJoinFlow(
      depsOf({
        send: () =>
          new Promise((resolve) => {
            release = resolve;
          }),
        onState,
      }),
    );
    flow.start("HomeNet", "geheim");
    flow.reset();
    release({ ssid: "HomeNet", state: "sta" });
    await withTimeout(Promise.resolve(), 10, "never");
    expect(states).toEqual([{ phase: "joining", ssid: "HomeNet", error: "" }, { ...IDLE_JOIN }]);
  });
});

// ── the portal's connection loss during a join ──

describe("createJoinFlow: an unconfirmed portal join", () => {
  test("a lost connection goes unconfirmed, and the returned hotspot's status names the error", async () => {
    const { states, onState } = recording();
    const flow = createJoinFlow(
      depsOf({
        send: () => Promise.reject(new Error("connection lost")),
        status: () =>
          Promise.resolve({
            ...hotspotStatus("failed"),
            error: "could not join Home: wrong password",
          }),
        portal: () => true,
        onState,
      }),
    );
    flow.start("Home", "geheim");
    await sleep(5);
    expect(states).toEqual([
      { phase: "joining", ssid: "Home", error: "" },
      { phase: "unconfirmed", ssid: "Home", error: "" },
    ]);
    // the hotspot (and with it the socket) came back: the join
    // failed, the status op names the reason
    flow.reconnected();
    await sleep(5);
    expect(last(states)).toEqual({
      phase: "failed",
      ssid: "Home",
      error: "could not join Home: wrong password",
    });
    expect(classifyJoinError(last(states).error)).toBe("wrongPassword");
  });

  test("a hotspot that never comes back reads as success after the wait", async () => {
    const { states, onState } = recording();
    const flow = createJoinFlow(
      depsOf({
        send: () => Promise.reject(new Error("connection lost")),
        portal: () => true,
        onState,
        confirmWaitMs: 20,
      }),
    );
    flow.start("Home", "geheim");
    await sleep(40);
    expect(last(states)).toEqual({ phase: "success", ssid: "Home", error: "" });
  });

  test("outside the portal a lost connection stays a failure — the phone is on the LAN", async () => {
    const { states, onState } = recording();
    const flow = createJoinFlow(
      depsOf({
        send: () => Promise.reject(new Error("connection lost")),
        portal: () => false,
        onState,
        confirmWaitMs: 20,
      }),
    );
    flow.start("Home", "geheim");
    await sleep(40);
    expect(last(states)).toEqual({
      phase: "failed",
      ssid: "Home",
      error: "connection lost",
    });
  });

  test("a reconnect outside a join changes nothing", async () => {
    const { states, onState } = recording();
    const flow = createJoinFlow(
      depsOf({
        send: () => Promise.reject(new Error("connection lost")),
        portal: () => true,
        onState,
      }),
    );
    flow.reconnected();
    await sleep(5);
    expect(states).toEqual([]);
  });
});
