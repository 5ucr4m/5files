import {
  ArrowLeft,
  ArrowRightLeft,
  ChevronRight,
  Cloud,
  Copy,
  FolderOpen,
  HardDrive,
  Loader2,
  Pause,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Server,
  ShieldCheck,
  Trash2,
  TriangleAlert,
  X,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useState } from "react";
import { api, errorMessage } from "../lib/api";
import { formatBytes } from "../lib/format";
import { fsName, plural } from "../lib/ops";
import type { Entry, FsId, Loc, SyncMode, SyncOp, SyncPair, SyncPreview } from "../lib/types";
import { useLayout } from "../store/layout";
import { useDrives, useSyncs } from "../store/live";
import { activeTab, useTabs } from "../store/tabs";
import { confirmDialog, toast, useUi } from "../store/ui";
import { FolderGlyph } from "./FileIcon";

// ---------------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------------

/** Folder name of a path; "" for a drive's root (callers show the drive name then). */
const lastSeg = (p: string) => p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? "";

export function relativeTime(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86_400) return `${Math.round(s / 3600)} h ago`;
  return new Date(ms).toLocaleDateString(undefined, { day: "numeric", month: "short" });
}

/** One line describing what a sync is doing right now. */
export function syncStatus(p: SyncPair): { text: string; tone: "ok" | "busy" | "warn" | "error" | "muted" } {
  switch (p.state) {
    case "paused":
      return { text: "Paused", tone: "muted" };
    case "scanning":
      return { text: "Checking for changes…", tone: "busy" };
    case "syncing":
      return { text: `Syncing ${p.done.toLocaleString()} of ${plural(p.total, "change")}`, tone: "busy" };
    case "review":
      return { text: `On hold: would delete ${plural(p.summary.deletedFiles, "file")}`, tone: "warn" };
    case "error":
      return { text: p.message, tone: "error" };
    default:
      if (!p.lastRun) return { text: "Waiting for the first sync", tone: "muted" };
      if (!p.lastRun.ok) return { text: p.lastRun.error ?? "Last sync had errors", tone: "error" };
      return { text: `Synced ${relativeTime(p.lastRun.at)}`, tone: "ok" };
  }
}

const TONE: Record<ReturnType<typeof syncStatus>["tone"], string> = {
  ok: "var(--success)",
  busy: "var(--accent)",
  warn: "var(--warning)",
  error: "var(--danger)",
  muted: "var(--ink-4)",
};

function DriveIcon({ fs, size = 14 }: { fs: FsId; size?: number }) {
  const conn = useDrives((s) => s.connections.find((c) => c.id === fs));
  if (fs === "local") return <HardDrive size={size} className="text-ink-3" />;
  const Icon = conn?.kind === "sftp" || conn?.kind === "ftp" ? Server : Cloud;
  return <Icon size={size} style={{ color: conn?.color ?? "var(--accent)" }} />;
}

/** A folder end of the pipe: drive, folder name, full path. */
function Endpoint({ loc, label, empty }: { loc: Loc | null; label: string; empty: string }) {
  return (
    <div className="min-w-0 flex-1 rounded-xl border border-line bg-surface px-3 py-2.5">
      <div className="mb-1 text-[10.5px] font-semibold tracking-[0.08em] text-ink-4 uppercase">{label}</div>
      {loc ? (
        <div className="flex min-w-0 items-center gap-2">
          <FolderGlyph size={22} />
          <div className="min-w-0">
            <div className="truncate text-[13px] font-medium">{lastSeg(loc.path) || fsName(loc.fs)}</div>
            <div className="flex min-w-0 items-center gap-1 text-2xs text-ink-3">
              <DriveIcon fs={loc.fs} size={11} />
              <span className="truncate" title={loc.path}>
                {fsName(loc.fs)} · <span className="num">{loc.path}</span>
              </span>
            </div>
          </div>
        </div>
      ) : (
        <div className="text-[12.5px] text-ink-4">{empty}</div>
      )}
    </div>
  );
}

/**
 * The signature: two folders joined by a pipe; dots travel in the sync's direction
 * (both lanes when two-way), and a badge says what happens to deletions.
 */
