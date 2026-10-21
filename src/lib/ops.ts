// User-level file operations. Everything long-running goes through the transfer queue.
import { api, errorKind, errorMessage } from "./api";
import { freshName } from "./format";
import type { Entry, FsId, JobKind, Loc } from "./types";
import { activeTab, useTabs, type Tab } from "../store/tabs";
import { confirmDialog, conflictDialog, toast, useUi } from "../store/ui";
import { useSettings } from "../store/settings";
import { useDrives } from "../store/live";
import { kbd } from "./platform";

export function selectedEntries(t: Tab | undefined): Entry[] {
  if (!t) return [];
  const pool = t.search ? t.search.results : (t.listing?.entries ?? []);
  return pool.filter((e) => t.selection.has(e.path));
}

const lastSegment = (p: string) => p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || p;

export function fsName(fs: FsId): string {
  if (fs === "local") return "This computer";
  return useDrives.getState().connections.find((c) => c.id === fs)?.name ?? "Remote";
}

export function copySelection(cut = false) {
  const t = activeTab();
  const items = selectedEntries(t).map((e) => ({ fs: t!.fs, path: e.path }));
  if (!items.length) return;
  useUi.getState().set({ clipboard: { mode: cut ? "cut" : "copy", items } });
  toast({ tone: "info", title: cut ? `Cut ${plural(items.length, "item")}` : `Copied ${plural(items.length, "item")}`, body: `Paste with ${kbd("Mod+V")} in any tab` });
}

export async function paste(dest?: Loc) {
  const clip = useUi.getState().clipboard;
  const t = activeTab();
  if (!clip || !t) return;
  const target = dest ?? { fs: t.fs, path: t.path };
  const ok = await transfer(clip.mode === "cut" ? "move" : "copy", clip.items, target);
  if (ok && clip.mode === "cut") useUi.getState().set({ clipboard: null });
}

/** Enqueues a copy/move after resolving name conflicts with the user. */
export async function transfer(kind: Exclude<JobKind, "delete">, sources: Loc[], dest: Loc): Promise<boolean> {
  if (!sources.length) return false;
  if (kind === "move" && sources.every((s) => s.fs === dest.fs && parentPath(s.path) === normalize(dest.path))) return false;
  try {
    const names = await api().checkConflicts(sources, dest);
    let conflict: "overwrite" | "skip" | "keepBoth" = "keepBoth";
    if (names.length) {
      const choice = await conflictDialog(names, lastSegment(dest.path) || fsName(dest.fs));
      if (!choice) return false;
      conflict = choice;
    }
    await api().enqueue({ kind, sources, dest, conflict });
    return true;
  } catch (e) {
    toast({ tone: "error", title: `Couldn't start ${kind}`, body: errorMessage(e) });
    return false;
  }
}

function normalize(p: string) {
  return p.length > 1 ? p.replace(/[\\/]+$/, "") : p;
}
function parentPath(p: string) {
  const n = normalize(p);
  const i = Math.max(n.lastIndexOf("/"), n.lastIndexOf("\\"));
  return i <= 0 ? n.slice(0, i + 1) || "/" : n.slice(0, i);
}

export async function deleteSelection(permanent = false) {
  const t = activeTab();
  const items = selectedEntries(t);
  if (!t || !items.length) return;
  const useTrash = t.fs === "local" && !permanent;
  const label = items.length === 1 ? `"${items[0].name}"` : plural(items.length, "item");
  if (!useTrash || useSettings.getState().confirmTrash) {
    const ok = await confirmDialog(
      useTrash
        ? { title: `Move ${label} to the Trash?`, confirm: "Move to Trash" }
        : {
            title: `Delete ${label} permanently?`,
            body: t.fs === "local" ? "This can't be undone." : `Files on ${fsName(t.fs)} don't go to a trash. This can't be undone.`,
            confirm: "Delete",
            danger: true,
          },
    );
    if (!ok) return;
  }
  try {
    if (useTrash) {
      await api().trash(items.map((e) => e.path));
      toast({ tone: "success", title: `Moved ${label} to the Trash` });
      useTabs.getState().refreshWhere((x) => x.fs === "local" && x.path === t.path);
    } else {
      await api().enqueue({ kind: "delete", sources: items.map((e) => ({ fs: t.fs, path: e.path })) });
    }
  } catch (e) {
    toast({ tone: "error", title: "Couldn't delete", body: errorMessage(e) });
  }
}

