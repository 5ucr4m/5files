// Internal drag & drop between views, tabs and the sidebar.
import type { DragEvent } from "react";
import { isMac } from "./platform";
import { transfer } from "./ops";
import type { Loc } from "./types";

export const MIME = "application/x-atlas-items";

let dragging: Loc[] | null = null;

export function startDrag(e: DragEvent, items: Loc[], label: string) {
  dragging = items;
  e.dataTransfer.effectAllowed = "copyMove";
  e.dataTransfer.setData(MIME, JSON.stringify(items));
  e.dataTransfer.setData("text/plain", items.map((i) => i.path).join("\n"));
  // Compact drag badge instead of the browser's ghost of the whole row.
  const badge = document.createElement("div");
  badge.textContent = label;
  badge.style.cssText =
    "position:fixed;top:-100px;left:-100px;padding:6px 10px;border-radius:9px;font:500 12px var(--font-sans);background:var(--accent);color:white;box-shadow:0 8px 24px rgb(0 0 0/.25);white-space:nowrap";
  document.body.appendChild(badge);
  e.dataTransfer.setDragImage(badge, -12, -8);
  setTimeout(() => badge.remove(), 0);
}

export function endDrag() {
  dragging = null;
}

export const isInternalDrag = (e: DragEvent) => e.dataTransfer.types.includes(MIME);

/** Copy when the platform's "copy" modifier is held (⌥ on macOS, Ctrl elsewhere) or across drives. */
export function dropEffect(e: DragEvent, dest: Loc): "copy" | "move" {
  const copyKey = isMac() ? e.altKey : e.ctrlKey;
  const src = dragging?.[0];
  if (copyKey) return "copy";
  return src && src.fs === dest.fs ? "move" : "copy";
}

export function canDrop(dest: Loc): boolean {
  if (!dragging) return true;
  // Can't drop a folder into itself.
  return !dragging.some((s) => s.fs === dest.fs && (s.path === dest.path || dest.path.startsWith(s.path + "/") || dest.path.startsWith(s.path + "\\")));
}

export async function handleDrop(e: DragEvent, dest: Loc) {
  const raw = e.dataTransfer.getData(MIME);
  if (!raw) return;
  e.preventDefault();
  const items: Loc[] = JSON.parse(raw);
  const effect = dropEffect(e, dest);
  dragging = null;
  if (!canDropItems(items, dest)) return;
  await transfer(effect, items, dest);
}

function canDropItems(items: Loc[], dest: Loc) {
  return !items.some((s) => s.fs === dest.fs && (s.path === dest.path || dest.path.startsWith(s.path + "/") || dest.path.startsWith(s.path + "\\")));
}

/** A drag carrying files from outside the app (only seen as HTML5 drag on Windows). */
export const isExternalFileDrag = (e: DragEvent) => !isInternalDrag(e) && e.dataTransfer.types.includes("Files");

/** Real files from an HTML5 drop (folders are left out: the File API can't read them). */
export function droppedFiles(e: DragEvent): { files: File[]; folders: number } {
  const files: File[] = [];
  let folders = 0;
  const items = Array.from(e.dataTransfer.items ?? []);
  items.forEach((it, i) => {
    const entry = (it as DataTransferItem & { webkitGetAsEntry?: () => { isDirectory: boolean } | null }).webkitGetAsEntry?.();
    if (entry?.isDirectory) folders++;
    else {
      const f = it.getAsFile() ?? e.dataTransfer.files[i];
      if (f) files.push(f);
    }
  });
  return { files, folders };
}
