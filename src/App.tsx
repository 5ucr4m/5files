import { AnimatePresence, MotionConfig, motion } from "motion/react";
import { useEffect } from "react";
import { CommandPalette } from "./components/CommandPalette";
import { ConnectionsPanel } from "./components/Connections";
import { FileView } from "./components/FileView";
import { ConfirmDialog, ConflictDialog, ContextMenu, Modal, Toasts } from "./components/Overlays";
import { PreviewPane, QuickLook } from "./components/Preview";
import { SettingsPanel } from "./components/Settings";
import { Sidebar } from "./components/Sidebar";
import { StatusBar } from "./components/StatusBar";
import { TitleBar } from "./components/TitleBar";
import { Toolbar } from "./components/Toolbar";
import { TransferPanel } from "./components/Transfers";
import { api } from "./lib/api";
import { fsName, plural, transfer } from "./lib/ops";
import { isMac, isMod } from "./lib/platform";
import { useDrives, useMetrics, useTransfers } from "./store/live";
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
}

function useBackendEvents() {
  useEffect(() => {
    const a = api();
    const subs = [
      a.onTransfers((jobs) => useTransfers.getState().set(jobs)),
      a.onMetrics((m) => useMetrics.getState().push(m)),
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

function useShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tabs = useTabs.getState();
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
      if (ui.palette || ui.confirm || ui.conflict) return;
      if (mod && k === "t") return stop(), tabs.newTab(t ? { fs: t.fs, path: t.path } : undefined, { afterId: t?.id });
      if (mod && k === "w") return stop(), t && tabs.closeTab(t.id);
      if ((e.ctrlKey && k === "tab") || (mod && e.shiftKey && (e.key === "]" || e.key === "}"))) return stop(), tabs.cycle(e.shiftKey && k === "tab" ? -1 : 1);
      if (mod && e.shiftKey && (e.key === "[" || e.key === "{")) return stop(), tabs.cycle(-1);
      if (mod && !e.shiftKey && /^[1-9]$/.test(e.key) && e.altKey) {
        const target = tabs.tabs[Number(e.key) - 1];
        return stop(), target && tabs.activate(target.id);
      }
      if (mod && !e.shiftKey && e.key === "1") return stop(), t && tabs.patch(t.id, { view: "list" });
      if (mod && !e.shiftKey && e.key === "2") return stop(), t && tabs.patch(t.id, { view: "grid" });
      if (mod && k === "l") return stop(), window.dispatchEvent(new Event("atlas:edit-path"));
      if (mod && k === "f") return stop(), window.dispatchEvent(new Event("atlas:focus-search"));
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
    const onMouse = (e: MouseEvent) => {
      const t = activeTab();
      if (!t) return;
      if (e.button === 3) (e.preventDefault(), useTabs.getState().back(t.id));
      if (e.button === 4) (e.preventDefault(), useTabs.getState().forward(t.id));
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mouseup", onMouse);
    return () => {
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
          <main className="mr-2 mb-2 flex min-w-0 flex-1 flex-col overflow-hidden rounded-xl border border-line bg-surface shadow-soft">
            {tab && (
              <>
                <Toolbar tab={tab} />
                <div className="flex min-h-0 flex-1">
                  <div className="relative min-w-0 flex-1">
                    {tab.loading && !tab.listing && <div className="indeterminate absolute inset-x-0 top-0 z-20 h-[2px] overflow-hidden" />}
                    <FileView key={tab.id} tab={tab} />
                  </div>
                  <AnimatePresence initial={false}>
                    {preview && (
                      <motion.div initial={{ width: 0, opacity: 0 }} animate={{ width: 300, opacity: 1 }} exit={{ width: 0, opacity: 0 }} transition={{ type: "spring", stiffness: 500, damping: 42 }} className="flex overflow-hidden">
                        <PreviewPane />
                      </motion.div>
                    )}
                  </AnimatePresence>
                </div>
                <StatusBar tab={tab} />
              </>
            )}
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
        <Modal open={panel === "settings"} onClose={closePanel} width={520}>
          <SettingsPanel />
        </Modal>

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
