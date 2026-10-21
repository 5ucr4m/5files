import {
  AppWindow,
  Archive,
  Code2,
  Disc3,
  File,
  FileSpreadsheet,
  FileText,
  Film,
  Image,
  Music2,
  Presentation,
  Type,
  type LucideIcon,
} from "lucide-react";
import { memo, useState } from "react";
import { api } from "../lib/api";
import { THUMBNAILABLE, categoryOf, type Category } from "../lib/format";
import type { Entry } from "../lib/types";

const META: Record<Exclude<Category, "folder">, { icon: LucideIcon; hue: string }> = {
  image: { icon: Image, hue: "#e0567f" },
  video: { icon: Film, hue: "#8b5cf6" },
  audio: { icon: Music2, hue: "#f08a24" },
  pdf: { icon: FileText, hue: "#e5484d" },
  doc: { icon: FileText, hue: "#2f6fec" },
  sheet: { icon: FileSpreadsheet, hue: "#1a9a5c" },
  slides: { icon: Presentation, hue: "#e8742b" },
  archive: { icon: Archive, hue: "#a07a3c" },
  code: { icon: Code2, hue: "#0f9fa8" },
  text: { icon: FileText, hue: "#7a8291" },
  font: { icon: Type, hue: "#c2417a" },
  disk: { icon: Disc3, hue: "#5c6b80" },
  app: { icon: AppWindow, hue: "#3d63dd" },
  file: { icon: File, hue: "#8a919e" },
};

export function categoryHue(c: Category) {
  return c === "folder" ? "var(--accent)" : META[c].hue;
}

/** A folder silhouette tinted with the accent (or a drive color). */
export function FolderGlyph({ size, tint = "var(--accent)", open = false }: { size: number; tint?: string; open?: boolean }) {
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" aria-hidden className="shrink-0">
      <path d="M3 8.5A2.5 2.5 0 0 1 5.5 6h6.4c.7 0 1.3.3 1.8.8L15.5 9H26.5A2.5 2.5 0 0 1 29 11.5V24a2.5 2.5 0 0 1-2.5 2.5h-21A2.5 2.5 0 0 1 3 24Z" fill={tint} opacity={0.42} />
      <path
        d={open ? "M3 13.5A2.5 2.5 0 0 1 5.5 11h22.3a1.5 1.5 0 0 1 1.45 1.9l-2.9 11.3A3 3 0 0 1 23.45 26.5H5.5A2.5 2.5 0 0 1 3 24Z" : "M3 12.5A2.5 2.5 0 0 1 5.5 10h21A2.5 2.5 0 0 1 29 12.5V24a2.5 2.5 0 0 1-2.5 2.5h-21A2.5 2.5 0 0 1 3 24Z"}
        fill={tint}
      />
      <path d="M3 12.5A2.5 2.5 0 0 1 5.5 10h21A2.5 2.5 0 0 1 29 12.5v.6H3Z" fill="#fff" opacity={0.18} />
    </svg>
  );
}

function SheetGlyph({ size, cat, ext }: { size: number; cat: Exclude<Category, "folder">; ext: string | null }) {
  const { icon: Icon, hue } = META[cat];
  const showExt = size >= 40 && ext && ext.length <= 5;
  return (
    <div className="relative shrink-0" style={{ width: size, height: size }}>
      <svg width={size} height={size} viewBox="0 0 32 32" aria-hidden className="absolute inset-0">
        <path d="M8 3h11l7 7v17a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2Z" fill="var(--surface)" stroke="var(--line-strong)" strokeWidth={1} />
        <path d="M19 3v5a2 2 0 0 0 2 2h5" fill="none" stroke="var(--line-strong)" strokeWidth={1} />
        <path d="M8 3h11l7 7v17a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2Z" fill={hue} opacity={0.08} />
      </svg>
      <div className="absolute inset-0 flex flex-col items-center justify-center" style={{ paddingTop: size * 0.12 }}>
        <Icon size={Math.max(10, size * 0.38)} color={hue} strokeWidth={size > 30 ? 1.8 : 2.1} />
        {showExt && (
          <span className="num mt-0.5 font-semibold uppercase" style={{ fontSize: Math.max(7, size * 0.13), color: hue, letterSpacing: "0.04em" }}>
            {ext}
          </span>
        )}
      </div>
    </div>
  );
}

interface Props {
  entry: Entry;
  size: number;
  thumbs?: boolean;
  open?: boolean;
}

export const FileIcon = memo(function FileIcon({ entry, size, thumbs = false, open }: Props) {
  const cat = categoryOf(entry);
  const [failed, setFailed] = useState(false);
  if (cat === "folder") return <FolderGlyph size={size} open={open} />;
  if (thumbs && !failed && entry.ext && THUMBNAILABLE.has(entry.ext) && size >= 32) {
    // Request a 2x thumbnail for crisp HiDPI rendering.
    const px = Math.min(1024, Math.round(size * 2));
    return (
      <div className="flex shrink-0 items-center justify-center" style={{ width: size, height: size }}>
        <img
          src={api().thumbUrl(entry.path, px)}
          alt=""
          loading="lazy"
          decoding="async"
          draggable={false}
          onError={() => setFailed(true)}
          className="max-h-full max-w-full rounded-[5px] object-contain shadow-[0_1px_3px_rgb(0_0_0/0.18)] ring-1 ring-black/5"
        />
      </div>
    );
  }
  return <SheetGlyph size={size} cat={cat} ext={entry.ext} />;
});
