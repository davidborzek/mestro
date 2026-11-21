import { describe, expect, test } from "bun:test";
import { createAdopter, filterZones, zoneLabel, type TimezoneSend } from "./timezone";

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
