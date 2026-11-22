import { expect, test } from "bun:test";
import { playPauseAvailable, tileAvailable } from "./remoteTiles";

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

const caps = { play_pause: true, seek: true, next_previous: true, shuffle: true, repeat: true };

test("play/pause follows the source's capability", () => {
  expect(playPauseAvailable({ ...caps, play_pause: false })).toBe(false);
  expect(playPauseAvailable(caps)).toBe(true);
});

test("a server without source_caps allows play/pause", () => {
  expect(playPauseAvailable(undefined)).toBe(true);
});
