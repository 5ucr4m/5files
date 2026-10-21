// Split-pane layout as a binary tree (tmux-style). Pure functions, no UI.

/** "row": side by side (vertical divider). "col": stacked (horizontal divider). */
export type SplitDir = "row" | "col";
export type Direction = "left" | "right" | "up" | "down";

export type LayoutNode =
  | { kind: "pane"; id: string }
  | {
      kind: "split";
      id: string;
      dir: SplitDir;
      /** Share of the first child (0..1). */
      ratio: number;
      a: LayoutNode;
      b: LayoutNode;
      /** Just created: animates open from the edge. */
      born?: boolean;
      /** Child being closed: animates shut, then is removed. */
      closing?: "a" | "b";
    };

export const MIN_RATIO = 0.12;
export const clampRatio = (r: number) => Math.min(1 - MIN_RATIO, Math.max(MIN_RATIO, r));

export function panesOf(n: LayoutNode): string[] {
  return n.kind === "pane" ? [n.id] : [...panesOf(n.a), ...panesOf(n.b)];
}

/** Replaces leaf `paneId` with a split holding it and `newId` (new one after/below). */
export function splitPane(n: LayoutNode, paneId: string, dir: SplitDir, newId: string, splitId: string): LayoutNode {
  if (n.kind === "pane") {
    return n.id === paneId ? { kind: "split", id: splitId, dir, ratio: 0.5, a: n, b: { kind: "pane", id: newId }, born: true } : n;
  }
  return { ...n, a: splitPane(n.a, paneId, dir, newId, splitId), b: splitPane(n.b, paneId, dir, newId, splitId) };
}

/** Flags the split whose direct child is `paneId` so it animates shut. Null when the pane is the root. */
export function markClosing(n: LayoutNode, paneId: string): LayoutNode | null {
  if (n.kind === "pane") return null;
  const side = n.a.kind === "pane" && n.a.id === paneId ? "a" : n.b.kind === "pane" && n.b.id === paneId ? "b" : null;
  if (side) return { ...n, closing: side };
  const a = markClosing(n.a, paneId);
  if (a) return { ...n, a };
  const b = markClosing(n.b, paneId);
  return b ? { ...n, b } : null;
}

/** Removes `paneId`; its sibling takes the parent split's place. Null if nothing is left. */
export function removePane(n: LayoutNode, paneId: string): LayoutNode | null {
  if (n.kind === "pane") return n.id === paneId ? null : n;
  const a = removePane(n.a, paneId);
  const b = removePane(n.b, paneId);
  if (!a) return b;
  if (!b) return a;
  return a === n.a && b === n.b ? n : { ...n, a, b };
}

export function updateSplit(n: LayoutNode, splitId: string, patch: Partial<Extract<LayoutNode, { kind: "split" }>>): LayoutNode {
  if (n.kind === "pane") return n;
  if (n.id === splitId) return { ...n, ...patch } as LayoutNode;
  return { ...n, a: updateSplit(n.a, splitId, patch), b: updateSplit(n.b, splitId, patch) };
}

export function findSplit(n: LayoutNode, splitId: string): Extract<LayoutNode, { kind: "split" }> | null {
  if (n.kind === "pane") return null;
  if (n.id === splitId) return n;
  return findSplit(n.a, splitId) ?? findSplit(n.b, splitId);
}

/** Replaces a pane id (used when restoring a saved layout). */
export function mapPanes(n: LayoutNode, f: (id: string) => string): LayoutNode {
  return n.kind === "pane" ? { kind: "pane", id: f(n.id) } : { ...n, a: mapPanes(n.a, f), b: mapPanes(n.b, f) };
}

export interface Rect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/**
 * tmux-style directional focus: among panes that lie entirely on the requested side,
 * prefer those overlapping on the other axis, then the nearest edge.
 */
export function neighbor(rects: Record<string, Rect>, from: string, dir: Direction): string | null {
  const r = rects[from];
  if (!r) return null;
  let best: { id: string; score: number } | null = null;
  for (const [id, c] of Object.entries(rects)) {
    if (id === from) continue;
    let gap: number;
    let overlap: number;
    if (dir === "left" || dir === "right") {
      gap = dir === "left" ? r.left - c.right : c.left - r.right;
      overlap = Math.min(r.bottom, c.bottom) - Math.max(r.top, c.top);
    } else {
      gap = dir === "up" ? r.top - c.bottom : c.top - r.bottom;
      overlap = Math.min(r.right, c.right) - Math.max(r.left, c.left);
    }
    if (gap < -2) continue; // not on that side
    const score = gap + (overlap > 0 ? 0 : 10_000) - Math.max(0, overlap) * 0.001;
    if (!best || score < best.score) best = { id, score };
  }
  return best?.id ?? null;
}
