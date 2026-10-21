import { describe, expect, it } from "vitest";
import { clampRatio, markClosing, neighbor, panesOf, removePane, splitPane, updateSplit, type LayoutNode } from "./layout";

const p = (id: string): LayoutNode => ({ kind: "pane", id });

describe("layout tree", () => {
  it("splits a pane in place and lists panes in order", () => {
    let t = splitPane(p("a"), "a", "row", "b", "s1");
    t = splitPane(t, "b", "col", "c", "s2");
    expect(panesOf(t)).toEqual(["a", "b", "c"]);
    expect(t.kind === "split" && t.born).toBe(true);
  });

  it("closing marks the parent split, removal promotes the sibling", () => {
    let t = splitPane(p("a"), "a", "row", "b", "s1");
    t = splitPane(t, "b", "col", "c", "s2");
    const marked = markClosing(t, "c")!;
    expect(marked.kind === "split" && marked.b.kind === "split" && marked.b.closing).toBe("b");
    const after = removePane(t, "c")!;
    expect(panesOf(after)).toEqual(["a", "b"]);
    expect(after.kind === "split" && after.b).toEqual(p("b"));
    expect(markClosing(p("a"), "a")).toBeNull();
    expect(removePane(p("a"), "a")).toBeNull();
  });

  it("updates ratios and clamps them", () => {
    const t = updateSplit(splitPane(p("a"), "a", "row", "b", "s1"), "s1", { ratio: clampRatio(0.99), born: false });
    expect(t.kind === "split" && t.ratio).toBeCloseTo(0.88);
  });
});

describe("neighbor", () => {
  //  a | b
  //  --+--
  //  c | b
  const rects = {
    a: { left: 0, top: 0, right: 100, bottom: 50 },
    c: { left: 0, top: 54, right: 100, bottom: 100 },
    b: { left: 104, top: 0, right: 200, bottom: 100 },
  };
  it("moves in the requested direction", () => {
    expect(neighbor(rects, "a", "right")).toBe("b");
    expect(neighbor(rects, "a", "down")).toBe("c");
    expect(neighbor(rects, "c", "up")).toBe("a");
    expect(neighbor(rects, "b", "left")).toBeTruthy();
  });
  it("returns null at the edge", () => {
    expect(neighbor(rects, "a", "left")).toBeNull();
    expect(neighbor(rects, "a", "up")).toBeNull();
  });
});
