// Tabs ("windows" in tmux terms) and the split-pane layout inside each of them.
// Pane state (location, listing, selection…) lives in the tabs store, keyed by pane id.
import { create } from "zustand";
import { markClosing, neighbor, panesOf, removePane, splitPane, updateSplit, type Direction, type LayoutNode, type Rect, type SplitDir } from "../lib/layout";
import { toast } from "./ui";
import { useTabs } from "./tabs";

export interface Workspace {
  id: string;
  root: LayoutNode;
  /** Focused pane. */
  focus: string;
  /** Pane shown alone (tmux "zoom"), or null. */
  zoom: string | null;
  /** Most recently focused panes first (for "the other pane"). */
  history: string[];
}

interface LayoutState {
  workspaces: Workspace[];
  active: string;
  /** Ctrl+B was pressed and the next key is a pane command. */
  prefix: boolean;
  add: (paneId: string, opts?: { background?: boolean; afterPane?: string }) => void;
  closeWorkspace: (id: string) => void;
  closeOthers: (id: string) => void;
  activate: (id: string) => void;
  cycle: (delta: number) => void;
  reorder: (ids: string[]) => void;
  focusPane: (paneId: string) => void;
  split: (dir: SplitDir) => void;
  requestClose: (paneId: string) => void;
  finishClose: (wsId: string, paneId: string) => void;
  settle: (wsId: string, splitId: string) => void;
  setRatio: (wsId: string, splitId: string, ratio: number) => void;
  move: (dir: Direction) => void;
  next: (delta: number) => void;
  toggleZoom: () => void;
  setPrefix: (on: boolean) => void;
}

let n = 0;
const uid = (p: string) => `${p}${Date.now().toString(36)}${(n++).toString(36)}`;

/** Panes can't get smaller than this when splitting (px). */
const MIN_W = 340;
const MIN_H = 220;

export function workspaceOf(paneId: string): Workspace | undefined {
  return useLayout.getState().workspaces.find((w) => panesOf(w.root).includes(paneId));
}

/** The pane files go to with "Copy/Move to other pane": the last one focused before this. */
export function otherPane(paneId: string): string | null {
  const ws = workspaceOf(paneId);
  if (!ws) return null;
  const live = new Set(panesOf(ws.root));
  return ws.history.find((p) => p !== paneId && live.has(p)) ?? panesOf(ws.root).find((p) => p !== paneId) ?? null;
}

