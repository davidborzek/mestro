import { describe, expect, test } from "bun:test";
import {
  createAdopter,
  createPickerActions,
  filterZones,
  zoneLabel,
  type PickerState,
  type TimezoneSend,
} from "./timezone";

const ZONES = [
  "America/Argentina/Buenos_Aires",
  "America/New_York",
  "Asia/Tokyo",
  "Europe/Berlin",
  "Europe/London",
  "UTC",
];

describe("zoneLabel", () => {
  test("reads underscores as spaces and keeps the path", () => {
    expect(zoneLabel("America/New_York")).toBe("America/New York");
    expect(zoneLabel("UTC")).toBe("UTC");
  });
});

describe("filterZones", () => {
  test("an empty query keeps everything", () => {
    expect(filterZones(ZONES, "")).toEqual(ZONES);
    expect(filterZones(ZONES, "   ")).toEqual(ZONES);
  });

  test("matches case-insensitively, with spaces for underscores", () => {
    expect(filterZones(ZONES, "berlin")).toEqual(["Europe/Berlin"]);
    expect(filterZones(ZONES, "new york")).toEqual(["America/New_York"]);
    expect(filterZones(ZONES, "EUROPE")).toEqual(["Europe/Berlin", "Europe/London"]);
    expect(filterZones(ZONES, "nowhere")).toEqual([]);
  });
});

describe("createAdopter", () => {
  test("sends the browser zone once per page, however often it connects", async () => {
    const sent: unknown[] = [];
    const send: TimezoneSend = async (request) => {
      sent.push(request);
      return { zone: "Europe/Berlin", taken: true, restarting: false };
    };
    const adopt = createAdopter(send, () => "Europe/Berlin");
    await adopt();
    await adopt();
    await adopt();
    expect(sent).toEqual([{ op: "timezone", action: "adopt", zone: "Europe/Berlin" }]);
  });

  test("a browser without a zone sends nothing", async () => {
    const sent: unknown[] = [];
    const adopt = createAdopter(
      async (request) => {
        sent.push(request);
        return {};
      },
      () => "",
    );
    await adopt();
    expect(sent).toEqual([]);
  });

  test("a refusal (not paired yet, unknown zone) is swallowed and retried on the next connect", async () => {
    let attempts = 0;
    const adopt = createAdopter(
      async () => {
        attempts += 1;
        if (attempts === 1) throw new Error("pairing required");
        return { zone: "Europe/Berlin", taken: false, restarting: false };
      },
      () => "Europe/Berlin",
    );
    await adopt();
    await adopt();
    await adopt();
    expect(attempts).toBe(2);
  });
});

function pickerState(): PickerState {
  return { zones: [], chosen: "", current: null, loaded: false, applying: false, error: "" };
}

describe("createPickerActions", () => {
  const zonesReply = (chosen: string, current: string) => ({
    zones: ["Europe/Berlin", "Europe/London"],
    chosen,
    current,
  });

  test("a restarting reply keeps the pill up; the reconnect resets it and reloads the zones", async () => {
    const state = pickerState();
    let restarted = false;
    const send: TimezoneSend = async (request) => {
      if (request.action === "set") {
        restarted = true;
        return { zone: "Europe/London", taken: true, restarting: true };
      }
      return restarted
        ? zonesReply("Europe/London", "Europe/London")
        : zonesReply("", "Europe/Berlin");
    };
    const picker = createPickerActions(send, state);
    await picker.load();
    expect(state.current).toBe("Europe/Berlin");

    await picker.pick("Europe/London");
    expect(state.applying).toBe(true);
    expect(state.chosen).toBe("Europe/London");

    await picker.reconnected();
    expect(state.applying).toBe(false);
    expect(state.current).toBe("Europe/London");
    expect(state.zones).toEqual(["Europe/Berlin", "Europe/London"]);
  });

  test("a reply without a restart ends the busy state at once", async () => {
    const state = pickerState();
    const send: TimezoneSend = async () => ({
      zone: "Europe/London",
      taken: true,
      restarting: false,
    });
    await createPickerActions(send, state).pick("Europe/London");
    expect(state.applying).toBe(false);
    expect(state.error).toBe("");
  });

  test("a refused choice shows the error and frees the row", async () => {
    const state = pickerState();
    const send: TimezoneSend = async () => {
      throw new Error("unknown timezone Nowhere/Land");
    };
    await createPickerActions(send, state).pick("Nowhere/Land");
    expect(state.applying).toBe(false);
    expect(state.error).toBe("unknown timezone Nowhere/Land");
  });

  test("choosing the zone already in use sends nothing", async () => {
    const state = { ...pickerState(), chosen: "Europe/Berlin" };
    const sent: unknown[] = [];
    const send: TimezoneSend = async (request) => {
      sent.push(request);
      return {};
    };
    await createPickerActions(send, state).pick("Europe/Berlin");
    expect(sent).toEqual([]);
    expect(state.applying).toBe(false);
  });

  test("a failed reload on reconnect still frees the row", async () => {
    const state = { ...pickerState(), applying: true };
    const send: TimezoneSend = async () => {
      throw new Error("pairing required");
    };
    await createPickerActions(send, state).reconnected();
    expect(state.applying).toBe(false);
    expect(state.error).toBe("pairing required");
  });
});
