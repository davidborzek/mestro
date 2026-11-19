import { describe, expect, test } from "bun:test";
import { dropIndex, moveItem, shiftOffset } from "./reorder";

describe("dropIndex", () => {
  test("half a row past the neighbor counts as crossing it", () => {
    expect(dropIndex(1, 29, 60, 3)).toBe(1);
    expect(dropIndex(1, 30, 60, 3)).toBe(2);
    expect(dropIndex(1, -30, 60, 3)).toBe(0);
    expect(dropIndex(1, -29, 60, 3)).toBe(1);
  });

  test("a long drag lands on the crossed row", () => {
    expect(dropIndex(0, 2.4 * 56, 56, 10)).toBe(2);
    expect(dropIndex(4, -2.4 * 56, 56, 10)).toBe(2);
  });

  test("clamps to the list bounds", () => {
    expect(dropIndex(0, 10000, 56, 3)).toBe(2);
    expect(dropIndex(2, -10000, 56, 3)).toBe(0);
  });

  test("degenerate inputs keep the row in place", () => {
    expect(dropIndex(1, 120, 0, 3)).toBe(1);
    expect(dropIndex(1, 120, -60, 3)).toBe(1);
    expect(dropIndex(1, 120, 60, 0)).toBe(1);
  });
});

describe("shiftOffset", () => {
  test("rows between from and to make room", () => {
    expect(shiftOffset(1, 0, 2, 56)).toBe(-56);
    expect(shiftOffset(2, 0, 2, 56)).toBe(-56);
    expect(shiftOffset(0, 0, 2, 56)).toBe(0);
    expect(shiftOffset(3, 0, 2, 56)).toBe(0);

    expect(shiftOffset(0, 2, 0, 56)).toBe(56);
    expect(shiftOffset(1, 2, 0, 56)).toBe(56);
    expect(shiftOffset(2, 2, 0, 56)).toBe(0);
    expect(shiftOffset(3, 2, 0, 56)).toBe(0);
  });

  test("a no-op drag moves nothing", () => {
    for (let index = 0; index < 4; index++) {
      expect(shiftOffset(index, 1, 1, 56)).toBe(0);
    }
  });
});

describe("moveItem", () => {
  test("moves an entry down and up", () => {
    expect(moveItem(["a", "b", "c"], 0, 2)).toEqual(["b", "c", "a"]);
    expect(moveItem(["a", "b", "c"], 2, 0)).toEqual(["c", "a", "b"]);
  });

  test("a no-op drop returns the list untouched", () => {
    const list = ["a", "b", "c"];
    expect(moveItem(list, 1, 1)).toBe(list);
    expect(moveItem(list, -1, 0)).toBe(list);
    expect(moveItem(list, 0, 3)).toBe(list);
  });
});
