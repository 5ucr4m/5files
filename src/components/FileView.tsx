import { useVirtualizer } from "@tanstack/react-virtual";
import { AlertCircle, ArrowDown, ArrowUp, FolderOpen, RotateCw, SearchX } from "lucide-react";
import { motion } from "motion/react";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type DragEvent } from "react";
import { canDrop, dropEffect, endDrag, handleDrop, isInternalDrag, startDrag } from "../lib/dnd";
import { formatBytes, formatDate, kindLabel, sortEntries, stemRange, type SortKey } from "../lib/format";
import { backgroundMenu, itemMenu } from "../lib/menus";
import { commitRename, copySelection, deleteSelection, newFolder, openEntry, paste, selectedEntries, transfer } from "../lib/ops";
import { isMac, isMod } from "../lib/platform";
import { clickSelect, hitGrid, hitList, jumpTo, moveFocus, normRect, typeAhead } from "../lib/selection";
import type { Entry } from "../lib/types";
import { useSettings } from "../store/settings";
import { useTabs, type Tab } from "../store/tabs";
import { useUi } from "../store/ui";
import { FileIcon } from "./FileIcon";

const HEADER_H = 30;
const GRID_PAD = 12;

export function visibleEntries(tab: Tab, showHidden: boolean, foldersFirst: boolean): Entry[] {
  const base = tab.search ? tab.search.results : (tab.listing?.entries ?? []);
  const f = tab.filter.trim().toLowerCase();
  const list = base.filter((e) => (showHidden || !e.hidden) && (!f || e.name.toLowerCase().includes(f)));
  return sortEntries(list, tab.sort, foldersFirst);
}

function RenameInput({ tab, entry, className }: { tab: Tab; entry: Entry; className?: string }) {
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useLayoutEffect(() => {
    const el = ref.current!;
    el.focus();
    const [a, b] = stemRange(entry.name, entry.isDir);
    el.setSelectionRange(a, b);
  }, [entry.name, entry.isDir]);
  const finish = (commit: boolean) => {
    if (done.current) return;
    done.current = true;
    if (commit) void commitRename(tab, entry, ref.current!.value);
    else useTabs.getState().patch(tab.id, { renaming: null });
    window.dispatchEvent(new Event("atlas:focus-list"));
  };
  return (
    <input
      ref={ref}
      defaultValue={entry.name}
      spellCheck={false}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") finish(true);
        if (e.key === "Escape") finish(false);
      }}
      onBlur={() => finish(true)}
      onClick={(e) => e.stopPropagation()}
      onMouseDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      className={`rounded-[5px] border border-accent bg-surface px-1 text-ink shadow-[0_0_0_3px_var(--focus)] outline-none ${className ?? ""}`}
    />
  );
}

interface ItemHandlers {
  onMouseDown: (e: React.MouseEvent, entry: Entry) => void;
  onClick: (e: React.MouseEvent, entry: Entry) => void;
  onDoubleClick: (e: React.MouseEvent, entry: Entry) => void;
  onContextMenu: (e: React.MouseEvent, entry: Entry) => void;
  onDragStart: (e: DragEvent, entry: Entry) => void;
}

function useDropTarget(tab: Tab, entry: Entry) {
  const [over, setOver] = useState(false);
  if (!entry.isDir) return { over: false, props: {} };
  const dest = { fs: tab.fs, path: entry.path };
  return {
    over,
    props: {
      onDragOver: (e: DragEvent) => {
        if (!isInternalDrag(e) || !canDrop(dest) || tab.selection.has(entry.path)) return;
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = dropEffect(e, dest);
        setOver(true);
      },
      onDragLeave: () => setOver(false),
      onDrop: (e: DragEvent) => {
        e.stopPropagation();
        setOver(false);
        void handleDrop(e, dest);
      },
    },
  };
}

