import { describe, expect, test } from "bun:test";
import { isValidPin, normalizePin, PIN_ALPHABET, PIN_LENGTH } from "./pairing";

describe("normalizePin", () => {
  test("uppercases and drops characters outside the alphabet", () => {
    expect(normalizePin("ab1c-de0")).toBe("ABCDE");
  });

  test("keeps the full alphabet", () => {
    expect(normalizePin("abcdefgh")).toBe("ABCDEFGH");
    expect(normalizePin("wxyz2345")).toBe("WXYZ2345");
  });

  test("never exceeds the PIN length", () => {
    expect(normalizePin("ABCDEFGHJKLMNPQRS")).toBe("ABCDEFGH");
  });

  test("returns empty for garbage", () => {
    expect(normalizePin("0101iioo")).toBe("");
  });
});

describe("isValidPin", () => {
  test("accepts 8 alphabet characters", () => {
    expect(isValidPin("7QWB3KMR")).toBe(true);
  });

  test("rejects wrong length and non-alphabet characters", () => {
    expect(isValidPin("7QWB3KMRX")).toBe(false);
    expect(isValidPin("7QWB3KM")).toBe(false);
    expect(isValidPin("7QWB3KM0")).toBe(false);
    expect(isValidPin("")).toBe(false);
  });

  test("covers every character of the declared alphabet", () => {
    expect(PIN_ALPHABET).toHaveLength(32);
    expect(PIN_LENGTH).toBe(8);
    for (const c of PIN_ALPHABET) expect(isValidPin(c.repeat(8))).toBe(true);
  });
});
