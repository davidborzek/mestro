import { describe, expect, test } from "bun:test";
import { availableDeviceSections, openDeviceSection } from "./settingsSections";

describe("availableDeviceSections", () => {
  test("lists audio before info when the build has the audio output", () => {
    expect(availableDeviceSections({ audioOutput: true })).toEqual(["audio", "info"]);
  });

  test("drops audio on a build without it, info is always there", () => {
    expect(availableDeviceSections({ audioOutput: false })).toEqual(["info"]);
  });
});

describe("openDeviceSection", () => {
  test("opens the device sections this build has", () => {
    expect(openDeviceSection("audio", { audioOutput: true })).toBe("audio");
    expect(openDeviceSection("info", { audioOutput: true })).toBe("info");
  });

  test("an unavailable device section is treated like an unknown one", () => {
    expect(openDeviceSection("audio", { audioOutput: false })).toBeNull();
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