export function FlowDiagram({
  source,
  dest,
  mode,
  twoWay,
  flowing = true,
  compact = false,
}: {
  source: Loc | null;
  dest: Loc | null;
  mode: SyncMode;
  twoWay: boolean;
  flowing?: boolean;
  compact?: boolean;
}) {
  const lane = (dir: "fwd" | "back") => (
    <div className="relative h-[3px] w-full overflow-hidden rounded-full bg-line">
      {[0, 1, 2, 3].map((i) => (
        <span
          key={i}
          className={`sync-dot ${dir === "back" ? "sync-dot-back" : ""} ${flowing ? "" : "sync-dot-still"}`}
          style={{
            animationDelay: `${i * 0.35}s`,
            background: dir === "fwd" ? "var(--trace-a)" : "var(--trace-b)",
            color: dir === "fwd" ? "var(--trace-a)" : "var(--trace-b)",
            left: flowing ? undefined : `${12 + i * 24}%`,
          }}
        />
      ))}
    </div>
  );
  return (
    <div className={`flex items-center ${compact ? "gap-2" : "gap-3"}`}>
      <Endpoint loc={source} label="From" empty="Choose a folder" />
      <div className={`flex shrink-0 flex-col items-center gap-1.5 ${compact ? "w-[86px]" : "w-[132px]"}`}>
        <div className="flex w-full flex-col gap-[5px]">
          {lane("fwd")}
          <AnimatePresence initial={false}>
            {twoWay && (
              <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }}>
                {lane("back")}
              </motion.div>
            )}
          </AnimatePresence>
        </div>
        <motion.span
          key={mode}
          initial={{ opacity: 0, y: 3 }}
          animate={{ opacity: 1, y: 0 }}
          className={`flex items-center gap-1 rounded-full px-2 py-0.5 text-[10.5px] font-medium whitespace-nowrap ${
            mode === "mirror" ? "bg-[color-mix(in_oklab,var(--warning)_14%,transparent)] text-warning" : "bg-[color-mix(in_oklab,var(--success)_14%,transparent)] text-success"
          }`}
        >
          {mode === "mirror" ? <Copy size={10} /> : <ShieldCheck size={10} />}
          {mode === "mirror" ? "Deletes mirrored" : "Nothing deleted"}
        </motion.span>
      </div>
      <Endpoint loc={dest} label="To" empty="Choose a folder" />
    </div>
  );
}

// ---------------------------------------------------------------------------------
// Folder picker (any drive)
// ---------------------------------------------------------------------------------

