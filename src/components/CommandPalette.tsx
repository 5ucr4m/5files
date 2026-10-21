import { CornerDownLeft, Search } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useRef, useState } from "react";
import { copySelection, deleteSelection, newFile, newFolder, openConnection, paste } from "../lib/ops";
import { isMac, kbd } from "../lib/platform";
import { useLayout } from "../store/layout";
import { useDrives } from "../store/live";
import { useSettings } from "../store/settings";
import { activeTab, useTabs } from "../store/tabs";
import { useUi } from "../store/ui";

interface Command {
  id: string;
  title: string;
  group: string;
  hint?: string;
  run: () => void;
}

function commands(): Command[] {
  const tabs = useTabs.getState();
  const L = useLayout.getState();
  const s = useSettings.getState();
  const ui = useUi.getState();
  const t = activeTab();
  const { places, volumes, connections } = useDrives.getState();
  const go = (fs: string, path: string) => t && tabs.navigate(t.id, { fs, path });
  return [
    { id: "new-tab", group: "Tabs", title: "New tab", hint: kbd("Mod+T"), run: () => tabs.newTab(undefined, { afterId: t?.id }) },
    { id: "close-tab", group: "Tabs", title: "Close tab", hint: kbd("Mod+W"), run: () => t && tabs.closeTab(t.id) },
    { id: "dup-tab", group: "Tabs", title: "Duplicate tab", run: () => t && tabs.newTab({ fs: t.fs, path: t.path }, { afterId: t.id }) },
    { id: "close-others", group: "Tabs", title: "Close other tabs", run: () => t && tabs.closeOthers(t.id) },
    { id: "split-right", group: "Panes", title: "Split right", hint: kbd("Mod+\\"), run: () => L.split("row") },
    { id: "split-down", group: "Panes", title: "Split down", hint: kbd("Mod+Shift+\\"), run: () => L.split("col") },
    { id: "pane-next", group: "Panes", title: "Focus next pane", hint: isMac() ? "⌃B o" : "Ctrl+B o", run: () => L.next(1) },
    { id: "pane-zoom", group: "Panes", title: "Zoom pane (toggle)", hint: kbd("Mod+Shift+Enter"), run: () => L.toggleZoom() },
    { id: "pane-close", group: "Panes", title: "Close pane", hint: kbd("Mod+W"), run: () => t && tabs.closeTab(t.id) },
    { id: "new-folder", group: "Files", title: "New folder", hint: kbd("Mod+Shift+N"), run: () => newFolder() },
    { id: "new-file", group: "Files", title: "New file", run: () => newFile() },
    { id: "copy", group: "Files", title: "Copy selection", hint: kbd("Mod+C"), run: () => copySelection(false) },
    { id: "cut", group: "Files", title: "Cut selection", hint: kbd("Mod+X"), run: () => copySelection(true) },
    { id: "paste", group: "Files", title: "Paste", hint: kbd("Mod+V"), run: () => paste() },
    { id: "trash", group: "Files", title: "Move selection to Trash", run: () => deleteSelection(false) },
    { id: "go-path", group: "Go", title: "Go to folder…", hint: kbd("Mod+L"), run: () => window.dispatchEvent(new Event("5files:edit-path")) },
    { id: "search", group: "Go", title: "Search in this folder", hint: kbd("Mod+F"), run: () => window.dispatchEvent(new Event("5files:focus-search")) },
    ...places.map((p) => ({ id: `place-${p.id}`, group: "Go", title: `Go to ${p.name}`, run: () => go("local", p.path) })),
    ...volumes.map((v) => ({ id: `vol-${v.mount}`, group: "Drives", title: `Open ${v.name}`, run: () => go("local", v.mount) })),
    ...connections.map((c) => ({ id: `conn-${c.id}`, group: "Drives", title: `Open ${c.name}`, run: () => openConnection(c.id) })),
    { id: "add-drive", group: "Drives", title: "Connect a remote drive…", run: () => ui.set({ panel: "connections", editing: null }) },
    { id: "view-list", group: "View", title: "View as list", hint: kbd("Mod+1"), run: () => t && tabs.patch(t.id, { view: "list" }) },
    { id: "view-grid", group: "View", title: "View as icons", hint: kbd("Mod+2"), run: () => t && tabs.patch(t.id, { view: "grid" }) },
    { id: "hidden", group: "View", title: s.showHidden ? "Hide hidden files" : "Show hidden files", hint: kbd("Mod+Shift+."), run: () => s.set({ showHidden: !s.showHidden }) },
    { id: "preview", group: "View", title: s.previewPane ? "Hide preview pane" : "Show preview pane", run: () => s.set({ previewPane: !s.previewPane }) },
    { id: "theme-light", group: "Appearance", title: "Light theme", run: () => s.set({ theme: "light" }) },
    { id: "theme-dark", group: "Appearance", title: "Dark theme", run: () => s.set({ theme: "dark" }) },
    { id: "theme-system", group: "Appearance", title: "Match system theme", run: () => s.set({ theme: "system" }) },
    { id: "transfers", group: "App", title: "Show transfers", run: () => ui.set({ panel: "transfers" }) },
    { id: "settings", group: "App", title: "Settings", hint: kbd("Mod+,"), run: () => ui.set({ panel: "settings" }) },
    { id: "shortcuts", group: "App", title: "Keyboard shortcuts", hint: kbd("Mod+/"), run: () => ui.set({ panel: "shortcuts" }) },
  ];
}

