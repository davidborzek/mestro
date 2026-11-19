import { describe, expect, test } from "bun:test";
import {
  localizeProviderStatus,
  maProfileUrl,
  maSummary,
  needsLogin,
  normalizeMaUrl,
} from "./maConnection";
import type { ProbeData } from "./protocol";

const probe = (auth: ProbeData["auth"]): ProbeData => ({
  version: "2.8.0",
  schema: 28,
  auth,
});

describe("normalizeMaUrl", () => {
  test("prepends http:// to a bare host", () => {
    expect(normalizeMaUrl("ma.local:8095")).toBe("http://ma.local:8095");
  });

  test("keeps an explicit scheme", () => {
    expect(normalizeMaUrl("https://ma.local:8095")).toBe("https://ma.local:8095");
  });

  test("trims spaces and trailing slashes", () => {
    expect(normalizeMaUrl("  http://ma.local:8095///  ")).toBe("http://ma.local:8095");
    expect(normalizeMaUrl("ma.local:8095/")).toBe("http://ma.local:8095");
  });

  test("keeps inner path segments", () => {
    expect(normalizeMaUrl("ma.local:8095/ma/")).toBe("http://ma.local:8095/ma");
  });
});

describe("needsLogin", () => {
  test("a missing probe needs login", () => {
    expect(needsLogin(null)).toBe(true);
  });

  test("an auth-free server (schema < 28) needs no login", () => {
    expect(needsLogin(probe("none"))).toBe(false);
  });

  test("login and token-only servers need login", () => {
    expect(needsLogin(probe("login"))).toBe(true);
    expect(needsLogin(probe("token-only"))).toBe(true);
  });
});

describe("maProfileUrl", () => {
  test("points at MA's profile page", () => {
    expect(maProfileUrl("http://ma.local:8095/")).toBe("http://ma.local:8095/#/settings/profile");
  });
});

describe("localizeProviderStatus", () => {
  const leadins = {
    unreachable: "Music Assistant nicht erreichbar: ",
    auth: "Music Assistant – Anmeldung: ",
  };

  test("translates the unreachable lead-in, keeps the detail", () => {
    expect(localizeProviderStatus("Music Assistant unreachable: connect timeout", leadins)).toBe(
      "Music Assistant nicht erreichbar: connect timeout",
    );
  });

  test("translates a known detail too, unknown details stay", () => {
    const details = { "connect timeout": "Zeitüberschreitung beim Verbinden" };
    expect(
      localizeProviderStatus("Music Assistant unreachable: connect timeout", leadins, details),
    ).toBe("Music Assistant nicht erreichbar: Zeitüberschreitung beim Verbinden");
    expect(
      localizeProviderStatus("Music Assistant unreachable: TLS handshake eof", leadins, details),
    ).toBe("Music Assistant nicht erreichbar: TLS handshake eof");
  });

  test("translates the auth lead-in", () => {
    expect(localizeProviderStatus("Music Assistant: auth failed (error_code 23)", leadins)).toBe(
      "Music Assistant – Anmeldung: auth failed (error_code 23)",
    );
  });

  test("unknown texts pass through untouched", () => {
    expect(localizeProviderStatus("something else broke", leadins)).toBe("something else broke");
    expect(localizeProviderStatus("", leadins)).toBe("");
  });
});

describe("maSummary", () => {
  const labels = { connected: "Verbunden", notConnected: "Nicht verbunden" };

  test("connected shows the player", () => {
    expect(maSummary({ connected: true, player: "Wohnzimmer", providerStatus: "" }, labels)).toBe(
      "Verbunden · Wohnzimmer",
    );
  });

  test("connected without a player is just the label", () => {
    expect(maSummary({ connected: true, player: "", providerStatus: "" }, labels)).toBe(
      "Verbunden",
    );
  });

  test("disconnected shows the provider error, else the label", () => {
    expect(
      maSummary(
        { connected: false, player: "Wohnzimmer", providerStatus: "Music Assistant unreachable" },
        labels,
      ),
    ).toBe("Music Assistant unreachable");
    expect(maSummary({ connected: false, player: "", providerStatus: "" }, labels)).toBe(
      "Nicht verbunden",
    );
  });
});
