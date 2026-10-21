import { create } from "zustand";
import { api, errorMessage } from "../lib/api";
import type { Sort } from "../lib/format";
import type { Entry, FsId, Listing, Loc } from "../lib/types";
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

interface TabsState {
  tabs: Tab[];
  active: string;
  newTab: (loc?: Loc, opts?: { background?: boolean; afterId?: string }) => string;
  closeTab: (id: string) => void;
  closeOthers: (id: string) => void;
  activate: (id: string) => void;
  cycle: (delta: number) => void;
  moveTab: (from: number, to: number) => void;
  navigate: (id: string, loc: Loc, opts?: { push?: boolean; select?: string }) => Promise<void>;
  back: (id: string) => void;
  forward: (id: string) => void;
  up: (id: string) => void;
  refresh: (id: string) => Promise<void>;
  refreshWhere: (pred: (t: Tab) => boolean) => void;
  patch: (id: string, patch: Partial<Tab>) => void;
  select: (id: string, paths: string[], opts?: { anchor?: string | null; focus?: string | null }) => void;
}

const SESSION_KEY = "5files.session.v1";
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
    newTab(loc, opts) {
      const target = loc ?? { fs: "local", path: "" };
      const t = blank(target);
      set((s) => {
        const tabs = [...s.tabs];
        const after = opts?.afterId ? tabs.findIndex((x) => x.id === opts.afterId) : -1;
        tabs.splice(after >= 0 ? after + 1 : tabs.length, 0, t);
        return { tabs, active: opts?.background ? s.active || t.id : t.id };
      });
      void load(t.id, target);
      return t.id;
    },
    closeTab(id) {
      const { tabs, active } = get();
      if (tabs.length === 1) {
        // Closing the last tab resets it to home instead of leaving an empty window.
        const t = tabs[0];
        void get().navigate(t.id, { fs: "local", path: "" });
        return;
      }
      const i = tabs.findIndex((t) => t.id === id);
      stopSearch(tabs[i]);
      const next = tabs.filter((t) => t.id !== id);
      set({ tabs: next, active: active === id ? next[Math.min(i, next.length - 1)].id : active });
      reqSeq.delete(id);
    },
    closeOthers(id) {
      get().tabs.filter((t) => t.id !== id).forEach(stopSearch);
      set((s) => ({ tabs: s.tabs.filter((t) => t.id === id), active: id }));
    },
    activate: (id) => set({ active: id }),
    cycle(delta) {
      const { tabs, active } = get();
      const i = tabs.findIndex((t) => t.id === active);
      set({ active: tabs[(i + delta + tabs.length) % tabs.length].id });
    },
    moveTab(from, to) {
      set((s) => {
        const tabs = [...s.tabs];
        const [t] = tabs.splice(from, 1);
        tabs.splice(to, 0, t);
        return { tabs };
      });
    },
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

export function restoreSession() {
  let saved: { tabs: (Loc & { view?: ViewMode })[]; active: number } | null = null;
  try {
    saved = JSON.parse(localStorage.getItem(SESSION_KEY) || "null");
  } catch {
    saved = null;
  }
  const s = useTabs.getState();
  if (saved?.tabs?.length) {
    const ids = saved.tabs.map((l) => s.newTab({ fs: l.fs, path: l.path }, { background: true }));
    saved.tabs.forEach((l, i) => l.view && s.patch(ids[i], { view: l.view }));
    useTabs.setState({ active: ids[Math.min(saved.active, ids.length - 1)] ?? ids[0] });
  } else {
    s.newTab();
  }
}

let saveTimer: ReturnType<typeof setTimeout> | undefined;
useTabs.subscribe((s) => {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    try {
      localStorage.setItem(
        SESSION_KEY,
        JSON.stringify({ tabs: s.tabs.map((t) => ({ fs: t.fs, path: t.path, view: t.view })), active: s.tabs.findIndex((t) => t.id === s.active) }),
      );
    } catch {
      /* ignore */
    }
  }, 400);
});
