// Pure selection logic shared by list and grid views.

export interface ClickMods {
  shift: boolean;
  toggle: boolean; // ⌘ on macOS, Ctrl elsewhere
}

export interface SelState {
  selection: Set<string>;
  anchor: string | null;
  focus: string | null;
}

/** Finder/Explorer semantics: plain click selects one, toggle adds/removes, shift selects a range from the anchor. */
export function clickSelect(order: string[], cur: SelState, clicked: string, m: ClickMods): SelState {
  if (m.shift && cur.anchor && order.includes(cur.anchor)) {
    const range = rangeBetween(order, cur.anchor, clicked);
    const selection = m.toggle ? new Set([...cur.selection, ...range]) : new Set(range);
    return { selection, anchor: cur.anchor, focus: clicked };
  }
  if (m.toggle) {
    const selection = new Set(cur.selection);
    if (selection.has(clicked)) selection.delete(clicked);
    else selection.add(clicked);
    return { selection, anchor: clicked, focus: clicked };
  }
  return { selection: new Set([clicked]), anchor: clicked, focus: clicked };
}

export function rangeBetween(order: string[], a: string, b: string): string[] {
  const i = order.indexOf(a);
  const j = order.indexOf(b);
  if (i < 0 || j < 0) return [b];
  const [lo, hi] = i <= j ? [i, j] : [j, i];
  return order.slice(lo, hi + 1);
}

/** Arrow-key movement. `delta` is ±1 (list / horizontal) or ±columns (grid vertical). */
export function moveFocus(order: string[], cur: SelState, delta: number, extend: boolean): SelState {
  if (!order.length) return cur;
  const from = cur.focus ? order.indexOf(cur.focus) : -1;
  let idx: number;
  if (from < 0) idx = delta > 0 ? 0 : order.length - 1;
  else idx = Math.max(0, Math.min(order.length - 1, from + delta));
  const target = order[idx];
  if (extend) {
    const anchor = cur.anchor && order.includes(cur.anchor) ? cur.anchor : (cur.focus ?? target);
    return { selection: new Set(rangeBetween(order, anchor, target)), anchor, focus: target };
  }
  return { selection: new Set([target]), anchor: target, focus: target };
}

export function jumpTo(order: string[], cur: SelState, idx: number, extend: boolean): SelState {
  if (!order.length) return cur;
  const target = order[Math.max(0, Math.min(order.length - 1, idx))];
  return moveFocus(order, { ...cur, focus: target }, 0, extend);
}

/** Type-ahead: first name (after the focused one when repeating a letter) starting with `prefix`. */
export function typeAhead(names: string[], prefix: string, startAt: number): number {
  const p = prefix.toLowerCase();
  const n = names.length;
  // A single repeated letter cycles through matches.
  const cycling = p.length > 1 && [...p].every((c) => c === p[0]);
  const needle = cycling ? p[0] : p;
  const offset = cycling || p.length === 1 ? 1 : 0;
  for (let k = 0; k < n; k++) {
    const i = (startAt + offset + k + n) % n;
    if (names[i].toLowerCase().startsWith(needle)) return i;
  }
  return -1;
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export function normRect(x1: number, y1: number, x2: number, y2: number): Rect {
  return { x: Math.min(x1, x2), y: Math.min(y1, y2), w: Math.abs(x2 - x1), h: Math.abs(y2 - y1) };
}

const intersects = (a: Rect, b: Rect) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

/** Indices of items hit by a rubber-band rectangle in a list layout (content coordinates). */
export function hitList(r: Rect, count: number, rowH: number, top: number, width: number): number[] {
  if (r.x > width) return [];
  const first = Math.max(0, Math.floor((r.y - top) / rowH));
  const last = Math.min(count - 1, Math.floor((r.y + r.h - top) / rowH));
  const out: number[] = [];
  for (let i = first; i <= last; i++) out.push(i);
  return out;
}

/** Same for a grid of fixed-size cells. */
export function hitGrid(r: Rect, count: number, cols: number, cellW: number, cellH: number, padX: number, padY: number): number[] {
  const out: number[] = [];
  const firstRow = Math.max(0, Math.floor((r.y - padY) / cellH));
  const lastRow = Math.floor((r.y + r.h - padY) / cellH);
  for (let row = firstRow; row <= lastRow; row++) {
    for (let col = 0; col < cols; col++) {
      const i = row * cols + col;
      if (i >= count) return out;
      // Inset the cell a little so grazing an edge doesn't select.
      const cell = { x: padX + col * cellW + 6, y: padY + row * cellH + 4, w: cellW - 12, h: cellH - 8 };
      if (intersects(r, cell)) out.push(i);
    }
  }
  return out;
}