const Row = memo(function Row({
  tab,
  entry,
  selected,
  focused,
  cut,
  renaming,
  h,
  showPath,
  thumbs,
  handlers,
}: {
  tab: Tab;
  entry: Entry;
  selected: boolean;
  focused: boolean;
  cut: boolean;
  renaming: boolean;
  h: number;
  showPath: boolean;
  thumbs: boolean;
  handlers: ItemHandlers;
}) {
  const drop = useDropTarget(tab, entry);
  return (
    <div
      role="row"
      aria-selected={selected}
      draggable={!renaming}
      onDragStart={(e) => handlers.onDragStart(e, entry)}
      onDragEnd={endDrag}
      onMouseDown={(e) => handlers.onMouseDown(e, entry)}
      onClick={(e) => handlers.onClick(e, entry)}
      onDoubleClick={(e) => handlers.onDoubleClick(e, entry)}
      onContextMenu={(e) => handlers.onContextMenu(e, entry)}
      {...drop.props}
      className={`grid items-center gap-3 rounded-[7px] pr-3 pl-2.5 text-[13px] ${
        drop.over ? "bg-sel-strong ring-1 ring-accent" : selected ? "bg-sel" : "hover:bg-surface-2"
      } ${focused && selected ? "ring-1 ring-[color-mix(in_oklab,var(--accent)_45%,transparent)] ring-inset" : ""} ${cut ? "opacity-45" : ""}`}
      style={{ height: h, gridTemplateColumns: "minmax(200px,1fr) 168px 92px 132px" }}
    >
      <div className="flex min-w-0 items-center gap-2.5">
        <FileIcon entry={entry} size={h >= 30 ? 20 : 17} thumbs={thumbs} open={drop.over} />
        {renaming ? (
          <RenameInput tab={tab} entry={entry} className="h-[22px] min-w-0 flex-1" />
        ) : (
          <div className="min-w-0 flex-1">
            <div className={`truncate ${entry.hidden ? "text-ink-3" : ""}`}>
              {entry.name}
              {entry.isSymlink && <span className="ml-1.5 text-2xs text-ink-4">alias</span>}
            </div>
            {showPath && <div className="num truncate text-[10.5px] leading-tight text-ink-4">{entry.path}</div>}
          </div>
        )}
      </div>
      <div className="num truncate text-[12px] text-ink-3">{formatDate(entry.modified)}</div>
      <div className="num truncate text-right text-[12px] text-ink-3">{entry.isDir ? "—" : formatBytes(entry.size)}</div>
      <div className="truncate text-[12px] text-ink-3">{kindLabel(entry)}</div>
    </div>
  );
});

const Cell = memo(function Cell({
  tab,
  entry,
  selected,
  cut,
  renaming,
  size,
  thumbs,
  handlers,
}: {
  tab: Tab;
  entry: Entry;
  selected: boolean;
  cut: boolean;
  renaming: boolean;
  size: number;
  thumbs: boolean;
  handlers: ItemHandlers;
}) {
  const drop = useDropTarget(tab, entry);
  return (
    <div
      role="gridcell"
      aria-selected={selected}
      draggable={!renaming}
      onDragStart={(e) => handlers.onDragStart(e, entry)}
      onDragEnd={endDrag}
      onMouseDown={(e) => handlers.onMouseDown(e, entry)}
      onClick={(e) => handlers.onClick(e, entry)}
      onDoubleClick={(e) => handlers.onDoubleClick(e, entry)}
      onContextMenu={(e) => handlers.onContextMenu(e, entry)}
      {...drop.props}
      className={`flex h-full flex-col items-center gap-1.5 px-1.5 pt-2 ${cut ? "opacity-45" : ""}`}
      title={entry.name}
    >
      <div
        className={`flex items-center justify-center rounded-[10px] transition-colors ${drop.over ? "bg-sel-strong ring-1 ring-accent" : selected ? "bg-sel" : ""}`}
        style={{ width: size + 12, height: size + 8 }}
      >
        <FileIcon entry={entry} size={size} thumbs={thumbs} open={drop.over} />
      </div>
      {renaming ? (
        <RenameInput tab={tab} entry={entry} className="w-full text-center text-[12px]" />
      ) : (
        <span
          className={`line-clamp-2 max-w-full rounded-[5px] px-1.5 py-px text-center text-[12px] leading-[1.3] break-words ${selected ? "bg-accent text-white" : entry.hidden ? "text-ink-3" : ""}`}
        >
          {entry.name}
        </span>
      )}
    </div>
  );
});

