import { AnimatePresence, MotionConfig, motion } from "motion/react";
import { useEffect } from "react";
import { CommandPalette } from "./components/CommandPalette";
import { ConnectionsPanel } from "./components/Connections";
import { ConfirmDialog, ConflictDialog, ContextMenu, Modal, Toasts } from "./components/Overlays";
import { PreviewPane, QuickLook } from "./components/Preview";
import { SettingsPanel } from "./components/Settings";
import { ShortcutsSheet } from "./components/Shortcuts";
import { DiskAccessNotice, useDiskAccessWatch } from "./components/DiskAccess";
import { SharePanel } from "./components/Share";
import { SyncPanel } from "./components/Sync";
import { Sidebar } from "./components/Sidebar";
import { TitleBar } from "./components/TitleBar";
import { TransferPanel } from "./components/Transfers";
import { PrefixHud, Workspace } from "./components/Workspace";
import { useLayout } from "./store/layout";
import { api } from "./lib/api";
import { ejectVolume, fsName, onVolume, plural, transfer } from "./lib/ops";
import { isMac, isMod } from "./lib/platform";
import { useDrives, useMetrics, useShares, useSyncs, useTransfers } from "./store/live";
import { ACCENTS, resolvedTheme, useSettings } from "./store/settings";
import { activeTab, useActiveTab, useTabs } from "./store/tabs";
import { toast, useUi } from "./store/ui";

