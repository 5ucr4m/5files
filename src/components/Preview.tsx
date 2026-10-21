import { ChevronLeft, ChevronRight, Download, ExternalLink, Loader2, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { api, errorMessage } from "../lib/api";
import { PREVIEWABLE_AUDIO, PREVIEWABLE_IMAGE, PREVIEWABLE_VIDEO, categoryOf, formatBytes, formatDateFull, kindLabel } from "../lib/format";
import { openEntry, plural, selectedEntries } from "../lib/ops";
import type { Entry, FsId, TextPreview } from "../lib/types";
import { useSettings } from "../store/settings";
import { useActiveTab, useTabs } from "../store/tabs";
import { useUi } from "../store/ui";
import { folderSize, useSizes } from "../store/sizes";
import { measureFolders } from "../lib/sizes";
import { CountingDots } from "./CountingDots";
import { FileIcon } from "./FileIcon";
import { visibleEntries } from "./FileView";

type Kind = "image" | "video" | "audio" | "pdf" | "text" | "none";

function previewKind(e: Entry): Kind {
  if (e.isDir) return "none";
  const ext = e.ext ?? "";
  if (PREVIEWABLE_IMAGE.has(ext)) return "image";
  if (PREVIEWABLE_VIDEO.has(ext)) return "video";
  if (PREVIEWABLE_AUDIO.has(ext)) return "audio";
  if (ext === "pdf") return "pdf";
  const c = categoryOf(e);
  if (c === "code" || c === "text" || e.size < 64 * 1024) return "text";
  return "none";
}

/** Resolves a URL for media: asset protocol for local files, a data: URL for remote ones. */
function useMediaUrl(fs: FsId, e: Entry, kind: Kind) {
  const [url, setUrl] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    setUrl(null);
    setErr(null);
    if (kind === "none" || kind === "text") return;
    if (fs === "local") {
      setUrl(api().fileUrl(e.path));
      return;
    }
    let live = true;
    api()
      .readDataUrl(fs, e.path)
      .then((u) => live && setUrl(u))
      .catch((x) => live && setErr(errorMessage(x)));
    return () => {
      live = false;
    };
  }, [fs, e.path, kind]);
  return { url, err };
}

function useText(fs: FsId, e: Entry, enabled: boolean, max: number) {
  const [text, setText] = useState<TextPreview | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    setText(null);
    setErr(null);
    if (!enabled) return;
    let live = true;
    api()
      .readText(fs, e.path, max)
      .then((t) => live && setText(t))
      .catch((x) => live && setErr(errorMessage(x)));
    return () => {
      live = false;
    };
  }, [fs, e.path, e.modified, enabled, max]);
  return { text, err };
}

export function PreviewBody({ fs, entry, large = false }: { fs: FsId; entry: Entry; large?: boolean }) {
  const kind = previewKind(entry);
  const media = useMediaUrl(fs, entry, kind);
  const txt = useText(fs, entry, kind === "text", large ? 1024 * 1024 : 96 * 1024);
  const fallback = (
    <div className="flex h-full items-center justify-center">
      <FileIcon entry={entry} size={large ? 128 : 88} thumbs={fs === "local"} />
    </div>
  );
  if (media.err || txt.err) return fallback;
  switch (kind) {
    case "image":
      return media.url ? (
        <img src={media.url} alt={entry.name} className="h-full w-full object-contain" draggable={false} />
      ) : (
        <Spinner />
      );
    case "video":
      return media.url ? <video src={media.url} controls className="h-full w-full rounded-lg bg-black object-contain" /> : <Spinner />;
    case "audio":
      return (
        <div className="flex h-full flex-col items-center justify-center gap-4">
          <FileIcon entry={entry} size={large ? 120 : 72} />
          {media.url ? <audio src={media.url} controls className="w-full max-w-sm" /> : <Spinner />}
        </div>
      );
    case "pdf":
      return media.url ? <iframe src={media.url} title={entry.name} className="h-full w-full rounded-lg border-0 bg-white" /> : <Spinner />;
    case "text":
      if (!txt.text) return <Spinner />;
      if (txt.text.binary) return fallback;
      return (
        <pre className={`code-view scroll h-full overflow-auto rounded-lg bg-surface-2 p-3 whitespace-pre-wrap text-ink-2 ${large ? "text-[12.5px]" : "text-[11px]"}`}>
          {txt.text.text || <span className="text-ink-4">Empty file</span>}
          {txt.text.truncated && <span className="mt-2 block text-ink-4">… preview truncated ({formatBytes(txt.text.size)} total)</span>}
        </pre>
      );
    default:
      return fallback;
  }
}

function Spinner() {
  return (
    <div className="flex h-full items-center justify-center text-ink-4">
      <Loader2 size={18} className="animate-spin" />
    </div>
  );
}

function Meta({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex gap-3 py-1.5 text-[12px]">
      <dt className="w-[72px] shrink-0 text-ink-3">{label}</dt>
      <dd className="num min-w-0 flex-1 break-words text-ink-2">{value}</dd>
    </div>
  );
}

function FolderSize({ fs, path }: { fs: FsId; path: string }) {
  const size = useSizes((s) => folderSize(s.sizes, fs, path));
  if (size)
    return (
      <>
        {formatBytes(size.bytes)}
        {size.done ? ` · ${plural(size.files, "file")}` : <CountingDots />}
      </>
    );
  return (
    <button onClick={() => measureFolders(fs, [path], true)} className="font-sans font-medium text-accent hover:underline">
      Calculate
    </button>
  );
}