export function paneRects(): Record<string, Rect> {
  const out: Record<string, Rect> = {};
  document.querySelectorAll<HTMLElement>("[data-pane-id]").forEach((el) => {
    const r = el.getBoundingClientRect();
    out[el.dataset.paneId!] = { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
  });
  return out;
}

export const useLayout = create<LayoutState>((set, get) => {
  const updateWs = (id: string, f: (w: Workspace) => Workspace) => set((s) => ({ workspaces: s.workspaces.map((w) => (w.id === id ? f(w) : w)) }));
  const current = () => get().workspaces.find((w) => w.id === get().active);
  /** Keeps the tabs store's `active` (= focused pane) in sync. */
  const syncActive = () => {
    const ws = current();
    if (ws && useTabs.getState().active !== ws.focus) useTabs.setState({ active: ws.focus });
  };

  return {
    workspaces: [],
    active: "",
    prefix: false,

    add(paneId, opts) {
      const ws: Workspace = { id: uid("w"), root: { kind: "pane", id: paneId }, focus: paneId, zoom: null, history: [paneId] };
      set((s) => {
        const list = [...s.workspaces];
        const after = opts?.afterPane ? list.findIndex((w) => panesOf(w.root).includes(opts.afterPane!)) : -1;
        list.splice(after >= 0 ? after + 1 : list.length, 0, ws);
        return { workspaces: list, active: opts?.background && s.active ? s.active : ws.id };
      });
      syncActive();
    },

    closeWorkspace(id) {
      const s = get();
      const ws = s.workspaces.find((w) => w.id === id);
      if (!ws) return;
      if (s.workspaces.length === 1) {
        // Last tab: keep the window, collapse to one pane at Home.
        const keep = ws.focus;
        useTabs.getState().dropPanes(panesOf(ws.root).filter((p) => p !== keep));
        updateWs(id, (w) => ({ ...w, root: { kind: "pane", id: keep }, zoom: null, history: [keep] }));
        void useTabs.getState().navigate(keep, { fs: "local", path: "" });
        return;
      }
      const i = s.workspaces.findIndex((w) => w.id === id);
      const rest = s.workspaces.filter((w) => w.id !== id);
      set({ workspaces: rest, active: s.active === id ? rest[Math.min(i, rest.length - 1)].id : s.active });
      useTabs.getState().dropPanes(panesOf(ws.root));
      syncActive();
    },

    closeOthers(id) {
      const others = get().workspaces.filter((w) => w.id !== id);
      set((s) => ({ workspaces: s.workspaces.filter((w) => w.id === id), active: id }));
      useTabs.getState().dropPanes(others.flatMap((w) => panesOf(w.root)));
      syncActive();
    },

    activate(id) {
      set({ active: id });
      syncActive();
    },

    cycle(delta) {
      const { workspaces, active } = get();
      const i = workspaces.findIndex((w) => w.id === active);
      set({ active: workspaces[(i + delta + workspaces.length) % workspaces.length].id });
      syncActive();
    },

    reorder(ids) {
      const byId = new Map(get().workspaces.map((w) => [w.id, w]));
      set({ workspaces: ids.map((id) => byId.get(id)!).filter(Boolean) });
    },

    focusPane(paneId) {
      const ws = workspaceOf(paneId);
      if (!ws) return;
      if (ws.focus !== paneId) updateWs(ws.id, (w) => ({ ...w, focus: paneId, history: [paneId, ...w.history.filter((p) => p !== paneId)].slice(0, 16) }));
      if (get().active !== ws.id) set({ active: ws.id });
      syncActive();
    },

    split(dir) {
      const ws = current();
      if (!ws) return;
      const el = document.querySelector<HTMLElement>(`[data-pane-id="${ws.focus}"]`);
      if (el) {
        const r = el.getBoundingClientRect();
        if ((dir === "row" && r.width / 2 < MIN_W) || (dir === "col" && r.height / 2 < MIN_H)) {
          toast({ tone: "info", title: "Not enough room to split this pane", body: "Make the window bigger or close a pane first." });
          return;
        }
      }
      const src = useTabs.getState().tabs.find((t) => t.id === ws.focus);
      // The new pane starts where the current one is, like tmux.
      const pane = useTabs.getState().createPane(src ? { fs: src.fs, path: src.path } : { fs: "local", path: "" }, src?.view);
      updateWs(ws.id, (w) => ({ ...w, zoom: null, root: splitPane(w.root, w.focus, dir, pane, uid("s")) }));
      get().focusPane(pane);
    },

    requestClose(paneId) {
      const ws = workspaceOf(paneId);
      if (!ws) return;
      if (panesOf(ws.root).length === 1) return get().closeWorkspace(ws.id);
      const marked = markClosing(ws.root, paneId);
      if (!marked) return;
      // Hand focus over right away so keyboard input never lands in a closing pane.
      if (ws.focus === paneId) {
        const live = panesOf(ws.root).filter((p) => p !== paneId);
        const next = ws.history.find((p) => live.includes(p)) ?? live[0];
        updateWs(ws.id, (w) => ({ ...w, root: marked, zoom: null, focus: next, history: [next, ...w.history.filter((p) => p !== next && p !== paneId)] }));
      } else {
        updateWs(ws.id, (w) => ({ ...w, root: marked, zoom: null }));
      }
      syncActive();
    },

    finishClose(wsId, paneId) {
      updateWs(wsId, (w) => ({ ...w, root: removePane(w.root, paneId) ?? w.root, history: w.history.filter((p) => p !== paneId) }));
      useTabs.getState().dropPanes([paneId]);
    },

    settle(wsId, splitId) {
      updateWs(wsId, (w) => ({ ...w, root: updateSplit(w.root, splitId, { born: false }) }));
    },

    setRatio(wsId, splitId, ratio) {
      updateWs(wsId, (w) => ({ ...w, root: updateSplit(w.root, splitId, { ratio }) }));
    },

    move(dir) {
      const ws = current();
      if (!ws || ws.zoom) return;
      const target = neighbor(paneRects(), ws.focus, dir);
      if (target) get().focusPane(target);
    },

    next(delta) {
      const ws = current();
      if (!ws) return;
      const all = panesOf(ws.root);
      const i = all.indexOf(ws.focus);
      get().focusPane(all[(i + delta + all.length) % all.length]);
      if (ws.zoom) updateWs(ws.id, (w) => ({ ...w, zoom: w.focus }));
    },

    toggleZoom() {
      const ws = current();
      if (!ws || panesOf(ws.root).length < 2) return;
      updateWs(ws.id, (w) => ({ ...w, zoom: w.zoom ? null : w.focus }));
    },

    setPrefix: (on) => set({ prefix: on }),
  };
});
