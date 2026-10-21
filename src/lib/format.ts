import type { Entry } from "./types";

const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

/** Decimal (SI) units, like Finder/Nautilus: 1 KB = 1000 B. */
export function formatBytes(n: number, digits = 1): string {
  if (!Number.isFinite(n) || n < 0) return "—";
  if (n < 1000) return `${Math.round(n)} B`;
  let i = 0;
  let v = n;
  while (v >= 1000 && i < UNITS.length - 1) {
    v /= 1000;
    i++;
  }
  const d = v >= 100 ? 0 : digits;
  return `${v.toFixed(d)} ${UNITS[i]}`;
}

export function formatRate(bytesPerSec: number): string {
  if (bytesPerSec < 1) return "0 B/s";
  return `${formatBytes(bytesPerSec)}/s`;
}

export function formatDuration(secs: number | null | undefined): string {
  if (secs == null || !Number.isFinite(secs)) return "";
  if (secs < 1) return "<1s";
  if (secs < 60) return `${Math.round(secs)}s`;
  const m = Math.floor(secs / 60);
  if (m < 60) return `${m}m ${Math.round(secs % 60)}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

const rtf = typeof Intl !== "undefined" ? new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }) : null;
const dtfDate = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric" });
const dtfTime = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const dtfFull = new Intl.DateTimeFormat(undefined, { dateStyle: "full", timeStyle: "short" });

/** "Today, 14:02" / "Yesterday, 09:11" / "3 Oct 2026". */
export function formatDate(ms: number | null | undefined, nowMs = Date.now()): string {
  if (!ms) return "—";
  const d = new Date(ms);
  const today = new Date(nowMs);
  today.setHours(0, 0, 0, 0);
  const diffDays = Math.floor((today.getTime() - new Date(d).setHours(0, 0, 0, 0)) / 86_400_000);
  if (diffDays === 0 && rtf) return `${capitalize(rtf.format(0, "day"))}, ${dtfTime.format(d)}`;
  if (diffDays === 1 && rtf) return `${capitalize(rtf.format(-1, "day"))}, ${dtfTime.format(d)}`;
  return dtfDate.format(d);
}

export function formatDateFull(ms: number | null | undefined): string {
  return ms ? dtfFull.format(new Date(ms)) : "—";
}

function capitalize(s: string) {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

export type Category =
  | "folder"
  | "image"
  | "video"
  | "audio"
  | "pdf"
  | "doc"
  | "sheet"
  | "slides"
  | "archive"
  | "code"
  | "text"
  | "font"
  | "disk"
  | "app"
  | "file";

const EXT: Record<string, Category> = {};
const add = (c: Category, list: string) => list.split(" ").forEach((e) => (EXT[e] = c));
add("image", "jpg jpeg png gif webp bmp ico tif tiff heic heif avif svg raw cr2 nef arw dng psd");
add("video", "mp4 mov mkv webm avi m4v wmv flv mpg mpeg 3gp");
add("audio", "mp3 wav flac aac ogg m4a opus aiff wma alac");
add("pdf", "pdf");
add("doc", "doc docx odt rtf pages");
add("sheet", "xls xlsx ods csv tsv numbers");
add("slides", "ppt pptx odp key");
add("archive", "zip rar 7z tar gz tgz bz2 xz zst lz4 cab");
add(
  "code",
  "js mjs cjs ts tsx jsx json rs go py rb php java kt swift c h cpp hpp cs css scss less html htm vue svelte sh bash zsh ps1 bat sql yml yaml toml xml lua dart r scala ex exs erl hs ml clj zig nim gradle dockerfile makefile lock",
);
add("text", "txt md markdown log ini cfg conf env rst tex org");
add("font", "ttf otf woff woff2");
add("disk", "iso img dmg vhd vhdx vmdk qcow2");
add("app", "exe msi app apk deb rpm appimage pkg");

export function categoryOf(e: Pick<Entry, "isDir" | "ext" | "name">): Category {
  if (e.isDir) return "folder";
  const ext = e.ext ?? (e.name.startsWith(".") ? null : null);
  if (ext && EXT[ext]) return EXT[ext];
  const lower = e.name.toLowerCase();
  if (lower === "dockerfile" || lower === "makefile" || lower.startsWith(".")) return "code";
  return "file";
}

const KIND_LABEL: Record<Category, string> = {
  folder: "Folder",
  image: "Image",
  video: "Video",
  audio: "Audio",
  pdf: "PDF document",
  doc: "Document",
  sheet: "Spreadsheet",
  slides: "Presentation",
  archive: "Archive",
  code: "Source code",
  text: "Text",
  font: "Font",
  disk: "Disk image",
  app: "Application",
  file: "File",
};

export function kindLabel(e: Pick<Entry, "isDir" | "ext" | "name">): string {
  const c = categoryOf(e);
  if (c === "file" && e.ext) return `${e.ext.toUpperCase()} file`;
  return KIND_LABEL[c];
}

export const PREVIEWABLE_IMAGE = new Set(["jpg", "jpeg", "png", "gif", "webp", "bmp", "ico", "svg", "avif"]);
export const THUMBNAILABLE = new Set(["jpg", "jpeg", "png", "gif", "webp", "bmp", "ico", "tif", "tiff"]);
export const PREVIEWABLE_VIDEO = new Set(["mp4", "webm", "mov", "m4v"]);
export const PREVIEWABLE_AUDIO = new Set(["mp3", "wav", "ogg", "m4a", "aac", "flac", "opus"]);

export type SortKey = "name" | "size" | "modified" | "kind";
export interface Sort {
  key: SortKey;
  dir: "asc" | "desc";
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** Folders first, then natural-order sort by the chosen key (name as tie-breaker). */
export function sortEntries(entries: Entry[], sort: Sort, foldersFirst = true): Entry[] {
  const mul = sort.dir === "asc" ? 1 : -1;
  return [...entries].sort((a, b) => {
    if (foldersFirst && a.isDir !== b.isDir) return a.isDir ? -1 : 1;
    let r = 0;
    switch (sort.key) {
      case "size":
        r = a.size - b.size;
        break;
      case "modified":
        r = (a.modified ?? 0) - (b.modified ?? 0);
        break;
      case "kind":
        r = collator.compare(kindLabel(a), kindLabel(b));
        break;
      default:
        r = 0;
    }
    if (r === 0) r = collator.compare(a.name, b.name);
    return r * mul;
  });
}

/** Proposes "untitled folder", "untitled folder 2"… not present in `names`. */
export function freshName(base: string, names: Set<string>): string {
  if (!names.has(base)) return base;
  for (let i = 2; ; i++) {
    const c = `${base} ${i}`;
    if (!names.has(c)) return c;
  }
}

/** Range of the file name to pre-select when renaming (stem without extension). */
export function stemRange(name: string, isDir: boolean): [number, number] {
  if (isDir) return [0, name.length];
  const dot = name.lastIndexOf(".");
  return dot > 0 ? [0, dot] : [0, name.length];
}
