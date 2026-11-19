import { describe, expect, test } from "bun:test";
import { confirmMatches } from "./confirmWord";

describe("confirmMatches", () => {
  test("the word matches case-insensitively and tolerates padding", () => {
    expect(confirmMatches("ZURÜCKSETZEN", "ZURÜCKSETZEN")).toBe(true);
    expect(confirmMatches("zurücksetzen", "ZURÜCKSETZEN")).toBe(true);
    expect(confirmMatches("  Zurücksetzen  ", "ZURÜCKSETZEN")).toBe(true);
    expect(confirmMatches("reset", "RESET")).toBe(true);
    expect(confirmMatches("Reset ", "RESET")).toBe(true);
  });

  test("anything else keeps the button locked", () => {
    expect(confirmMatches("", "ZURÜCKSETZEN")).toBe(false);
    expect(confirmMatches("zurücksetze", "ZURÜCKSETZEN")).toBe(false);
    // no umlaut substitution
    expect(confirmMatches("ZURUCKSETZEN", "ZURÜCKSETZEN")).toBe(false);
    expect(confirmMatches("reset!", "RESET")).toBe(false);
    expect(confirmMatches("RESET ", "ZURÜCKSETZEN")).toBe(false);
  });
});