export function PreviewPane() {
  const tab = useActiveTab();
  const sel = selectedEntries(tab);
  const entry = sel.length === 1 ? sel[0] : null;
  return (
    <aside className="flex w-[300px] shrink-0 flex-col border-l border-line bg-surface">
      {!tab ? null : entry ? (
        <motion.div key={entry.path} initial={{ opacity: 0 }} animate={{ opacity: 1 }} transition={{ duration: 0.15 }} className="scroll flex min-h-0 flex-1 flex-col overflow-y-auto p-4">
          <div className="h-[220px] shrink-0">
            <PreviewBody fs={tab.fs} entry={entry} />
          </div>
          <h3 className="mt-4 text-[14px] font-semibold tracking-tight break-words">{entry.name}</h3>
          <p className="text-2xs text-ink-3">
            {kindLabel(entry)}
            {!entry.isDir && ` · ${formatBytes(entry.size)}`}
          </p>
          <dl className="mt-3 divide-y divide-line border-t border-line">
            {entry.isDir && <Meta label="Size" value={<FolderSize fs={tab.fs} path={entry.path} />} />}
            {!entry.isDir && <Meta label="Size" value={`${entry.size.toLocaleString()} bytes`} />}
            <Meta label="Modified" value={formatDateFull(entry.modified)} />
            {entry.created && <Meta label="Created" value={formatDateFull(entry.created)} />}
            <Meta label="Where" value={<span className="select-text">{entry.path}</span>} />
            {entry.readonly && <Meta label="Access" value="Read-only" />}
          </dl>
          <button onClick={() => openEntry(tab, entry)} className="mt-4 flex h-8 items-center justify-center gap-1.5 rounded-lg bg-surface-3 text-[12.5px] font-medium hover:bg-line">
            {tab.fs === "local" || entry.isDir ? <ExternalLink size={13} /> : <Download size={13} />}
            {entry.isDir ? "Open" : tab.fs === "local" ? "Open with default app" : "Download & open"}
          </button>
        </motion.div>
      ) : (
        <div className="flex flex-1 flex-col items-center justify-center p-6 text-center text-ink-3">
          {sel.length > 1 ? (
            <>
              <div className="relative mb-4 h-16 w-16">
                {sel.slice(0, 3).map((e, i) => (
                  <div key={e.path} className="absolute" style={{ left: i * 8, top: i * -4, transform: `rotate(${(i - 1) * 6}deg)` }}>
                    <FileIcon entry={e} size={52} />
                  </div>
                ))}
              </div>
              <p className="font-medium text-ink-2">{plural(sel.length, "item")} selected</p>
              <p className="num mt-0.5 text-2xs">{formatBytes(sel.reduce((t, e) => t + e.size, 0))} in files</p>
            </>
          ) : (
            <p className="text-2xs">Select a file to see a preview and its details.</p>
          )}
        </div>
      )}
    </aside>
  );
}

export function QuickLook() {
  const ql = useUi((s) => s.quickLook);
  const tab = useActiveTab();
  const showHidden = useSettings((s) => s.showHidden);
  const foldersFirst = useSettings((s) => s.foldersFirst);
  const close = () => useUi.getState().set({ quickLook: null });

  const step = (d: number) => {
    if (!tab || !ql) return;
    const list = visibleEntries(tab, showHidden, foldersFirst);
    const i = list.findIndex((e) => e.path === ql.entry.path);
    const next = list[Math.max(0, Math.min(list.length - 1, i + d))];
    if (next) {
      useUi.getState().set({ quickLook: { fs: ql.fs, entry: next } });
      useTabs.getState().select(tab.id, [next.path]);
    }
  };

  useEffect(() => {
    if (!ql) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" || e.key === " ") {
        e.preventDefault();
        e.stopPropagation();
        close();
      } else if (e.key === "ArrowRight" || e.key === "ArrowDown") {
        e.preventDefault();
        e.stopPropagation();
        step(1);
      } else if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
        e.preventDefault();
        e.stopPropagation();
        step(-1);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }); // re-bind each render so `step` sees the current list

  return (
    <AnimatePresence>
      {ql && (
        <motion.div className="fixed inset-0 z-[85] flex items-center justify-center p-10" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.16 }}>
          <div className="absolute inset-0 bg-black/45 backdrop-blur-sm" onMouseDown={close} />
          <motion.div
            initial={{ scale: 0.92, opacity: 0 }}
            animate={{ scale: 1, opacity: 1 }}
            exit={{ scale: 0.95, opacity: 0 }}
            transition={{ type: "spring", stiffness: 420, damping: 32 }}
            className="relative flex h-full max-h-[760px] w-full max-w-[1000px] flex-col overflow-hidden rounded-2xl border border-line bg-elevated shadow-pop"
          >
            <div className="flex h-11 shrink-0 items-center gap-2 border-b border-line px-3">
              <button onClick={() => step(-1)} className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-surface-3" aria-label="Previous">
                <ChevronLeft size={16} />
              </button>
              <button onClick={() => step(1)} className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-surface-3" aria-label="Next">
                <ChevronRight size={16} />
              </button>
              <div className="min-w-0 flex-1 text-center">
                <span className="truncate text-[13px] font-medium">{ql.entry.name}</span>
                <span className="num ml-2 text-2xs text-ink-3">{ql.entry.isDir ? "Folder" : formatBytes(ql.entry.size)}</span>
              </div>
              <button onClick={close} className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-surface-3" aria-label="Close">
                <X size={15} />
              </button>
            </div>
            <div className="min-h-0 flex-1 p-4">
              <PreviewBody key={ql.entry.path} fs={ql.fs} entry={ql.entry} large />
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
