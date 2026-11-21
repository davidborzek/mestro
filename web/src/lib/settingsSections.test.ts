import { describe, expect, test } from "bun:test";
import {
  availableDeviceSections,
  openDeviceSection,
  systemBlocks,
  systemSummaryText,
} from "./settingsSections";

describe("availableDeviceSections", () => {
  test("lists audio, system, info in that order when the build has the audio output", () => {
    expect(availableDeviceSections({ audioOutput: true })).toEqual(["audio", "system", "info"]);
  });

  test("drops audio on a build without it, system and info are always there", () => {
    expect(availableDeviceSections({ audioOutput: false })).toEqual(["system", "info"]);
  });
});

describe("openDeviceSection", () => {
  test("opens the device sections this build has", () => {
    expect(openDeviceSection("audio", { audioOutput: true })).toBe("audio");
    expect(openDeviceSection("system", { audioOutput: true })).toBe("system");
    expect(openDeviceSection("info", { audioOutput: true })).toBe("info");
  });

  test("an unavailable device section is treated like an unknown one", () => {
    expect(openDeviceSection("audio", { audioOutput: false })).toBeNull();
    expect(openDeviceSection("system", { audioOutput: false })).toBe("system");
    expect(openDeviceSection("info", { audioOutput: false })).toBe("info");
  });

  test("schema, connection, unknown and no section are not device sections", () => {
    const facts = { audioOutput: true };
    expect(openDeviceSection("vu", facts)).toBeNull();
    expect(openDeviceSection("connection", facts)).toBeNull();
    expect(openDeviceSection("nope", facts)).toBeNull();
    expect(openDeviceSection(null, facts)).toBeNull();
  });
});

describe("systemBlocks", () => {
  test("a full build shows every block", () => {
    expect(systemBlocks({ timezone: true, wifi: true, factoryReset: true })).toEqual({
      timezone: true,
      wifi: true,
      network: true,
      factoryReset: true,
    });
  });

  test("the network card needs the timezone or the wifi lane", () => {
    expect(systemBlocks({ timezone: true, wifi: false, factoryReset: false }).network).toBe(true);
    expect(systemBlocks({ timezone: false, wifi: true, factoryReset: false }).network).toBe(true);
    expect(systemBlocks({ timezone: false, wifi: false, factoryReset: false })).toEqual({
      timezone: false,
      wifi: false,
      network: false,
      factoryReset: false,
    });
  });

  test("the danger zone follows the factory reset capability alone", () => {
    expect(systemBlocks({ timezone: false, wifi: false, factoryReset: true }).factoryReset).toBe(
      true,
    );
  });
});

describe("systemSummaryText", () => {
  const full = { timezone: true, wifi: true, factoryReset: true };

  test("prefers the wifi network", () => {
    expect(systemSummaryText(full, "HomeNet", "Europe/Berlin")).toBe("HomeNet");
  });

  test("falls back to the timezone while the network is unknown or absent", () => {
    expect(systemSummaryText(full, "", "Europe/Berlin")).toBe("Europe/Berlin");
    expect(systemSummaryText({ ...full, wifi: false }, "HomeNet", "Europe/Berlin")).toBe(
      "Europe/Berlin",
    );
  });

  test("is empty when neither is known or the capability is missing", () => {
    expect(systemSummaryText(full, "", null)).toBe("");
    expect(systemSummaryText({ ...full, timezone: false }, "", "Europe/Berlin")).toBe("");
  });
});