function FolderPicker({ initial, onPick, onCancel }: { initial: Loc | null; onPick: (l: Loc) => void; onCancel: () => void }) {
  const connections = useDrives((s) => s.connections);
  const places = useDrives((s) => s.places);
  const [fs, setFs] = useState<FsId>(initial?.fs ?? "local");
  const [path, setPath] = useState<string>(initial?.path ?? "");
  const [dirs, setDirs] = useState<Entry[] | null>(null);
  const [crumbs, setCrumbs] = useState<{ name: string; path: string }[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setDirs(null);
    setError(null);
    api()
      .listDir(fs, path || null)
      .then((l) => {
        if (!live) return;
        setPath(l.path);
        setCrumbs(l.crumbs);
        setDirs(l.entries.filter((e) => e.isDir && !e.hidden).sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true })));
      })
      .catch((e) => live && setError(errorMessage(e)));
    return () => {
      live = false;
    };
  }, [fs, path]);

  const here = activeTab();
  return (
    <motion.div
      initial={{ opacity: 0, height: 0 }}
      animate={{ opacity: 1, height: "auto" }}
      exit={{ opacity: 0, height: 0 }}
      className="overflow-hidden"
    >
      <div className="mt-2 rounded-xl border border-line bg-surface-2/60 p-2">
        <div className="mb-2 flex flex-wrap items-center gap-1">
          {[{ id: "local", name: "This computer" }, ...connections.map((c) => ({ id: c.id, name: c.name }))].map((d) => (
            <button
              key={d.id}
              onClick={() => {
                setFs(d.id);
                setPath("");
              }}
              className={`flex items-center gap-1.5 rounded-lg px-2 py-1 text-[12px] ${fs === d.id ? "bg-surface text-ink shadow-soft" : "text-ink-3 hover:text-ink"}`}
            >
              <DriveIcon fs={d.id} size={12} />
              {d.name}
            </button>
          ))}
          <span className="flex-1" />
          {here && (
            <button onClick={() => onPick({ fs: here.fs, path: here.path })} className="rounded-lg px-2 py-1 text-[12px] font-medium text-accent hover:bg-sel" title={here.path}>
              Use current folder
            </button>
          )}
        </div>
        {fs === "local" && (
          <div className="mb-1.5 flex flex-wrap gap-1">
            {places.map((p) => (
              <button key={p.id} onClick={() => setPath(p.path)} className="rounded-md bg-surface px-1.5 py-0.5 text-2xs text-ink-2 hover:text-ink">
                {p.name}
              </button>
            ))}
          </div>
        )}
        <div className="mb-1 flex min-w-0 items-center gap-0.5 overflow-x-auto text-[12px] [scrollbar-width:none]">
          {crumbs.map((c, i) => (
            <span key={c.path} className="flex shrink-0 items-center">
              {i > 0 && <ChevronRight size={12} className="text-ink-4" />}
              <button onClick={() => setPath(c.path)} className="rounded px-1 text-ink-3 hover:text-ink">
                {c.name || c.path}
              </button>
            </span>
          ))}
        </div>
        <div className="scroll h-[168px] overflow-y-auto rounded-lg bg-surface">
          {error ? (
            <p className="p-3 text-2xs text-danger">{error}</p>
          ) : !dirs ? (
            <div className="flex h-full items-center justify-center text-ink-4">
              <Loader2 size={15} className="animate-spin" />
            </div>
          ) : dirs.length === 0 ? (
            <p className="p-3 text-2xs text-ink-3">No folders here. You can pick this folder.</p>
          ) : (
            dirs.map((d) => (
              <button key={d.path} onDoubleClick={() => setPath(d.path)} onClick={() => setPath(d.path)} className="flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-[12.5px] hover:bg-surface-2">
                <FolderGlyph size={16} />
                <span className="truncate">{d.name}</span>
              </button>
            ))
          )}
        </div>
        <div className="mt-2 flex items-center justify-end gap-2">
          <span className="num min-w-0 flex-1 truncate text-2xs text-ink-3" title={path}>
            {path}
          </span>
          <button onClick={onCancel} className="h-7 rounded-lg px-2.5 text-[12px] text-ink-2 hover:bg-surface-3">
            Cancel
          </button>
          <button onClick={() => onPick({ fs, path })} disabled={!path} className="h-7 rounded-lg bg-accent px-2.5 text-[12px] font-medium text-white disabled:opacity-40">
            Choose this folder
          </button>
        </div>
      </div>
    </motion.div>
  );
}

// ---------------------------------------------------------------------------------
// Editor: settings → preview → start
// ---------------------------------------------------------------------------------

const MODES: { value: SyncMode; title: string; body: string; icon: typeof Copy }[] = [
  { value: "mirror", title: "Mirror", body: "The destination becomes an exact copy. Deleting something in the source deletes it there too.", icon: Copy },
  { value: "keepDeleted", title: "Mirror, keep deleted", body: "Everything is copied, and nothing is ever deleted from the destination.", icon: ShieldCheck },
];

function OpLine({ op }: { op: SyncOp }) {
  const [icon, text, color] =
    op.op === "delete"
      ? [<Trash2 size={12} />, `Delete from ${op.on === "dest" ? "destination" : "source"}`, "var(--danger)"]
      : op.op === "conflict"
        ? [<TriangleAlert size={12} />, "Changed on both sides: newest kept, other saved as a copy", "var(--warning)"]
        : op.op === "mkDir"
          ? [<FolderOpen size={12} />, `New folder in ${op.to === "dest" ? "destination" : "source"}`, "var(--ink-3)"]
          : [<Copy size={12} />, op.to === "dest" ? "Copy to destination" : "Copy back to source", "var(--accent)"];
  return (
    <div className="flex items-center gap-2 py-1 text-[12px]">
      <span style={{ color }}>{icon}</span>
      <span className="num min-w-0 flex-1 truncate">{op.rel}</span>
      <span className="shrink-0 text-2xs text-ink-4">{text}</span>
    </div>
  );
}