function SortHeader({ tab }: { tab: Tab }) {
  const cols: { key: SortKey; label: string; right?: boolean }[] = [
    { key: "name", label: "Name" },
    { key: "modified", label: "Date modified" },
    { key: "size", label: "Size", right: true },
    { key: "kind", label: "Kind" },
  ];
  return (
    <div
      className="sticky top-0 z-10 grid items-center gap-3 border-b border-line bg-surface/90 pr-6 pl-5 text-[11.5px] font-medium text-ink-3 backdrop-blur"
      style={{ height: HEADER_H, gridTemplateColumns: "minmax(200px,1fr) 168px 92px 132px" }}
    >
      {cols.map((c) => {
        const active = tab.sort.key === c.key;
        return (
          <button
            key={c.key}
            onClick={() =>
              useTabs.getState().patch(tab.id, {
                sort: { key: c.key, dir: active ? (tab.sort.dir === "asc" ? "desc" : "asc") : c.key === "name" || c.key === "kind" ? "asc" : "desc" },
              })
            }
            className={`flex items-center gap-1 truncate hover:text-ink ${c.right ? "justify-end" : ""} ${active ? "text-ink-2" : ""}`}
          >
            {c.label}
            {active && (tab.sort.dir === "asc" ? <ArrowUp size={11} /> : <ArrowDown size={11} />)}
          </button>
        );
      })}
    </div>
  );
}

function EmptyState({ tab }: { tab: Tab }) {
  if (tab.error)
    return (
      <div className="flex h-full flex-col items-center justify-center p-8 text-center">
        <div className="mb-3 flex size-12 items-center justify-center rounded-2xl bg-[color-mix(in_oklab,var(--danger)_12%,transparent)] text-danger">
          <AlertCircle size={22} />
        </div>
        <p className="font-medium">Can't open this folder</p>
        <p className="mt-1 max-w-md text-2xs text-ink-3">{tab.error}</p>
        <button onClick={() => useTabs.getState().refresh(tab.id)} className="mt-4 flex items-center gap-1.5 rounded-lg bg-surface-3 px-3 py-1.5 text-[12px] font-medium hover:bg-line">
          <RotateCw size={13} /> Try again
        </button>
      </div>
    );
  if (tab.search)
    return (
      <div className="flex h-full flex-col items-center justify-center text-center text-ink-3">
        {tab.search.running ? (
          <p className="num text-2xs">Searching… {tab.search.scanned.toLocaleString()} items checked</p>
        ) : (
          <>
            <SearchX size={26} className="mb-2 text-ink-4" />
            <p className="font-medium text-ink-2">No matches for “{tab.search.query}”</p>
            <p className="mt-1 text-2xs">Searched this folder and everything inside it. Try fewer words or a pattern like *.pdf</p>
          </>
        )}
      </div>
    );
  if (tab.loading) return null;
  return (
    <div className="flex h-full flex-col items-center justify-center text-center text-ink-3">
      <motion.div initial={{ scale: 0.9, opacity: 0 }} animate={{ scale: 1, opacity: 1 }} className="mb-3 flex size-14 items-center justify-center rounded-2xl bg-surface-2">
        <FolderOpen size={24} className="text-ink-4" />
      </motion.div>
      <p className="font-medium text-ink-2">{tab.filter ? `Nothing here matches “${tab.filter}”` : "This folder is empty"}</p>
      {!tab.filter && <p className="mt-1 text-2xs">Drop files here, or create a folder with {isMac() ? "⇧⌘N" : "Ctrl+Shift+N"}.</p>}
    </div>
  );
}

