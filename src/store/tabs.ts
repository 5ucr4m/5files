import { create } from "zustand";
import { api, errorMessage } from "../lib/api";
import type { Sort } from "../lib/format";
import type { Entry, FsId, Listing, Loc } from "../lib/types";
import type { LayoutNode } from "../lib/layout";
import { mapPanes } from "../lib/layout";
import { useLayout, workspaceOf } from "./layout";
import { useSettings, type ViewMode } from "./settings";

export interface SearchState {
  /** Unique per search run; events from older runs are ignored. */
  run: number;
  id: string | null;
  query: string;
  results: Entry[];
  running: boolean;
  scanned: number;
  truncated: boolean;
}

export interface Tab {
  id: string;
  fs: FsId;
  path: string;
  listing: Listing | null;
  loading: boolean;
  error: string | null;
  history: Loc[];
  index: number;
  selection: Set<string>;
  anchor: string | null;
  focus: string | null;
  view: ViewMode;
  sort: Sort;
  filter: string;
  search: SearchState | null;
  renaming: string | null;
  /** Bumped on every navigation so views can reset scroll & animate. */
  nav: number;
}

/**
 * Pane state. Each "tab" in the title bar is a workspace (see store/layout) holding one
 * or more panes; `Tab` here is one pane, and `active` is the focused pane.
 */
interface TabsState {
  tabs: Tab[];
  active: string;
  /** Opens a new title-bar tab with one pane; returns the pane id. */
  newTab: (loc?: Loc, opts?: { background?: boolean; afterId?: string }) => string;
  /** Creates pane state only (the layout decides where it goes). */
  createPane: (loc: Loc, view?: ViewMode) => string;
  /** Forgets panes that were closed. */
  dropPanes: (ids: string[]) => void;
  /** Closes a pane (or its tab, when it is the only pane). */
  closeTab: (id: string) => void;
  closeOthers: (id: string) => void;
  activate: (id: string) => void;
  cycle: (delta: number) => void;
  navigate: (id: string, loc: Loc, opts?: { push?: boolean; select?: string }) => Promise<void>;
  back: (id: string) => void;
  forward: (id: string) => void;
  up: (id: string) => void;
  refresh: (id: string) => Promise<void>;
  refreshWhere: (pred: (t: Tab) => boolean) => void;
  patch: (id: string, patch: Partial<Tab>) => void;
  select: (id: string, paths: string[], opts?: { anchor?: string | null; focus?: string | null }) => void;
}

const SESSION_KEY = "5files.session.v2";
let counter = 0;
const tabId = () => `t${Date.now().toString(36)}${(counter++).toString(36)}`;

function blank(loc: Loc): Tab {
  return {
    id: tabId(),
    fs: loc.fs,
    path: loc.path,
    listing: null,
    loading: true,
    error: null,
    history: [loc],
    index: 0,
    selection: new Set(),
    anchor: null,
    focus: null,
    view: useSettings.getState().defaultView,
    sort: { key: "name", dir: "asc" },
    filter: "",
    search: null,
    renaming: null,
    nav: 0,
  };
}

/** Stops a tab's running search in the backend (if any). */
function stopSearch(t: Tab | undefined) {
  if (t?.search?.id && t.search.running) void api().cancelSearch(t.search.id).catch(() => {});
}

/** Race guard: only the latest request per tab may write its result. */
const reqSeq = new Map<string, number>();

