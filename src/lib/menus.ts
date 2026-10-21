import {
  ClipboardPaste,
  Copy,
  Eye,
  FilePlus,
  FolderOpen,
  FolderPlus,
  Info,
  Link2,
  PanelTop,
  Pencil,
  RefreshCw,
  Scissors,
  Star,
  Trash2,
  CopyPlus,
  ExternalLink,
  Ruler,
} from "lucide-react";
import { api } from "./api";
import { copySelection, deleteSelection, newFile, newFolder, openEntry, paste, selectedEntries, transfer } from "./ops";
import { isMac, kbd } from "./platform";
import { measureFolders } from "./sizes";
import type { Entry } from "./types";
import { useSettings } from "../store/settings";
import { useTabs, type Tab } from "../store/tabs";
import { toast, useUi, type MenuItem } from "../store/ui";

const revealLabel = () => (isMac() ? "Reveal in Finder" : platformIsWindows() ? "Show in Explorer" : "Show in file manager");
const platformIsWindows = () => document.documentElement.dataset.os === "windows";

async function copyText(text: string, what: string) {
  try {
    await navigator.clipboard.writeText(text);
    toast({ tone: "success", title: `Copied ${what}` });
  } catch {
    toast({ tone: "error", title: "Clipboard unavailable" });
  }
}

export function itemMenu(tab: Tab, entry: Entry): MenuItem[] {
  const sel = selectedEntries(tab);
  const many = sel.length > 1;
  const clip = useUi.getState().clipboard;
  const local = tab.fs === "local";
  const items: MenuItem[] = [
    { label: entry.isDir ? "Open" : local ? "Open" : "Download & open", icon: FolderOpen, shortcut: "↩", run: () => openEntry(tab, entry) },
  ];
  if (entry.isDir) items.push({ label: "Open in new tab", icon: PanelTop, shortcut: kbd("Mod+↩"), run: () => openEntry(tab, entry, { newTab: true }) });
  items.push({ label: "Quick Look", icon: Eye, shortcut: "Space", run: () => useUi.getState().set({ quickLook: { fs: tab.fs, entry } }) });
  items.push({ divider: true });
  items.push({ label: "Copy", icon: Copy, shortcut: kbd("Mod+C"), run: () => copySelection(false) });
  items.push({ label: "Cut", icon: Scissors, shortcut: kbd("Mod+X"), run: () => copySelection(true) });
  if (entry.isDir && clip) items.push({ label: "Paste into folder", icon: ClipboardPaste, run: () => paste({ fs: tab.fs, path: entry.path }) });
  items.push({
    label: many ? `Duplicate ${sel.length} items` : "Duplicate",
    icon: CopyPlus,
    shortcut: kbd("Mod+D"),
    run: () => transfer("copy", sel.map((e) => ({ fs: tab.fs, path: e.path })), { fs: tab.fs, path: tab.path }),
  });
  if (!many) items.push({ label: "Rename", icon: Pencil, shortcut: "F2", run: () => useTabs.getState().patch(tab.id, { renaming: entry.path }) });
  items.push({ divider: true });
  if (entry.isDir && !many) {
    const fav = useSettings.getState().favorites.some((f) => f.fs === tab.fs && f.path === entry.path);
    items.push({ label: fav ? "Remove from Favorites" : "Add to Favorites", icon: Star, run: () => useSettings.getState().toggleFavorite({ fs: tab.fs, path: entry.path, name: entry.name }) });
  }
  items.push({ label: many ? "Copy paths" : "Copy path", icon: Link2, shortcut: kbd("Mod+Alt+C"), run: () => copyText(sel.map((e) => e.path).join("\n"), many ? "paths" : "path") });
  if (local && !many) items.push({ label: revealLabel(), icon: ExternalLink, run: () => api().reveal(entry.path) });
  const dirs = sel.filter((e) => e.isDir);
  if (dirs.length) {
    items.push({
      label: dirs.length > 1 ? `Calculate size of ${dirs.length} folders` : "Calculate size",
      icon: Ruler,
      run: () => void measureFolders(tab.fs, dirs.map((e) => e.path), true),
    });
  }
  items.push({ label: "Get info", icon: Info, shortcut: kbd("Mod+I"), run: () => useSettings.getState().set({ previewPane: true }) });
  items.push({ divider: true });
  items.push(
    local
      ? { label: "Move to Trash", icon: Trash2, shortcut: isMac() ? "⌘⌫" : "Del", danger: true, run: () => deleteSelection(false) }
      : { label: "Delete permanently…", icon: Trash2, shortcut: isMac() ? "⌘⌫" : "Del", danger: true, run: () => deleteSelection(true) },
  );
  if (local) items.push({ label: "Delete permanently…", icon: Trash2, shortcut: isMac() ? "⌥⌘⌫" : "Shift+Del", danger: true, run: () => deleteSelection(true) });
  return items;
}

export function backgroundMenu(tab: Tab): MenuItem[] {
  const clip = useUi.getState().clipboard;
  const s = useSettings.getState();
  const { patch, refresh } = useTabs.getState();
  return [
    { label: "New folder", icon: FolderPlus, shortcut: kbd("Mod+Shift+N"), run: () => newFolder() },
    { label: "New file", icon: FilePlus, run: () => newFile() },
    { divider: true },
    { label: clip ? `Paste ${clip.items.length === 1 ? "item" : `${clip.items.length} items`}` : "Paste", icon: ClipboardPaste, shortcut: kbd("Mod+V"), disabled: !clip, run: () => paste() },
    { divider: true },
    { label: tab.view === "list" ? "View as icons" : "View as list", run: () => patch(tab.id, { view: tab.view === "list" ? "grid" : "list" }) },
    { label: s.showHidden ? "Hide hidden files" : "Show hidden files", shortcut: kbd("Mod+Shift+."), run: () => s.set({ showHidden: !s.showHidden }) },
    { label: "Refresh", icon: RefreshCw, shortcut: isMac() ? "⌘R" : "F5", run: () => refresh(tab.id) },
    { divider: true },
    { label: "Copy folder path", icon: Link2, run: () => copyText(tab.path, "path") },
    ...(tab.fs === "local" ? [{ label: revealLabel(), icon: ExternalLink, run: () => api().reveal(tab.path) }] : []),
  ];
}