function useTheme() {
  const theme = useSettings((s) => s.theme);
  const accent = useSettings((s) => s.accent);
  const reduce = useSettings((s) => s.reduceMotion);
  useEffect(() => {
    const apply = () => {
      const mode = resolvedTheme(theme);
      const root = document.documentElement;
      root.dataset.theme = mode;
      root.style.setProperty("--accent", ACCENTS[accent][mode]);
    };
    apply();
    const mq = matchMedia("(prefers-color-scheme: dark)");
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [theme, accent]);
  useEffect(() => {
    document.documentElement.dataset.reduceMotion = String(reduce);
  }, [reduce]);
  const keepRunning = useSettings((s) => s.keepRunning);
  useEffect(() => {
    void api().setBackgroundMode(keepRunning);
  }, [keepRunning]);
}

function useBackendEvents() {
  useEffect(() => {
    const a = api();
    const subs = [
      a.onTransfers((jobs) => useTransfers.getState().set(jobs)),
      a.onMetrics((m) => useMetrics.getState().push(m)),
      a.onSyncs((list) => useSyncs.getState().set(list)),
      a.onShares((list) => useShares.getState().set(list)),
      a.onTrayOpen((what) =>
        useUi.getState().set(what === "syncs" ? { panel: "sync", syncView: null } : what === "shares" ? { panel: "share", shareView: null } : { panel: "transfers" }),
      ),
      a.onFsChanged((dirs) => {
        const set = new Set(dirs);
        useTabs.getState().refreshWhere((t) => t.fs === "local" && !t.search && set.has(t.path));
      }),
      a.onTransferDone((job) => {
        // Remote folders have no watcher: refresh tabs that show the source or destination.
        const touched = new Set([job.dest ? `${job.dest.fs}|${job.dest.path}` : ""]);
        useTabs.getState().refreshWhere((t) => touched.has(`${t.fs}|${t.path}`) || (t.fs === job.srcFs && t.fs !== "local"));
        if (job.openAfter) return;
        const what = job.kind === "delete" ? "Deleted" : job.kind === "move" ? "Moved" : "Copied";
        toast({
          tone: "success",
          title: `${what} ${job.title}`,
          body: job.dest ? `to ${fsName(job.dest.fs)}${job.skipped ? ` · ${plural(job.skipped, "item")} skipped` : ""}` : undefined,
        });
      }),
      a.onDropFiles(async (paths, pos) => {
        if (!paths.length) return;
        const el = document.elementFromPoint(pos.x, pos.y)?.closest("[data-drop-dir]") as HTMLElement | null;
        const t = activeTab();
        const dir = el?.dataset.dropDir ?? t?.path;
        if (!t || !dir) return;
        await transfer("copy", paths.map((p) => ({ fs: "local", path: p })), { fs: t.fs, path: dir });
      }),
    ];
    void a.transfers().then((j) => useTransfers.getState().set(j));
    void a.syncList().then((list) => useSyncs.getState().set(list));
    void a.shareList().then((list) => useShares.getState().set(list));
    return () => subs.forEach((p) => p.then((u) => u()));
  }, []);

  // Watch the local folders that are open in any tab.
  useEffect(() => {
    let last = "";
    return useTabs.subscribe((s) => {
      const paths = [...new Set(s.tabs.filter((t) => t.fs === "local" && t.listing).map((t) => t.path))].sort();
      const key = paths.join("\n");
      if (key !== last) {
        last = key;
        void api().watch(paths);
      }
    });
  }, []);
}

const ARROWS: Record<string, "left" | "right" | "up" | "down"> = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" };

/** tmux keys after the Ctrl+B prefix. Returns false for keys it doesn't know. */
function prefixCommand(e: KeyboardEvent): boolean {
  const L = useLayout.getState();
  const tabs = useTabs.getState();
  // e.key first (works on every layout); the physical key covers synthetic events.
  if (e.key === "%" || (e.shiftKey && e.code === "Digit5")) L.split("row");
  else if (e.key === '"' || e.key === "'" || (e.shiftKey && e.code === "Quote")) L.split("col");
  else if (ARROWS[e.key]) L.move(ARROWS[e.key]);
  else if (e.key === "o") L.next(1);
  else if (e.key === ";") L.next(-1);
  else if (e.key === "z") L.toggleZoom();
  else if (e.key === "x") {
    const t = activeTab();
    if (t) tabs.closeTab(t.id);
  } else if (e.key === "c") {
    const t = activeTab();
    tabs.newTab(t ? { fs: t.fs, path: t.path } : undefined, { afterId: t?.id });
  } else if (e.key === "?") useUi.getState().set({ panel: "shortcuts" });
  else if (e.key === "n") L.cycle(1);
  else if (e.key === "p") L.cycle(-1);
  else if (/^[1-9]$/.test(e.key)) {
    const w = L.workspaces[Number(e.key) - 1];
    if (w) L.activate(w.id);
  } else return false;
  return true;
}

function useShortcuts() {
  useEffect(() => {
    let prefixTimer: ReturnType<typeof setTimeout> | undefined;
    const setPrefix = (on: boolean) => {
      clearTimeout(prefixTimer);
      useLayout.getState().setPrefix(on);
      if (on) prefixTimer = setTimeout(() => useLayout.getState().setPrefix(false), 2500);
    };
    // Capture phase: the tmux prefix must win over every other key handler.
    const onPrefix = (e: KeyboardEvent) => {
      const typing = (e.target as HTMLElement).closest("input, textarea, select, [contenteditable]");
      const ui = useUi.getState();
      if (typing || ui.palette || ui.confirm || ui.conflict) return;
      const swallow = () => {
        e.preventDefault();
        e.stopImmediatePropagation();
      };
      if (useLayout.getState().prefix) {
        if (["Shift", "Control", "Alt", "Meta"].includes(e.key)) return;
        swallow();
        setPrefix(false);
        if (e.key !== "Escape") prefixCommand(e);
        return;
      }
      if (e.ctrlKey && !e.metaKey && !e.altKey && e.key.toLowerCase() === "b") {
        swallow();
        setPrefix(true);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return;
      const tabs = useTabs.getState();
      const L = useLayout.getState();
      const ui = useUi.getState();
      const s = useSettings.getState();
      const t = activeTab();
      const mod = isMod(e);
      const k = e.key.toLowerCase();
      const typing = (e.target as HTMLElement).closest("input, textarea, select, [contenteditable]");
      const stop = () => {
        e.preventDefault();
        e.stopPropagation();
      };
      if (mod && k === "k") return stop(), ui.set({ palette: !ui.palette });
      if (mod && (e.key === "/" || e.code === "Slash")) return stop(), ui.set({ panel: ui.panel === "shortcuts" ? null : "shortcuts" });
      if (ui.palette || ui.confirm || ui.conflict) return;
      if (mod && k === "t") return stop(), tabs.newTab(t ? { fs: t.fs, path: t.path } : undefined, { afterId: t?.id });
      // Closes the focused pane when the tab is split, the tab otherwise.
      if (mod && k === "w") return stop(), t && tabs.closeTab(t.id);
      // Direct split shortcuts (no prefix): Mod+\ side by side, Mod+Shift+\ stacked.
      if (mod && !e.altKey && (e.key === "\\" || e.key === "|" || e.code === "Backslash")) return stop(), L.split(e.shiftKey ? "col" : "row");
      if (mod && e.altKey && ARROWS[e.key]) return stop(), L.move(ARROWS[e.key]);
      if (mod && e.shiftKey && e.key === "Enter") return stop(), L.toggleZoom();
      if ((e.ctrlKey && k === "tab") || (mod && e.shiftKey && (e.key === "]" || e.key === "}"))) return stop(), L.cycle(e.shiftKey && k === "tab" ? -1 : 1);
      if (mod && e.shiftKey && (e.key === "[" || e.key === "{")) return stop(), L.cycle(-1);
      if (mod && !e.shiftKey && /^[1-9]$/.test(e.key) && e.altKey) {
        const target = L.workspaces[Number(e.key) - 1];
        return stop(), target && L.activate(target.id);
      }
      if (mod && !e.shiftKey && e.key === "1") return stop(), t && tabs.patch(t.id, { view: "list" });
      if (mod && !e.shiftKey && e.key === "2") return stop(), t && tabs.patch(t.id, { view: "grid" });
      if (mod && k === "e" && !e.shiftKey && t?.fs === "local") {
        // Eject the drive the focused pane is on (like Finder's ⌘E).
        const v = useDrives.getState().volumes.filter((x) => x.ejectable && onVolume(t.path, x.mount)).sort((a, b) => b.mount.length - a.mount.length)[0];
        if (v) return stop(), void ejectVolume(v);
      }
      if (mod && k === "l") return stop(), window.dispatchEvent(new Event("5files:edit-path"));
      if (mod && k === "f") return stop(), window.dispatchEvent(new Event("5files:focus-search"));
      if (mod && e.key === ",") return stop(), ui.set({ panel: ui.panel === "settings" ? null : "settings" });
      if (mod && e.shiftKey && (e.key === "." || e.key === ">")) return stop(), s.set({ showHidden: !s.showHidden });
      if ((mod && e.altKey && k === "p") || (mod && k === "i")) return stop(), s.set({ previewPane: !s.previewPane });
      if (typing) return;
      if (!t) return;
      if ((mod && e.key === "[") || (e.altKey && e.key === "ArrowLeft")) return stop(), tabs.back(t.id);
      if ((mod && e.key === "]") || (e.altKey && e.key === "ArrowRight")) return stop(), tabs.forward(t.id);
      if (e.altKey && e.key === "ArrowUp") return stop(), tabs.up(t.id);
      if (e.key === "F5" || (mod && k === "r")) return stop(), void tabs.refresh(t.id);
    };
    // Mouse back/forward buttons act on the pane under the pointer (like a browser
    // window), falling back to the focused one.
    let pointer = { x: -1, y: -1 };
    const paneAt = (x: number, y: number) => {
      const id = (document.elementFromPoint(x, y)?.closest("[data-pane-id]") as HTMLElement | null)?.dataset.paneId;
      return id && useTabs.getState().tabs.some((t) => t.id === id) ? id : activeTab()?.id;
    };
    const mouseNav = (dir: "back" | "forward", x: number, y: number) => {
      const id = paneAt(x, y);
      if (!id) return;
      if (useUi.getState().panel || useUi.getState().quickLook) return;
      useLayout.getState().focusPane(id);
      if (dir === "back") useTabs.getState().back(id);
      else useTabs.getState().forward(id);
    };
    const onMove = (e: MouseEvent) => (pointer = { x: e.clientX, y: e.clientY });
    // Windows / Linux deliver the buttons to the page (3 = back, 4 = forward); cancelling
    // them also stops the webview from navigating its own history.
    const onMouseDown = (e: MouseEvent) => {
      if (e.button === 3 || e.button === 4) e.preventDefault();
    };
    const onMouse = (e: MouseEvent) => {
      if (e.button !== 3 && e.button !== 4) return;
      e.preventDefault();
      mouseNav(e.button === 3 ? "back" : "forward", e.clientX, e.clientY);
    };
    const nativeNav = api().onMouseNav((dir) => mouseNav(dir, pointer.x, pointer.y));
    window.addEventListener("keydown", onPrefix, true);
    window.addEventListener("keydown", onKey);
    window.addEventListener("mouseup", onMouse);
    window.addEventListener("mousedown", onMouseDown);
    window.addEventListener("mousemove", onMove, { passive: true });
    return () => {
      void nativeNav.then((u) => u());
      window.removeEventListener("mousedown", onMouseDown);
      window.removeEventListener("mousemove", onMove);
      clearTimeout(prefixTimer);
      window.removeEventListener("keydown", onPrefix, true);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mouseup", onMouse);
    };
  }, []);
}

function SidebarResizer() {
  return (
    <div
      className="group relative w-0 shrink-0 cursor-col-resize"
      onMouseDown={(e) => {
        const startX = e.clientX;
        const start = useSettings.getState().sidebarWidth;
        const move = (ev: MouseEvent) => useSettings.getState().set({ sidebarWidth: Math.max(180, Math.min(380, start + ev.clientX - startX)) });
        const up = () => {
          window.removeEventListener("mousemove", move);
          window.removeEventListener("mouseup", up);
          document.body.style.cursor = "";
        };
        document.body.style.cursor = "col-resize";
        window.addEventListener("mousemove", move);
        window.addEventListener("mouseup", up);
      }}
    >
      <div className="absolute inset-y-0 -left-1 w-2" />
    </div>
  );
}

export default function App() {
  useTheme();
  useBackendEvents();
  useDiskAccessWatch();
  useShortcuts();
  const tab = useActiveTab();
  const preview = useSettings((s) => s.previewPane);
  const reduce = useSettings((s) => s.reduceMotion);
  const panel = useUi((s) => s.panel);
  const closePanel = () => useUi.getState().set({ panel: null, editing: null });

  useEffect(() => {
    void useDrives.getState().refreshVolumes();
    void useDrives.getState().refreshConnections();
    // Volumes come and go (USB sticks): refresh periodically and on focus.
    const iv = setInterval(() => void useDrives.getState().refreshVolumes(), 15_000);
    const onFocus = () => void useDrives.getState().refreshVolumes();
    window.addEventListener("focus", onFocus);
    return () => {
      clearInterval(iv);
      window.removeEventListener("focus", onFocus);
    };
  }, []);

  return (
    <MotionConfig reducedMotion={reduce ? "always" : "user"}>
      <div className={`flex h-full flex-col bg-bg ${isMac() ? "" : "border border-line"}`}>
        <TitleBar />
        <div className="flex min-h-0 flex-1">
          <Sidebar />
          <SidebarResizer />
          <main className="mr-2 mb-2 flex min-w-0 flex-1 gap-[6px] overflow-hidden">
            <Workspace />
            <AnimatePresence initial={false}>
              {preview && tab && (
                <motion.div
                  initial={{ width: 0, opacity: 0 }}
                  animate={{ width: 300, opacity: 1 }}
                  exit={{ width: 0, opacity: 0 }}
                  transition={{ type: "spring", stiffness: 500, damping: 42 }}
                  className="flex overflow-hidden rounded-xl border border-line bg-surface shadow-soft"
                >
                  <PreviewPane />
                </motion.div>
              )}
            </AnimatePresence>
          </main>
        </div>

        <AnimatePresence>
          {panel === "transfers" && (
            <>
              <motion.div key="scrim" className="fixed inset-0 z-[70]" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} onMouseDown={closePanel} />
              <motion.aside
                key="transfers"
                initial={{ x: 380, opacity: 0.6 }}
                animate={{ x: 0, opacity: 1 }}
                exit={{ x: 380, opacity: 0 }}
                transition={{ type: "spring", stiffness: 420, damping: 40 }}
                className="fixed top-[50px] right-2 bottom-2 z-[71] w-[370px] overflow-hidden rounded-2xl border border-line bg-elevated shadow-pop"
              >
                <TransferPanel />
              </motion.aside>
            </>
          )}
        </AnimatePresence>
        <Modal open={panel === "connections"} onClose={closePanel} width={680}>
          <ConnectionsPanel />
        </Modal>
        <Modal open={panel === "sync"} onClose={() => useUi.getState().set({ panel: null, syncView: null })} width={720}>
          <SyncPanel />
        </Modal>
        <Modal open={panel === "share"} onClose={() => useUi.getState().set({ panel: null, shareView: null })} width={640}>
          <SharePanel />
        </Modal>
        <Modal open={panel === "shortcuts"} onClose={closePanel} width={820}>
          <ShortcutsSheet />
        </Modal>
        <Modal open={panel === "settings"} onClose={closePanel} width={520}>
          <SettingsPanel />
        </Modal>

        <DiskAccessNotice />
        <PrefixHud />
        <QuickLook />
        <CommandPalette />
        <ConfirmDialog />
        <ConflictDialog />
        <ContextMenu />
        <Toasts />
      </div>
    </MotionConfig>
  );
}