export function FileView({ tab }: { tab: Tab }) {
  const showHidden = useSettings((s) => s.showHidden);
  const foldersFirst = useSettings((s) => s.foldersFirst);
  const density = useSettings((s) => s.density);
  const gridSize = useSettings((s) => s.gridSize);
  const clipboard = useUi((s) => s.clipboard);
  const entries = useMemo(() => visibleEntries(tab, showHidden, foldersFirst), [tab.listing, tab.search, tab.filter, tab.sort, showHidden, foldersFirst]); // eslint-disable-line react-hooks/exhaustive-deps
  const order = useMemo(() => entries.map((e) => e.path), [entries]);
  const byPath = useMemo(() => new Map(entries.map((e) => [e.path, e])), [entries]);
  const cutSet = useMemo(() => new Set(clipboard?.mode === "cut" ? clipboard.items.filter((i) => i.fs === tab.fs).map((i) => i.path) : []), [clipboard, tab.fs]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(800);
  useLayoutEffect(() => {
    const el = scrollRef.current!;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  const isGrid = tab.view === "grid";
  const rowH = density === "compact" ? 26 : 32;
  const listRowH = rowH + (tab.search ? 12 : 0);
  const cellW = gridSize + 28;
  const cellH = gridSize + 52;
  const cols = isGrid ? Math.max(1, Math.floor((width - GRID_PAD * 2) / cellW)) : 1;
  const rows = isGrid ? Math.ceil(entries.length / cols) : entries.length;
  const thumbs = !!tab.listing?.caps.thumbnails || (tab.fs === "local" && !!tab.search);

  const virt = useVirtualizer({
    count: rows,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => (isGrid ? cellH : listRowH),
    overscan: isGrid ? 3 : 12,
    paddingStart: isGrid ? GRID_PAD : 4,
    paddingEnd: 24,
    // List rows start below the sticky header; keep scrollToIndex clear of it.
    scrollMargin: isGrid || entries.length === 0 ? 0 : HEADER_H,
    scrollPaddingStart: isGrid ? 0 : HEADER_H,
  });
  useEffect(() => virt.measure(), [isGrid, cellH, listRowH, cols, virt]);

  // Reset scroll on navigation; keep the focused item visible otherwise.
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 });
  }, [tab.nav, tab.id]);
  const ensureVisible = useCallback(
    (path: string | null) => {
      if (!path) return;
      const i = order.indexOf(path);
      if (i >= 0) virt.scrollToIndex(isGrid ? Math.floor(i / cols) : i, { align: "auto" });
    },
    [order, virt, isGrid, cols],
  );
  useEffect(() => {
    if (tab.focus) ensureVisible(tab.focus);
  }, [tab.focus]); // eslint-disable-line react-hooks/exhaustive-deps

  const setSel = (s: { selection: Set<string>; anchor: string | null; focus: string | null }) => useTabs.getState().patch(tab.id, s);

  // ---------------- mouse ----------------
  const pressed = useRef<{ path: string; wasSelected: boolean } | null>(null);
  const handlers: ItemHandlers = useMemo(
    () => ({
      onMouseDown(e, entry) {
        if (e.button !== 0) return;
        e.stopPropagation();
        scrollRef.current?.focus({ preventScroll: true });
        const t = useTabs.getState().tabs.find((x) => x.id === tab.id)!;
        const wasSelected = t.selection.has(entry.path);
        pressed.current = { path: entry.path, wasSelected };
        // Keep a multi-selection intact on mousedown so it can be dragged; collapse on click.
        if (!wasSelected || e.shiftKey || isMod(e)) setSel(clickSelect(order, t, entry.path, { shift: e.shiftKey, toggle: isMod(e) }));
      },
      onClick(e, entry) {
        const p = pressed.current;
        pressed.current = null;
        if (!p || p.path !== entry.path || e.shiftKey || isMod(e) || !p.wasSelected) return;
        const t = useTabs.getState().tabs.find((x) => x.id === tab.id)!;
        if (t.selection.size > 1) setSel({ selection: new Set([entry.path]), anchor: entry.path, focus: entry.path });
      },
      onDoubleClick(e, entry) {
        e.stopPropagation();
        const t = useTabs.getState().tabs.find((x) => x.id === tab.id)!;
        void openEntry(t, entry, { newTab: isMod(e) && entry.isDir });
      },
      onContextMenu(e, entry) {
        e.preventDefault();
        e.stopPropagation();
        let t = useTabs.getState().tabs.find((x) => x.id === tab.id)!;
        if (!t.selection.has(entry.path)) {
          setSel({ selection: new Set([entry.path]), anchor: entry.path, focus: entry.path });
          t = useTabs.getState().tabs.find((x) => x.id === tab.id)!;
        }
        useUi.getState().openMenu(e.clientX, e.clientY, itemMenu(t, entry));
      },
      onDragStart(e, entry) {
        const t = useTabs.getState().tabs.find((x) => x.id === tab.id)!;
        const paths = t.selection.has(entry.path) ? [...t.selection] : [entry.path];
        if (!t.selection.has(entry.path)) setSel({ selection: new Set([entry.path]), anchor: entry.path, focus: entry.path });
        startDrag(
          e,
          paths.map((path) => ({ fs: t.fs, path })),
          paths.length === 1 ? entry.name : `${paths.length} items`,
        );
      },
    }),
    [order, tab.id], // eslint-disable-line react-hooks/exhaustive-deps
  );

  // Rubber-band selection on empty space.
  const [band, setBand] = useState<{ x1: number; y1: number; x2: number; y2: number } | null>(null);
  const bandBase = useRef<Set<string>>(new Set());
  const onBackgroundDown = (e: React.MouseEvent) => {
    if (e.button !== 0 || e.target !== e.currentTarget && !(e.target as HTMLElement).dataset.bg) return;
    const el = scrollRef.current!;
    el.focus({ preventScroll: true });
    const r = el.getBoundingClientRect();
    const toContent = (cx: number, cy: number) => ({ x: cx - r.left + el.scrollLeft, y: cy - r.top + el.scrollTop });
    const start = toContent(e.clientX, e.clientY);
    const additive = e.shiftKey || isMod(e);
    bandBase.current = additive ? new Set(tab.selection) : new Set();
    if (!additive) setSel({ selection: new Set(), anchor: null, focus: null });
    let moved = false;
    const move = (ev: MouseEvent) => {
      const p = toContent(ev.clientX, ev.clientY);
      if (!moved && Math.hypot(p.x - start.x, p.y - start.y) < 4) return;
      moved = true;
      // Auto-scroll near edges.
      if (ev.clientY > r.bottom - 24) el.scrollTop += 14;
      else if (ev.clientY < r.top + 24) el.scrollTop -= 14;
      setBand({ x1: start.x, y1: start.y, x2: p.x, y2: p.y });
      const rect = normRect(start.x, start.y, p.x, p.y);
      const hits = isGrid ? hitGrid(rect, entries.length, cols, cellW, cellH, GRID_PAD + (width - GRID_PAD * 2 - cols * cellW) / 2, GRID_PAD) : hitList(rect, entries.length, listRowH, HEADER_H + 4, width - 8);
      const sel = new Set(bandBase.current);
      hits.forEach((i) => sel.add(order[i]));
      setSel({ selection: sel, anchor: order[hits[0]] ?? null, focus: order[hits[hits.length - 1]] ?? null });
    };
    const up = () => {
      setBand(null);
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  // ---------------- keyboard ----------------
  const typed = useRef({ text: "", at: 0 });
  const onKeyDown = (e: React.KeyboardEvent | KeyboardEvent) => {
    const t = useTabs.getState().tabs.find((x) => x.id === tab.id)!;
    if (t.renaming) return;
    const mod = isMod(e);
    const sel = selectedEntries(t);
    const focusEntry = t.focus ? byPath.get(t.focus) : sel[0];
    const k = e.key;
    const handled = () => {
      e.preventDefault();
      e.stopPropagation();
    };
    if (k === "ArrowDown" && mod && focusEntry) return handled(), void openEntry(t, focusEntry);
    if (k === "ArrowUp" && mod) return handled(), useTabs.getState().up(t.id);
    if (k === "ArrowDown" || k === "ArrowUp") return handled(), setSel(moveFocus(order, t, (k === "ArrowDown" ? 1 : -1) * cols, e.shiftKey));
    if (isGrid && (k === "ArrowRight" || k === "ArrowLeft")) return handled(), setSel(moveFocus(order, t, k === "ArrowRight" ? 1 : -1, e.shiftKey));
    if (k === "Home") return handled(), setSel(jumpTo(order, t, 0, e.shiftKey));
    if (k === "End") return handled(), setSel(jumpTo(order, t, order.length - 1, e.shiftKey));
    if (k === "PageDown" || k === "PageUp") {
      const page = Math.max(1, Math.floor((scrollRef.current!.clientHeight - HEADER_H) / (isGrid ? cellH : listRowH))) * cols;
      return handled(), setSel(moveFocus(order, t, k === "PageDown" ? page : -page, e.shiftKey));
    }
    if (k === "Enter" && focusEntry) return handled(), void openEntry(t, focusEntry, { newTab: mod });
    if (k === "F2" && focusEntry) return handled(), useTabs.getState().patch(t.id, { renaming: focusEntry.path });
    if (k === " " && focusEntry && Date.now() - typed.current.at > 700) return handled(), useUi.getState().set({ quickLook: { fs: t.fs, entry: focusEntry } });
    if ((k === "Delete" || (k === "Backspace" && mod)) && sel.length) return handled(), void deleteSelection(e.shiftKey || e.altKey);
    if (k === "Backspace" && !mod && !isMac()) return handled(), useTabs.getState().back(t.id);
    if (mod && k.toLowerCase() === "a") return handled(), setSel({ selection: new Set(order), anchor: order[0] ?? null, focus: t.focus });
    if (mod && k.toLowerCase() === "c" && !e.altKey) return handled(), copySelection(false);
    if (mod && e.altKey && (k.toLowerCase() === "c" || e.code === "KeyC")) return handled(), void navigator.clipboard?.writeText(sel.map((x) => x.path).join("\n"));
    if (mod && k.toLowerCase() === "x") return handled(), copySelection(true);
    if (mod && k.toLowerCase() === "v") return handled(), void paste();
    if (mod && k.toLowerCase() === "d" && sel.length) return handled(), void transfer("copy", sel.map((x) => ({ fs: t.fs, path: x.path })), { fs: t.fs, path: t.path });
    if (mod && e.shiftKey && k.toLowerCase() === "n") return handled(), void newFolder();
    if (k === "Escape") return handled(), setSel({ selection: new Set(), anchor: null, focus: null });
    if (k === "ContextMenu" && focusEntry) {
      handled();
      const el = scrollRef.current!.querySelector('[aria-selected="true"]') as HTMLElement | null;
      const r = (el ?? scrollRef.current!).getBoundingClientRect();
      return useUi.getState().openMenu(r.left + 40, r.top + r.height / 2, itemMenu(t, focusEntry));
    }
    // Type-ahead.
    if (k.length === 1 && !mod && !e.altKey && /\S/.test(k)) {
      handled();
      const now = Date.now();
      typed.current = { text: now - typed.current.at < 700 ? typed.current.text + k : k, at: now };
      const start = t.focus ? Math.max(0, order.indexOf(t.focus)) : 0;
      const i = typeAhead(entries.map((x) => x.name), typed.current.text, start);
      if (i >= 0) setSel({ selection: new Set([order[i]]), anchor: order[i], focus: order[i] });
    }
  };
  // Route file shortcuts here even when focus sits on a sidebar/toolbar button.
  const keyRef = useRef(onKeyDown);
  keyRef.current = onKeyDown;
  useEffect(() => {
    const onWinKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return;
      const target = e.target as HTMLElement;
      if (scrollRef.current?.contains(target)) return; // React handler already ran
      if (target.closest("input, textarea, select, [contenteditable], [role=dialog], [role=menu]")) return;
      const ui = useUi.getState();
      if (ui.palette || ui.menu || ui.quickLook || ui.confirm || ui.conflict || ui.panel) return;
      keyRef.current(e);
      // Move focus into the list so the button doesn't also activate on keyup.
      if (e.defaultPrevented && !document.querySelector("[role=dialog], [role=menu]")) scrollRef.current?.focus({ preventScroll: true });
    };
    window.addEventListener("keydown", onWinKey);
    return () => window.removeEventListener("keydown", onWinKey);
  }, []);
  useEffect(() => {
    const focus = () => {
      scrollRef.current?.focus({ preventScroll: true });
      const t = useTabs.getState().tabs.find((x) => x.id === tab.id);
      if (t && !t.selection.size && order[0]) setSel({ selection: new Set([order[0]]), anchor: order[0], focus: order[0] });
    };
    window.addEventListener("atlas:focus-list", focus);
    return () => window.removeEventListener("atlas:focus-list", focus);
  }, [order, tab.id]); // eslint-disable-line react-hooks/exhaustive-deps
  // Focus the list when a tab is shown so arrow keys work immediately.
  useEffect(() => {
    if (!document.activeElement || document.activeElement === document.body) scrollRef.current?.focus({ preventScroll: true });
  }, [tab.id]);

  // ---------------- drop onto the folder background ----------------
  const [over, setOver] = useState(false);
  const here = { fs: tab.fs, path: tab.path };
  const dropProps = tab.search
    ? {}
    : {
        onDragOver: (e: DragEvent) => {
          if (!isInternalDrag(e) || !canDrop(here)) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = dropEffect(e, here);
          setOver(true);
        },
        onDragLeave: (e: DragEvent) => {
          if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node)) setOver(false);
        },
        onDrop: (e: DragEvent) => {
          setOver(false);
          void handleDrop(e, here);
        },
      };

  const items = virt.getVirtualItems();
  const offset = isGrid ? (width - GRID_PAD * 2 - cols * cellW) / 2 : 0;

  return (
    <div
      ref={scrollRef}
      tabIndex={0}
      onKeyDown={onKeyDown}
      onMouseDown={onBackgroundDown}
      onContextMenu={(e) => {
        e.preventDefault();
        setSel({ selection: new Set(), anchor: null, focus: null });
        useUi.getState().openMenu(e.clientX, e.clientY, backgroundMenu(useTabs.getState().tabs.find((x) => x.id === tab.id)!));
      }}
      {...dropProps}
      data-drop-dir={tab.search ? undefined : tab.path}
      className={`scroll relative h-full overflow-y-auto outline-none ${over ? "bg-[color-mix(in_oklab,var(--accent)_5%,var(--surface))] shadow-[inset_0_0_0_2px_var(--accent)]" : ""}`}
      role={isGrid ? "grid" : "table"}
      aria-label="Files"
    >
      {!isGrid && entries.length > 0 && <SortHeader tab={tab} />}
      <motion.div
        key={`${tab.id}:${tab.nav}:${tab.view}`}
        initial={{ opacity: 0, y: 6 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.18, ease: [0.2, 0.8, 0.2, 1] }}
        data-bg="1"
        className="relative w-full"
        style={{ height: virt.getTotalSize() }}
        onMouseDown={onBackgroundDown}
      >
        {items.map((vi) => {
          if (!isGrid) {
            const entry = entries[vi.index];
            return (
              <div key={entry.path} className="absolute right-2 left-2" style={{ top: vi.start - virt.options.scrollMargin, height: vi.size }}>
                <Row
                  tab={tab}
                  entry={entry}
                  selected={tab.selection.has(entry.path)}
                  focused={tab.focus === entry.path}
                  cut={cutSet.has(entry.path)}
                  renaming={tab.renaming === entry.path}
                  h={listRowH}
                  showPath={!!tab.search}
                  thumbs={thumbs}
                  handlers={handlers}
                />
              </div>
            );
          }
          const slice = entries.slice(vi.index * cols, vi.index * cols + cols);
          return (
            <div key={vi.key} className="absolute flex" style={{ top: vi.start, left: GRID_PAD + offset, height: cellH }}>
              {slice.map((entry) => (
                <div key={entry.path} style={{ width: cellW, height: cellH }}>
                  <Cell
                    tab={tab}
                    entry={entry}
                    selected={tab.selection.has(entry.path)}
                    cut={cutSet.has(entry.path)}
                    renaming={tab.renaming === entry.path}
                    size={gridSize - 16}
                    thumbs={thumbs}
                    handlers={handlers}
                  />
                </div>
              ))}
            </div>
          );
        })}
      </motion.div>
      {entries.length === 0 && (
        <div className="pointer-events-none absolute inset-0 [&_button]:pointer-events-auto">
          <EmptyState tab={tab} />
        </div>
      )}
      {band && (
        <div
          className="pointer-events-none absolute z-20 rounded-[3px] border border-accent bg-[color-mix(in_oklab,var(--accent)_14%,transparent)]"
          style={{ left: Math.min(band.x1, band.x2), top: Math.min(band.y1, band.y2), width: Math.abs(band.x2 - band.x1), height: Math.abs(band.y2 - band.y1) }}
        />
      )}
    </div>
  );
}