export const useTabs = create<TabsState>((set, get) => {
  const update = (id: string, fn: (t: Tab) => Partial<Tab>) =>
    set((s) => ({ tabs: s.tabs.map((t) => (t.id === id ? { ...t, ...fn(t) } : t)) }));

  async function load(id: string, loc: Loc, select?: string, keepSelection = false) {
    const seq = (reqSeq.get(id) ?? 0) + 1;
    reqSeq.set(id, seq);
    update(id, () => ({ loading: true }));
    try {
      const listing = await api().listDir(loc.fs, loc.path || null);
      if (reqSeq.get(id) !== seq) return;
      update(id, (t) => {
        const paths = new Set(listing.entries.map((e) => e.path));
        const selection = keepSelection ? new Set([...t.selection].filter((p) => paths.has(p))) : select ? new Set([select]) : new Set<string>();
        return {
          listing,
          path: listing.path,
          loading: false,
          error: null,
          selection,
          focus: select ?? (keepSelection && t.focus && paths.has(t.focus) ? t.focus : null),
          anchor: select ?? (keepSelection ? t.anchor : null),
          // Fix up history entry when the backend resolved an empty path to the root.
          history: t.history.map((h, i) => (i === t.index ? { fs: listing.fs, path: listing.path } : h)),
        };
      });
    } catch (e) {
      if (reqSeq.get(id) !== seq) return;
      update(id, () => ({ loading: false, error: errorMessage(e), listing: null }));
    }
  }

  return {
    tabs: [],
    active: "",
    createPane(loc, view) {
      const t = { ...blank(loc), ...(view ? { view } : {}) };
      set((s) => ({ tabs: [...s.tabs, t] }));
      void load(t.id, loc);
      return t.id;
    },
    newTab(loc, opts) {
      const id = get().createPane(loc ?? { fs: "local", path: "" });
      useLayout.getState().add(id, { background: opts?.background, afterPane: opts?.afterId });
      return id;
    },
    dropPanes(ids) {
      const gone = new Set(ids);
      get().tabs.filter((t) => gone.has(t.id)).forEach(stopSearch);
      ids.forEach((id) => reqSeq.delete(id));
      set((s) => ({ tabs: s.tabs.filter((t) => !gone.has(t.id)) }));
    },
    closeTab(id) {
      useLayout.getState().requestClose(id);
    },
    closeOthers(id) {
      const ws = workspaceOf(id);
      if (ws) useLayout.getState().closeOthers(ws.id);
    },
    activate: (id) => useLayout.getState().focusPane(id),
    cycle: (delta) => useLayout.getState().cycle(delta),
    async navigate(id, loc, opts) {
      const push = opts?.push ?? true;
      stopSearch(get().tabs.find((t) => t.id === id));
      update(id, (t) => {
        const history = push ? [...t.history.slice(0, t.index + 1), loc] : t.history;
        return {
          fs: loc.fs,
          path: loc.path,
          history,
          index: push ? history.length - 1 : t.index,
          filter: "",
          search: null,
          renaming: null,
          error: null,
          nav: t.nav + 1,
        };
      });
      await load(id, loc, opts?.select);
    },
    back(id) {
      const t = get().tabs.find((x) => x.id === id);
      if (!t || t.index === 0) return;
      const prev = t.history[t.index];
      update(id, () => ({ index: t.index - 1 }));
      // Re-select the folder we came from, like Finder does.
      void get().navigate(id, t.history[t.index - 1], { push: false, select: prev.path });
    },
    forward(id) {
      const t = get().tabs.find((x) => x.id === id);
      if (!t || t.index >= t.history.length - 1) return;
      update(id, () => ({ index: t.index + 1 }));
      void get().navigate(id, t.history[t.index + 1], { push: false });
    },
    up(id) {
      const t = get().tabs.find((x) => x.id === id);
      if (!t?.listing?.parent) return;
      void get().navigate(id, { fs: t.fs, path: t.listing.parent }, { select: t.path });
    },
    refresh: async (id) => {
      const t = get().tabs.find((x) => x.id === id);
      if (t) await load(id, { fs: t.fs, path: t.path }, undefined, true);
    },
    refreshWhere(pred) {
      for (const t of get().tabs) if (pred(t)) void load(t.id, { fs: t.fs, path: t.path }, undefined, true);
    },
    patch: (id, p) => update(id, () => p),
    select(id, paths, opts) {
      update(id, (t) => ({
        selection: new Set(paths),
        anchor: opts?.anchor !== undefined ? opts.anchor : (paths[paths.length - 1] ?? t.anchor),
        focus: opts?.focus !== undefined ? opts.focus : (paths[paths.length - 1] ?? t.focus),
      }));
    },
  };
});

export function useActiveTab(): Tab | undefined {
  return useTabs((s) => s.tabs.find((t) => t.id === s.active));
}

export function activeTab(): Tab | undefined {
  const s = useTabs.getState();
  return s.tabs.find((t) => t.id === s.active);
}

interface SavedPane extends Loc {
  view?: ViewMode;
}
interface SavedSession {
  v: 2;
  workspaces: { root: LayoutNode; focus: string }[];
  panes: Record<string, SavedPane>;
  active: number;
}

export function restoreSession() {
  // Subscribed here (not at module load): tabs and layout import each other.
  useTabs.subscribe(saveSession);
  useLayout.subscribe(saveSession);
  let saved: SavedSession | null = null;
  try {
    const raw = JSON.parse(localStorage.getItem(SESSION_KEY) || "null");
    saved = raw?.v === 2 ? raw : null;
  } catch {
    saved = null;
  }
  const s = useTabs.getState();
  if (!saved?.workspaces?.length) {
    s.newTab();
    return;
  }
  const layout = useLayout.getState();
  const ids: string[] = [];
  for (const w of saved.workspaces) {
    const remap = new Map<string, string>();
    const root = mapPanes(w.root, (old) => {
      const p = saved!.panes[old] ?? { fs: "local", path: "" };
      const id = s.createPane({ fs: p.fs, path: p.path }, p.view);
      remap.set(old, id);
      return id;
    });
    const first = remap.get(w.focus) ?? [...remap.values()][0];
    layout.add(first, { background: true });
    const ws = useLayout.getState().workspaces.at(-1)!;
    useLayout.setState((st) => ({
      workspaces: st.workspaces.map((x) => (x.id === ws.id ? { ...x, root, focus: first, history: [first] } : x)),
    }));
    ids.push(ws.id);
  }
  layout.activate(ids[Math.min(saved.active, ids.length - 1)] ?? ids[0]);
}

let saveTimer: ReturnType<typeof setTimeout> | undefined;
function saveSession() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    const { workspaces, active } = useLayout.getState();
    const panes: Record<string, SavedPane> = {};
    for (const t of useTabs.getState().tabs) panes[t.id] = { fs: t.fs, path: t.path, view: t.view };
    const data: SavedSession = {
      v: 2,
      workspaces: workspaces.map((w) => ({ root: w.root, focus: w.focus })),
      panes,
      active: workspaces.findIndex((w) => w.id === active),
    };
    try {
      localStorage.setItem(SESSION_KEY, JSON.stringify(data));
    } catch {
      /* ignore */
    }
  }, 400);
}