function Editor({ existing, onDone }: { existing?: SyncPair; onDone: (id?: string) => void }) {
  const [name, setName] = useState(existing?.name ?? "");
  const [source, setSource] = useState<Loc | null>(existing?.source ?? null);
  const [dest, setDest] = useState<Loc | null>(existing?.dest ?? null);
  const [mode, setMode] = useState<SyncMode>(existing?.mode ?? "mirror");
  const [twoWay, setTwoWay] = useState(existing?.twoWay ?? false);
  const [picking, setPicking] = useState<"source" | "dest" | null>(existing ? null : "source");
  const [preview, setPreview] = useState<SyncPreview | null>(null);
  const [busy, setBusy] = useState<null | "preview" | "save">(null);
  const [error, setError] = useState<string | null>(null);
  const autoName = source && dest ? `${lastSeg(source.path) || fsName(source.fs)} → ${lastSeg(dest.path) || fsName(dest.fs)}` : "";
  const input = () => ({ id: existing?.id ?? null, name: name.trim() || autoName, source: source!, dest: dest!, mode, twoWay });
  // Any change invalidates the preview.
  useEffect(() => setPreview(null), [source, dest, mode, twoWay]);

  const review = async () => {
    setBusy("preview");
    setError(null);
    try {
      setPreview(await api().syncPreview(input()));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };
  const start = async () => {
    setBusy("save");
    setError(null);
    try {
      const p = await api().syncSave(input());
      toast({ tone: "success", title: existing ? `Saved ${p.name}` : `${p.name} is syncing`, body: existing ? undefined : "Changes are kept in step automatically from now on." });
      onDone(p.id);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };
  const s = preview?.summary;
  const destIsLocal = dest?.fs === "local";

  return (
    <div className="flex h-full flex-col">
      <div className="scroll flex-1 overflow-y-auto px-6 pt-5 pb-6">
        <FlowDiagram source={source} dest={dest} mode={mode} twoWay={twoWay} />

        <div className="mt-5 grid grid-cols-2 gap-3">
          {(["source", "dest"] as const).map((which) => {
            const loc = which === "source" ? source : dest;
            return (
              <div key={which} className="col-span-2">
                <div className="flex items-center gap-2">
                  <span className="w-[86px] shrink-0 text-[12px] font-medium text-ink-2">{which === "source" ? "Sync from" : "Into"}</span>
                  <button
                    onClick={() => setPicking(picking === which ? null : which)}
                    className={`flex h-9 min-w-0 flex-1 items-center gap-2 rounded-lg border px-2.5 text-left text-[12.5px] transition-colors ${picking === which ? "border-accent bg-surface" : "border-line bg-surface-2/70 hover:border-line-strong"}`}
                  >
                    {loc ? <DriveIcon fs={loc.fs} size={13} /> : <FolderOpen size={13} className="text-ink-4" />}
                    <span className={`num min-w-0 flex-1 truncate ${loc ? "" : "text-ink-4"}`}>{loc ? `${fsName(loc.fs)} · ${loc.path}` : "Choose a folder…"}</span>
                    <ChevronRight size={13} className={`text-ink-4 transition-transform ${picking === which ? "rotate-90" : ""}`} />
                  </button>
                </div>
                <AnimatePresence>
                  {picking === which && (
                    <FolderPicker
                      initial={loc}
                      onCancel={() => setPicking(null)}
                      onPick={(l) => {
                        if (which === "source") setSource(l);
                        else setDest(l);
                        setPicking(which === "source" && !dest ? "dest" : null);
                      }}
                    />
                  )}
                </AnimatePresence>
              </div>
            );
          })}
        </div>

        <div className="mt-5 grid grid-cols-2 gap-2.5" role="radiogroup" aria-label="Sync mode">
          {MODES.map((m) => (
            <button
              key={m.value}
              role="radio"
              aria-checked={mode === m.value}
              onClick={() => setMode(m.value)}
              className={`flex flex-col items-start gap-1.5 rounded-xl border p-3 text-left transition-colors ${mode === m.value ? "border-accent bg-sel" : "border-line hover:bg-surface-2"}`}
            >
              <span className="flex items-center gap-2 text-[13px] font-semibold">
                <m.icon size={14} className={mode === m.value ? "text-accent" : "text-ink-3"} />
                {m.title}
              </span>
              <span className="text-2xs leading-snug text-ink-3">{m.body}</span>
            </button>
          ))}
        </div>

        <button role="switch" aria-checked={twoWay} onClick={() => setTwoWay(!twoWay)} className="mt-3 flex w-full items-center gap-3 rounded-xl border border-line p-3 text-left hover:bg-surface-2">
          <ArrowRightLeft size={16} className={twoWay ? "text-accent" : "text-ink-3"} />
          <span className="min-w-0 flex-1">
            <span className="block text-[13px] font-medium">Sync both ways</span>
            <span className="block text-2xs text-ink-3">
              Changes made in the destination go back to the source too.
              {twoWay && mode === "mirror" && " Deleting on either side deletes on the other."}
              {twoWay && mode === "keepDeleted" && " Deletions stay where they happened."}
            </span>
          </span>
          <span className={`relative h-[20px] w-[36px] shrink-0 rounded-full transition-colors ${twoWay ? "bg-accent" : "bg-line-strong"}`}>
            <motion.span className="absolute top-[2px] size-[16px] rounded-full bg-white shadow" animate={{ left: twoWay ? 18 : 2 }} transition={{ type: "spring", stiffness: 700, damping: 35 }} />
          </span>
        </button>

        <label className="mt-3 flex items-center gap-2">
          <span className="w-[86px] shrink-0 text-[12px] font-medium text-ink-2">Name</span>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={autoName || "e.g. Photos backup"}
            className="h-9 min-w-0 flex-1 rounded-lg border border-line bg-surface-2/70 px-2.5 text-[12.5px] outline-none placeholder:text-ink-4 focus:border-accent focus:bg-surface"
          />
        </label>

        <AnimatePresence>
          {s && preview && (
            <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} className="mt-5 rounded-xl border border-line bg-surface-2/60 p-3.5">
              <p className="text-[13px] font-semibold">The first sync will</p>
              <div className="num mt-2 grid grid-cols-2 gap-x-4 gap-y-1 text-[12.5px]">
                <span>
                  Copy <b>{plural(s.copies - s.toSource, "file")}</b> · {formatBytes(s.copyBytes)}
                </span>
                {s.toSource > 0 && (
                  <span>
                    Copy <b>{s.toSource}</b> back to the source
                  </span>
                )}
                <span className={s.deletes ? "text-danger" : "text-ink-3"}>
                  {s.deletes ? (
                    <>
                      Delete <b>{plural(s.deletedFiles, "file")}</b>
                      {destIsLocal ? " (to the Trash)" : ""}
                    </>
                  ) : (
                    "Delete nothing"
                  )}
                </span>
                {s.conflicts > 0 && <span className="text-warning">{plural(s.conflicts, "conflict")}: newest wins, other kept as a copy</span>}
                {s.copies + s.deletes + s.conflicts + s.mkdirs === 0 && <span className="text-success">Nothing to do: both folders already match</span>}
              </div>
              {preview.needsReview && (
                <p className="mt-2 flex items-start gap-1.5 rounded-lg bg-[color-mix(in_oklab,var(--warning)_12%,transparent)] p-2 text-2xs text-warning">
                  <TriangleAlert size={13} className="mt-px shrink-0" /> That's a large share of the destination. Double-check that you picked the right folders.
                </p>
              )}
              {preview.sample.length > 0 && (
                <div className="scroll mt-2 max-h-[150px] overflow-y-auto border-t border-line pt-1.5">
                  {preview.sample.slice(0, 40).map((op, i) => (
                    <OpLine key={i} op={op} />
                  ))}
                </div>
              )}
            </motion.div>
          )}
        </AnimatePresence>
        {error && <p className="mt-3 rounded-lg bg-[color-mix(in_oklab,var(--danger)_10%,transparent)] p-2.5 text-[12px] text-danger">{error}</p>}
      </div>

      <div className="flex shrink-0 items-center justify-end gap-2 border-t border-line px-6 py-3">
        <span className="flex-1 text-2xs text-ink-4">{!source || !dest ? "Choose both folders to continue." : preview ? "Looks right? Start keeps them in step from now on." : "Review shows exactly what the first sync will do."}</span>
        {!preview ? (
          <button
            onClick={review}
            disabled={!source || !dest || !!busy}
            className="flex h-8 items-center gap-1.5 rounded-lg bg-accent px-3.5 text-[12.5px] font-medium text-white hover:brightness-110 disabled:opacity-40"
          >
            {busy === "preview" && <Loader2 size={13} className="animate-spin" />} Review
          </button>
        ) : (
          <button onClick={start} disabled={!!busy} className="flex h-8 items-center gap-1.5 rounded-lg bg-accent px-3.5 text-[12.5px] font-medium text-white hover:brightness-110 disabled:opacity-40">
            {busy === "save" ? <Loader2 size={13} className="animate-spin" /> : <Play size={13} />} {existing ? "Save and sync" : "Start syncing"}
          </button>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------------
// Detail
// ---------------------------------------------------------------------------------

function openBoth(p: SyncPair) {
  useUi.getState().set({ panel: null, syncView: null });
  const tabs = useTabs.getState();
  tabs.newTab(p.source);
  // Let the new tab mount before splitting it.
  setTimeout(() => {
    useLayout.getState().split("row");
    const right = activeTab();
    if (right) void useTabs.getState().navigate(right.id, p.dest);
  }, 60);
}

function Detail({ pair, onEdit }: { pair: SyncPair; onEdit: () => void }) {
  const st = syncStatus(pair);
  const busy = pair.state === "syncing" || pair.state === "scanning";
  const run = pair.lastRun;
  const act = (f: () => Promise<unknown>) => f().catch((e) => toast({ tone: "error", title: "Couldn't do that", body: errorMessage(e) }));
  const remove = async () => {
    const ok = await confirmDialog({ title: `Stop and remove “${pair.name}”?`, body: "Files stay where they are in both folders. Only the sync is removed.", confirm: "Remove sync", danger: true });
    if (!ok) return;
    await act(() => api().syncDelete(pair.id));
    useUi.getState().set({ syncView: null });
  };
  return (
    <div className="scroll h-full overflow-y-auto px-6 pt-5 pb-6">
      <FlowDiagram source={pair.source} dest={pair.dest} mode={pair.mode} twoWay={pair.twoWay} flowing={busy} />

      <div className="mt-4 flex items-center gap-2 text-[12.5px]">
        <span className={`size-2 shrink-0 rounded-full ${busy ? "animate-pulse" : ""}`} style={{ background: TONE[st.tone] }} />
        <span className="min-w-0 flex-1 truncate" style={{ color: st.tone === "error" ? "var(--danger)" : undefined }} title={st.text}>
          {st.text}
        </span>
      </div>
      {pair.state === "syncing" && (
        <div className="mt-2">
          <div className="relative h-[5px] overflow-hidden rounded-full bg-surface-3">
            <motion.div className="stripes absolute inset-y-0 left-0 rounded-full" style={{ backgroundColor: "var(--accent)" }} animate={{ width: `${(pair.done / Math.max(1, pair.total)) * 100}%` }} />
          </div>
          <p className="num mt-1 truncate text-2xs text-ink-3">
            {formatBytes(pair.bytes)} of {formatBytes(pair.totalBytes)}
            {pair.current && <span className="text-ink-4"> · {pair.current}</span>}
          </p>
        </div>
      )}

      {pair.state === "review" && (
        <div className="mt-4 rounded-xl border border-[color-mix(in_oklab,var(--warning)_40%,transparent)] bg-[color-mix(in_oklab,var(--warning)_8%,transparent)] p-3.5">
          <p className="flex items-center gap-2 text-[13px] font-semibold text-warning">
            <TriangleAlert size={15} /> This sync is on hold
          </p>
          <p className="mt-1 text-[12px] text-ink-2">
            The next run would delete {plural(pair.summary.deletedFiles, "file")}. That often means the source is temporarily empty — an unplugged drive or an expired login. Check
            the source before continuing.
          </p>
          <div className="mt-3 flex gap-2">
            <button onClick={() => act(() => api().syncRun(pair.id, true))} className="h-8 rounded-lg bg-danger px-3 text-[12.5px] font-medium text-white hover:brightness-110">
              Delete and sync
            </button>
            <button onClick={() => act(() => api().syncPause(pair.id, true))} className="h-8 rounded-lg border border-line px-3 text-[12.5px] font-medium hover:bg-surface-2">
              Pause this sync
            </button>
          </div>
        </div>
      )}

      {run && (
        <div className="mt-5">
          <p className="mb-2 text-[11px] font-semibold tracking-[0.08em] text-ink-4 uppercase">Last sync · {relativeTime(run.at)}</p>
          <div className="num grid grid-cols-4 gap-2">
            {[
              ["Copied", run.copied.toLocaleString()],
              ["Deleted", run.deleted.toLocaleString()],
              ["Conflicts", run.conflicts.toLocaleString()],
              ["Moved", formatBytes(run.bytes)],
            ].map(([k, v]) => (
              <div key={k} className="rounded-lg bg-surface-2 px-2.5 py-2">
                <div className="text-2xs text-ink-3">{k}</div>
                <div className="text-[13px] font-medium">{v}</div>
              </div>
            ))}
          </div>
          {run.failed > 0 && <p className="mt-2 text-2xs text-danger">{run.error}</p>}
        </div>
      )}

      <div className="mt-6 flex flex-wrap gap-2">
        <button
          onClick={() => act(() => api().syncRun(pair.id, false))}
          disabled={busy || pair.paused}
          className="flex h-8 items-center gap-1.5 rounded-lg bg-accent px-3 text-[12.5px] font-medium text-white hover:brightness-110 disabled:opacity-40"
        >
          <RefreshCw size={13} className={busy ? "animate-spin" : ""} /> Sync now
        </button>
        <button onClick={() => act(() => api().syncPause(pair.id, !pair.paused))} className="flex h-8 items-center gap-1.5 rounded-lg border border-line px-3 text-[12.5px] font-medium hover:bg-surface-2">
          {pair.paused ? <Play size={13} /> : <Pause size={13} />} {pair.paused ? "Resume" : "Pause"}
        </button>
        <button onClick={() => openBoth(pair)} className="flex h-8 items-center gap-1.5 rounded-lg border border-line px-3 text-[12.5px] font-medium hover:bg-surface-2">
          <FolderOpen size={13} /> Open both side by side
        </button>
        <button onClick={onEdit} className="flex h-8 items-center gap-1.5 rounded-lg border border-line px-3 text-[12.5px] font-medium hover:bg-surface-2">
          <Pencil size={13} /> Edit
        </button>
        <span className="flex-1" />
        <button onClick={remove} className="flex h-8 items-center gap-1.5 rounded-lg px-3 text-[12.5px] font-medium text-danger hover:bg-[color-mix(in_oklab,var(--danger)_10%,transparent)]">
          <Trash2 size={13} /> Remove
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------------

function List({ onOpen, onNew }: { onOpen: (id: string) => void; onNew: () => void }) {
  const syncs = useSyncs((s) => s.syncs);
  if (!syncs.length)
    return (
      <div className="flex h-full flex-col items-center justify-center px-10 text-center">
        <div className="w-full max-w-[420px] opacity-90">
          <FlowDiagram source={{ fs: "local", path: "Pictures" }} dest={{ fs: "local", path: "Backup" }} mode="keepDeleted" twoWay={false} compact />
        </div>
        <p className="mt-6 text-[14px] font-semibold">Keep two folders in step</p>
        <p className="mt-1 max-w-[380px] text-[12.5px] text-ink-3">
          Pick a folder and where it should go — another disk, a server or the cloud. 5files copies every change automatically, one way or both ways.
        </p>
        <button onClick={onNew} className="mt-5 flex h-9 items-center gap-1.5 rounded-lg bg-accent px-4 text-[13px] font-medium text-white hover:brightness-110">
          <Plus size={15} /> New sync
        </button>
      </div>
    );
  return (
    <div className="scroll h-full overflow-y-auto p-5">
      <div className="space-y-2">
        {syncs.map((p) => {
          const st = syncStatus(p);
          return (
            <button key={p.id} onClick={() => onOpen(p.id)} className="flex w-full items-center gap-3 rounded-xl border border-line bg-surface p-3 text-left hover:bg-surface-2">
              <SyncGlyph pair={p} size={32} />
              <span className="min-w-0 flex-1">
                <span className="block truncate text-[13px] font-medium">{p.name}</span>
                <span className="num block truncate text-2xs text-ink-3">
                  {lastSeg(p.source.path) || fsName(p.source.fs)} {p.twoWay ? "⇄" : "→"} {fsName(p.dest.fs)}
                  {lastSeg(p.dest.path) && ` · ${lastSeg(p.dest.path)}`}
                </span>
              </span>
              <span className="max-w-[180px] truncate text-2xs" style={{ color: TONE[st.tone] }}>
                {st.text}
              </span>
            </button>
          );
        })}
      </div>
      <button onClick={onNew} className="mt-3 flex w-full items-center justify-center gap-1.5 rounded-xl border border-dashed border-line-strong p-3 text-[12.5px] text-ink-3 hover:border-accent hover:text-ink">
        <Plus size={14} /> New sync
      </button>
    </div>
  );
}

/** Small mode glyph with a live status ring (used in the sidebar and lists). */
export function SyncGlyph({ pair, size = 16 }: { pair: SyncPair; size?: number }) {
  const st = syncStatus(pair);
  const busy = st.tone === "busy";
  return (
    <span className="relative flex shrink-0 items-center justify-center" style={{ width: size, height: size }}>
      {pair.twoWay ? (
        <ArrowRightLeft size={size * 0.82} className={busy ? "text-accent" : "text-ink-3"} />
      ) : (
        <RefreshCw size={size * 0.78} className={busy ? "animate-spin text-accent [animation-duration:1.6s]" : "text-ink-3"} />
      )}
      <span className="absolute -right-0.5 -bottom-0.5 size-[7px] rounded-full border border-bg" style={{ background: TONE[st.tone] }} />
    </span>
  );
}

export function SyncPanel() {
  const view = useUi((s) => s.syncView);
  const syncs = useSyncs((s) => s.syncs);
  const [editing, setEditing] = useState(false);
  const pair = useMemo(() => (view && view !== "new" ? syncs.find((s) => s.id === view) : undefined), [view, syncs]);
  useEffect(() => setEditing(false), [view]);
  const go = (v: string | null) => useUi.getState().set({ syncView: v });
  const close = () => useUi.getState().set({ panel: null, syncView: null });
  const screen = view === "new" ? "new" : pair ? (editing ? "edit" : "detail") : "list";
  const title = screen === "new" ? "New sync" : screen === "edit" ? `Edit ${pair?.name}` : screen === "detail" ? pair!.name : "Sync";

  return (
    <div className="flex h-[min(680px,88vh)] flex-col">
      <div className="flex h-14 shrink-0 items-center gap-2 border-b border-line px-4">
        {screen !== "list" && (
          <button onClick={() => (screen === "edit" ? setEditing(false) : go(null))} className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-surface-3 hover:text-ink" aria-label="Back">
            <ArrowLeft size={16} />
          </button>
        )}
        <div className="min-w-0 flex-1 pl-1">
          <h2 className="truncate text-[15px] font-semibold tracking-tight">{title}</h2>
          {screen === "list" && <p className="text-2xs text-ink-3">Folders kept in step automatically, on any drive.</p>}
        </div>
        <button onClick={close} className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-surface-3 hover:text-ink" aria-label="Close">
          <X size={16} />
        </button>
      </div>
      <div className="relative min-h-0 flex-1">
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.div
            key={screen + (pair?.id ?? "")}
            initial={{ opacity: 0, x: screen === "list" ? -20 : 20 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: screen === "list" ? 20 : -20 }}
            transition={{ type: "spring", stiffness: 500, damping: 40 }}
            className="absolute inset-0"
          >
            {screen === "list" && <List onOpen={go} onNew={() => go("new")} />}
            {screen === "new" && <Editor onDone={(id) => go(id ?? null)} />}
            {screen === "edit" && pair && <Editor existing={pair} onDone={() => setEditing(false)} />}
            {screen === "detail" && pair && <Detail pair={pair} onEdit={() => setEditing(true)} />}
          </motion.div>
        </AnimatePresence>
      </div>
    </div>
  );
}
