import { describe, expect, it } from "vitest";
import { clickSelect, hitGrid, hitList, moveFocus, normRect, typeAhead, type SelState } from "./selection";

const order = ["a", "b", "c", "d", "e"];
const empty: SelState = { selection: new Set(), anchor: null, focus: null };
const arr = (s: SelState) => [...s.selection].sort();

describe("clickSelect", () => {
  it("selects one on plain click", () => {
    expect(arr(clickSelect(order, empty, "c", { shift: false, toggle: false }))).toEqual(["c"]);
  });
  it("extends a range with shift from the anchor", () => {
    const s1 = clickSelect(order, empty, "b", { shift: false, toggle: false });
    const s2 = clickSelect(order, s1, "d", { shift: true, toggle: false });
    expect(arr(s2)).toEqual(["b", "c", "d"]);
    // Shift-clicking the other side re-anchors from the same anchor.
    const s3 = clickSelect(order, s2, "a", { shift: true, toggle: false });
    expect(arr(s3)).toEqual(["a", "b"]);
  });
  it("toggles with the modifier", () => {
    const s1 = clickSelect(order, empty, "a", { shift: false, toggle: false });
    const s2 = clickSelect(order, s1, "c", { shift: false, toggle: true });
    expect(arr(s2)).toEqual(["a", "c"]);
    expect(arr(clickSelect(order, s2, "a", { shift: false, toggle: true }))).toEqual(["c"]);
  });
});

describe("moveFocus", () => {
  it("starts at the edges and clamps", () => {
    expect(moveFocus(order, empty, 1, false).focus).toBe("a");
    expect(moveFocus(order, empty, -1, false).focus).toBe("e");
    const s = moveFocus(order, { ...empty, focus: "e", anchor: "e" }, 1, false);
    expect(s.focus).toBe("e");
  });
  it("extends selection with shift", () => {
    const s = moveFocus(order, { selection: new Set(["b"]), anchor: "b", focus: "b" }, 2, true);
    expect(arr(s)).toEqual(["b", "c", "d"]);
  });
});

describe("typeAhead", () => {
  const names = ["Apple", "Banana", "berry", "Cherry", "blue"];
  it("finds prefixes case-insensitively", () => {
    expect(typeAhead(names, "ch", 0)).toBe(3);
    expect(typeAhead(names, "be", 0)).toBe(2);
  });
  it("cycles on repeated letters", () => {
    expect(typeAhead(names, "b", 1)).toBe(2);
    expect(typeAhead(names, "bb", 2)).toBe(4);
    expect(typeAhead(names, "z", 0)).toBe(-1);
  });
});

describe("rubber band hit testing", () => {
  it("list rows", () => {
    expect(hitList(normRect(10, 45, 100, 100), 10, 30, 0, 800)).toEqual([1, 2, 3]);
    expect(hitList(normRect(900, 0, 950, 50), 10, 30, 0, 800)).toEqual([]);
  });
  it("grid cells", () => {
    // 3 columns of 100x120 cells, no padding: a box over the first two cells of row 0.
    expect(hitGrid(normRect(20, 20, 180, 60), 9, 3, 100, 120, 0, 0)).toEqual([0, 1]);
    expect(hitGrid(normRect(20, 130, 50, 250), 4, 3, 100, 120, 0, 0)).toEqual([3]);
  });
});
