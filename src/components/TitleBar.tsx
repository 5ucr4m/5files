import { Cloud, Columns2, HardDrive, Maximize2, Minus, Plus, Search, Server, Square, X } from "lucide-react";
import { AnimatePresence, Reorder, motion } from "motion/react";
import { api } from "../lib/api";
import { isMac, kbd } from "../lib/platform";
import { useDrives } from "../store/live";
import { panesOf } from "../lib/layout";
import { useLayout, type Workspace } from "../store/layout";
import { useTabs, type Tab } from "../store/tabs";
import { useUi } from "../store/ui";
import { FolderGlyph } from "./FileIcon";
import { IoMonitor } from "./IoMonitor";
import { TransfersButton } from "./Transfers";

function tabTitle(t: Tab, connName?: string) {
  const crumbs = t.listing?.crumbs;
  if (t.search) return `Search “${t.search.query}”`;
  if (crumbs && crumbs.length) {
    const last = crumbs[crumbs.length - 1];
    if (crumbs.length === 1 && t.fs !== "local") return connName ?? last.name;
    return last.name || last.path;
  }
  const seg = t.path.split(/[\\/]/).filter(Boolean).pop();
  return seg ?? (t.fs === "local" ? "Home" : (connName ?? "Drive"));
}

function TabIcon({ t }: { t: Tab }) {
  const conn = useDrives((s) => s.connections.find((c) => c.id === t.fs));
  if (t.search) return <Search size={13} className="text-ink-3" />;
  if (conn) {
    const Icon = conn.kind === "sftp" || conn.kind === "ftp" ? Server : Cloud;
    return <Icon size={13} style={{ color: conn.color ?? "var(--accent)" }} />;
  }
  if (t.listing && t.listing.crumbs.length <= 1) return <HardDrive size={13} className="text-ink-3" />;
  return <FolderGlyph size={15} />;
}