export async function newFolder() {
  const t = activeTab();
  if (!t?.listing) return;
  const names = new Set(t.listing.entries.map((e) => e.name));
  try {
    const e = await api().createDir(t.fs, t.path, freshName("untitled folder", names));
    await useTabs.getState().refresh(t.id);
    useTabs.getState().select(t.id, [e.path]);
    useTabs.getState().patch(t.id, { renaming: e.path });
  } catch (err) {
    toast({ tone: "error", title: "Couldn't create folder", body: errorMessage(err) });
  }
}

export async function newFile() {
  const t = activeTab();
  if (!t?.listing) return;
  const names = new Set(t.listing.entries.map((e) => e.name));
  try {
    const e = await api().createFile(t.fs, t.path, freshName("untitled.txt", names));
    await useTabs.getState().refresh(t.id);
    useTabs.getState().select(t.id, [e.path]);
    useTabs.getState().patch(t.id, { renaming: e.path });
  } catch (err) {
    toast({ tone: "error", title: "Couldn't create file", body: errorMessage(err) });
  }
}

export async function commitRename(tab: Tab, entry: Entry, newName: string) {
  const name = newName.trim();
  useTabs.getState().patch(tab.id, { renaming: null });
  if (!name || name === entry.name) return;
  try {
    const e = await api().rename(tab.fs, entry.path, name);
    await useTabs.getState().refresh(tab.id);
    useTabs.getState().select(tab.id, [e.path]);
  } catch (err) {
    toast({ tone: "error", title: `Couldn't rename "${entry.name}"`, body: errorMessage(err) });
  }
}

export async function openEntry(tab: Tab, entry: Entry, opts?: { newTab?: boolean }) {
  if (entry.isDir) {
    const loc = { fs: tab.fs, path: entry.path };
    if (opts?.newTab) useTabs.getState().newTab(loc, { background: true, afterId: tab.id });
    else await useTabs.getState().navigate(tab.id, loc);
    return;
  }
  try {
    const job = await api().open(tab.fs, entry.path);
    if (job) toast({ tone: "info", title: `Downloading "${entry.name}"`, body: "It opens as soon as the download finishes." });
  } catch (e) {
    toast({ tone: "error", title: `Couldn't open "${entry.name}"`, body: errorMessage(e) });
  }
}

/** Opens a connection in the active tab (or a new one), connecting first. */
export async function openConnection(id: string, opts?: { newTab?: boolean }) {
  const drives = useDrives.getState();
  drives.setStatus(id, "connecting");
  try {
    const root = await api().connect(id);
    drives.setStatus(id, "connected");
    const t = activeTab();
    const loc = { fs: id, path: root };
    if (!t || opts?.newTab) useTabs.getState().newTab(loc);
    else await useTabs.getState().navigate(t.id, loc);
  } catch (e) {
    drives.setStatus(id, "error", errorMessage(e));
    const c = drives.connections.find((x) => x.id === id);
    toast({
      tone: "error",
      title: `Couldn't connect to ${c?.name ?? "drive"}`,
      body: errorMessage(e),
      action: errorKind(e) === "auth" || errorKind(e) === "invalid" ? { label: "Edit connection", run: () => useUi.getState().set({ panel: "connections", editing: { id } }) } : undefined,
    });
  }
}

export function plural(n: number, word: string) {
  return `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;
}