/** Subsequence fuzzy score: higher is better, -1 means no match. */
export function fuzzy(query: string, text: string): number {
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  if (!q) return 0;
  let score = 0;
  let ti = 0;
  let streak = 0;
  for (const ch of q) {
    const i = t.indexOf(ch, ti);
    if (i < 0) return -1;
    streak = i === ti ? streak + 1 : 0;
    score += 1 + streak * 2 + (i === 0 || t[i - 1] === " " ? 3 : 0);
    ti = i + 1;
  }
  return score - (t.length - q.length) * 0.01;
}

export function CommandPalette() {
  const open = useUi((s) => s.palette);
  const [q, setQ] = useState("");
  const [hi, setHi] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const all = useMemo(() => (open ? commands() : []), [open]);
  const results = useMemo(
    () =>
      q
        ? all
            .map((c) => ({ c, s: fuzzy(q, c.title) }))
            .filter((x) => x.s >= 0)
            .sort((a, b) => b.s - a.s)
            .map((x) => x.c)
        : all,
    [q, all],
  );
  useEffect(() => {
    if (open) {
      setQ("");
      setHi(0);
      setTimeout(() => input.current?.focus(), 10);
    }
  }, [open]);
  useEffect(() => setHi(0), [q]);
  useEffect(() => {
    list.current?.querySelector(`[data-i="${hi}"]`)?.scrollIntoView({ block: "nearest" });
  }, [hi]);

  const close = () => useUi.getState().set({ palette: false });
  const run = (c: Command) => {
    close();
    setTimeout(c.run, 0);
  };

  return (
    <AnimatePresence>
      {open && (
        <motion.div className="fixed inset-0 z-[96] flex justify-center pt-[12vh]" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.12 }}>
          <div className="absolute inset-0 bg-black/25" onMouseDown={close} />
          <motion.div
            initial={{ opacity: 0, y: -10, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -6, scale: 0.98 }}
            transition={{ type: "spring", stiffness: 600, damping: 38 }}
            className="relative flex h-fit max-h-[60vh] w-[560px] flex-col overflow-hidden rounded-2xl border border-line bg-elevated shadow-pop"
          >
            <div className="flex items-center gap-2.5 border-b border-line px-4">
              <Search size={16} className="text-ink-3" />
              <input
                ref={input}
                value={q}
                onChange={(e) => setQ(e.target.value)}
                placeholder="Type a command or a place…"
                className="h-12 flex-1 bg-transparent text-[14px] outline-none placeholder:text-ink-4"
                onKeyDown={(e) => {
                  e.stopPropagation();
                  if (e.key === "Escape") close();
                  if (e.key === "ArrowDown") (e.preventDefault(), setHi((h) => Math.min(results.length - 1, h + 1)));
                  if (e.key === "ArrowUp") (e.preventDefault(), setHi((h) => Math.max(0, h - 1)));
                  if (e.key === "Enter" && results[hi]) run(results[hi]);
                }}
                aria-label="Command"
              />
            </div>
            <div ref={list} className="scroll overflow-y-auto p-1.5">
              {results.length === 0 && <p className="px-3 py-6 text-center text-2xs text-ink-3">No commands match “{q}”.</p>}
              {results.map((c, i) => (
                <button
                  key={c.id}
                  data-i={i}
                  onMouseMove={() => setHi(i)}
                  onClick={() => run(c)}
                  className={`flex h-9 w-full items-center gap-3 rounded-lg px-3 text-left text-[13px] ${hi === i ? "bg-sel" : ""}`}
                >
                  <span className="w-20 shrink-0 text-2xs text-ink-4">{c.group}</span>
                  <span className="flex-1 truncate">{c.title}</span>
                  {c.hint && <span className="num text-[11px] text-ink-4">{c.hint}</span>}
                  {hi === i && <CornerDownLeft size={13} className="text-ink-3" />}
                </button>
              ))}
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