function TabPill({ ws, active }: { ws: Workspace; active: boolean }) {
  const t = useTabs((s) => s.tabs.find((x) => x.id === ws.focus));
  const conn = useDrives((s) => s.connections.find((c) => c.id === t?.fs));
  const panes = panesOf(ws.root).length;
  const L = useLayout.getState();
  if (!t) return null;
  const title = tabTitle(t, conn?.name);
  return (
    <Reorder.Item
      value={ws.id}
      id={ws.id}
      layout="position"
      initial={{ opacity: 0, scale: 0.9, width: 0 }}
      animate={{ opacity: 1, scale: 1, width: "auto" }}
      exit={{ opacity: 0, scale: 0.9, width: 0, transition: { duration: 0.14 } }}
      transition={{ type: "spring", stiffness: 600, damping: 40 }}
      whileDrag={{ scale: 1.03, zIndex: 10 }}
      onPointerDown={() => L.activate(ws.id)}
      onAuxClick={(e) => e.button === 1 && L.closeWorkspace(ws.id)}
      className="no-drag group relative flex h-[30px] max-w-[220px] min-w-[96px] shrink cursor-default items-center"
      title={panes > 1 ? `${panes} panes${ws.zoom ? " (one zoomed)" : ""} · ${t.path}` : t.path}
    >
      {active && (
        <motion.div
          layoutId="tab-active"
          className="absolute inset-0 rounded-[9px] border border-line bg-surface shadow-soft"
          transition={{ type: "spring", stiffness: 520, damping: 38 }}
        />
      )}
      <div className={`relative flex w-full min-w-0 items-center gap-2 rounded-[9px] pr-1.5 pl-2.5 ${active ? "text-ink" : "text-ink-3 hover:bg-chrome-active hover:text-ink-2"}`}>
        <span className="flex size-4 shrink-0 items-center justify-center">
          <TabIcon t={t} />
        </span>
        <span className="min-w-0 flex-1 truncate text-[12.5px] font-medium">{title}</span>
        {panes > 1 && (
          <motion.span
            initial={{ scale: 0.6, opacity: 0 }}
            animate={{ scale: 1, opacity: 1 }}
            className={`num flex h-[16px] shrink-0 items-center gap-0.5 rounded-[5px] px-1 text-[10px] font-semibold ${active ? "bg-sel text-accent" : "bg-surface-3 text-ink-3"}`}
          >
            {ws.zoom ? <Maximize2 size={9} strokeWidth={2.6} /> : <Columns2 size={10} strokeWidth={2.4} />}
            {panes}
          </motion.span>
        )}
        {t.loading && <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-accent" />}
        <button
          onPointerDown={(e) => e.stopPropagation()}
          onClick={() => L.closeWorkspace(ws.id)}
          aria-label={`Close ${title}`}
          className={`flex size-[18px] shrink-0 items-center justify-center rounded-[5px] text-ink-3 hover:bg-surface-3 hover:text-ink ${active ? "opacity-100" : "opacity-0 group-hover:opacity-100"}`}
        >
          <X size={12} strokeWidth={2.2} />
        </button>
      </div>
    </Reorder.Item>
  );
}

function WindowControls() {
  const w = api().window;
  const btn = "no-drag flex h-full w-[46px] items-center justify-center text-ink-2 transition-colors hover:bg-surface-3";
  return (
    <div className="flex h-full items-stretch">
      <button className={btn} onClick={() => w.minimize()} aria-label="Minimize">
        <Minus size={15} />
      </button>
      <button className={btn} onClick={() => w.toggleMaximize()} aria-label="Maximize">
        <Square size={12} />
      </button>
      <button className={`${btn} hover:!bg-[#e81123] hover:!text-white`} onClick={() => w.close()} aria-label="Close">
        <X size={16} />
      </button>
    </div>
  );
}

export function TitleBar() {
  const workspaces = useLayout((s) => s.workspaces);
  const activeWs = useLayout((s) => s.active);
  const active = useTabs((s) => s.active);
  const order = workspaces.map((w) => w.id);
  const mac = isMac();

  return (
    <header
      className="drag-region flex h-[46px] shrink-0 items-center gap-2 bg-bg"
      data-tauri-drag-region
    >
      {/* Double-click to maximize is handled by Tauri's drag region itself. */}
      {/* macOS traffic lights sit here (overlay title bar). */}
      <div className={mac ? "w-[78px] shrink-0" : "w-2 shrink-0"} data-tauri-drag-region />
      <Reorder.Group axis="x" values={order} onReorder={(ids) => useLayout.getState().reorder(ids)} className="flex min-w-0 flex-1 items-center gap-1 overflow-hidden" data-tauri-drag-region>
        <AnimatePresence initial={false}>
          {workspaces.map((w) => (
            <TabPill key={w.id} ws={w} active={w.id === activeWs} />
          ))}
        </AnimatePresence>
        <motion.button
          layout
          whileTap={{ scale: 0.9 }}
          onClick={() => useTabs.getState().newTab(undefined, { afterId: active })}
          className="no-drag ml-0.5 flex size-[28px] shrink-0 items-center justify-center rounded-[8px] text-ink-3 hover:bg-chrome-active hover:text-ink"
          title={`New tab (${kbd("Mod+T")})`}
          aria-label="New tab"
        >
          <Plus size={16} />
        </motion.button>
        <div className="h-full min-w-6 flex-1" data-tauri-drag-region />
      </Reorder.Group>
      <div className="flex items-center gap-1 pr-2">
        <IoMonitor />
        <TransfersButton />
        <button
          onClick={() => useUi.getState().set({ palette: true })}
          className="no-drag hidden h-[30px] items-center gap-2 rounded-[9px] border border-line bg-surface/60 px-2.5 text-ink-3 hover:text-ink-2 lg:flex"
          title="Command palette"
        >
          <Search size={13} />
          <span className="text-[12px]">Commands</span>
          <kbd className="num rounded bg-surface-3 px-1 text-[10px]">{kbd("Mod+K")}</kbd>
        </button>
      </div>
      {!mac && <WindowControls />}
    </header>
  );
}
