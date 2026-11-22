import { expect, test } from "bun:test";
import { tileAvailable } from "./remoteTiles";

test("the queue tile is off while an external source plays", () => {
  expect(tileAvailable("queue", "AirPlay")).toBe(false);
  expect(tileAvailable("queue", "Spotify Connect")).toBe(false);
});

test("the queue tile is on for Music Assistant's own queue", () => {
  expect(tileAvailable("queue", "")).toBe(true);
});

test("the other tiles do not care about the source", () => {
  for (const cmd of ["now-playing", "vu"] as const) {
    expect(tileAvailable(cmd, "AirPlay")).toBe(true);
    expect(tileAvailable(cmd, "")).toBe(true);
  }
});
