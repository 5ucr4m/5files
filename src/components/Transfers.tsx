import { ArrowRightLeft, Check, Copy, FolderOpen, Pause, Play, RotateCw, Trash2, X, AlertTriangle, Ban } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { api, errorMessage } from "../lib/api";
import { formatBytes, formatDuration, formatRate } from "../lib/format";
import { fsName, plural } from "../lib/ops";
import type { Job } from "../lib/types";
import { aggregate, isActive, useTransfers } from "../store/live";
import { useTabs } from "../store/tabs";
import { toast, useUi } from "../store/ui";

function Ring({ progress, size = 20 }: { progress: number; size?: number }) {
  const r = (size - 3) / 2;
  const c = 2 * Math.PI * r;
  return (
    <svg width={size} height={size} className="-rotate-90">
      <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="var(--line-strong)" strokeWidth={2.2} />
      <motion.circle
        cx={size / 2}
        cy={size / 2}
        r={r}
        fill="none"
        stroke="var(--accent)"
        strokeWidth={2.2}
        strokeLinecap="round"
        strokeDasharray={c}
        animate={{ strokeDashoffset: c * (1 - progress) }}
        transition={{ type: "tween", duration: 0.25 }}
      />
    </svg>
  );
}

export function TransfersButton() {
  const jobs = useTransfers((s) => s.jobs);
  const open = useUi((s) => s.panel === "transfers");
  const agg = aggregate(jobs);
  const failed = jobs.some((j) => j.status === "failed");
  return (
    <button
      onClick={() => useUi.getState().set({ panel: open ? null : "transfers" })}
      className={`no-drag relative flex h-[30px] items-center gap-2 rounded-[9px] px-2 text-ink-2 hover:bg-chrome-active ${open ? "bg-chrome-active" : ""}`}
      title="Transfers"
      aria-label="Transfers"
    >
      {agg ? <Ring progress={agg.progress} /> : <ArrowRightLeft size={15} />}
      <AnimatePresence>
        {agg && (
          <motion.span initial={{ opacity: 0, width: 0 }} animate={{ opacity: 1, width: "auto" }} exit={{ opacity: 0, width: 0 }} className="num overflow-hidden text-[11px] whitespace-nowrap">
            {Math.round(agg.progress * 100)}%
          </motion.span>
        )}
      </AnimatePresence>
      {failed && !agg && <span className="absolute top-1 right-1 size-1.5 rounded-full bg-danger" />}
    </button>
  );
}

const verb: Record<Job["kind"], [string, string]> = {
  copy: ["Copying", "Copied"],
  move: ["Moving", "Moved"],
  delete: ["Deleting", "Deleted"],
};

function statusLine(j: Job) {
  switch (j.status) {
    case "queued":
      return "Waiting…";
    case "scanning":
      return j.totalFiles ? `Preparing · ${plural(j.totalFiles, "file")} · ${formatBytes(j.totalBytes)}` : "Preparing…";
    case "paused":
      return `Paused · ${formatBytes(j.doneBytes)} of ${formatBytes(j.totalBytes)}`;
    case "running":
      if (j.kind === "delete") return `${j.doneFiles.toLocaleString()} of ${plural(j.totalFiles, "item")}`;
      return [`${formatBytes(j.doneBytes)} of ${formatBytes(j.totalBytes)}`, j.speed ? formatRate(j.speed) : null, j.etaSecs != null ? `${formatDuration(j.etaSecs)} left` : null]
        .filter(Boolean)
        .join(" · ");
    case "done": {
      const secs = j.finishedAt && j.createdAt ? (j.finishedAt - j.createdAt) / 1000 : 0;
      const skipped = j.skipped ? ` · ${j.skipped} skipped` : "";
      return j.kind === "delete" ? `${verb.delete[1]} ${plural(j.totalFiles, "item")}` : `${formatBytes(j.totalBytes)} in ${formatDuration(secs)}${skipped}`;
    }
    case "failed":
      return j.error ?? "Failed";
    case "cancelled":
      return "Cancelled";
  }
}

function JobRow({ j }: { j: Job }) {
  const a = api();
  const progress = j.totalBytes > 0 ? j.doneBytes / j.totalBytes : j.totalFiles > 0 ? j.doneFiles / j.totalFiles : 0;
  const running = j.status === "running";
  const final = !isActive(j);
  const KindIcon = j.kind === "delete" ? Trash2 : j.kind === "move" ? ArrowRightLeft : Copy;
  const tone = j.status === "failed" ? "var(--danger)" : j.status === "done" ? "var(--success)" : j.status === "paused" ? "var(--warning)" : "var(--accent)";
  const run = (p: Promise<unknown>) => p.catch((e) => toast({ tone: "error", title: "Action failed", body: errorMessage(e) }));
  const reveal = () => {
    if (!j.dest) return;
    const tabs = useTabs.getState();
    tabs.newTab({ fs: j.dest.fs, path: j.dest.path });
    useUi.getState().set({ panel: null });
  };

  return (
    <motion.li
      layout
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, x: 24, transition: { duration: 0.15 } }}
      transition={{ type: "spring", stiffness: 500, damping: 38 }}
      className="group rounded-xl border border-line bg-surface p-3"
    >
      <div className="flex items-start gap-3">
        <div className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-lg" style={{ background: `color-mix(in oklab, ${tone} 14%, transparent)`, color: tone }}>
          {j.status === "done" ? <Check size={16} strokeWidth={2.4} /> : j.status === "failed" ? <AlertTriangle size={15} /> : j.status === "cancelled" ? <Ban size={15} /> : <KindIcon size={15} />}
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-1.5">
            <span className="shrink-0 text-ink-3">{final ? verb[j.kind][1] : verb[j.kind][0]}</span>
            <span className="truncate font-medium">{j.title}</span>
          </div>
          <div className="truncate text-2xs text-ink-3">
            {j.dest ? `${fsName(j.srcFs)} → ${fsName(j.dest.fs)} · ${j.dest.path.split(/[\\/]/).filter(Boolean).pop() ?? "/"}` : fsName(j.srcFs)}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-0.5 opacity-70 transition-opacity group-hover:opacity-100">
          {(running || j.status === "paused") && (
            <IconBtn label={running ? "Pause" : "Resume"} onClick={() => run(a.pauseTransfer(j.id, running))}>
              {running ? <Pause size={13} /> : <Play size={13} />}
            </IconBtn>
          )}
          {!final && (
            <IconBtn label="Cancel" onClick={() => run(a.cancelTransfer(j.id))}>
              <X size={14} />
            </IconBtn>
          )}
          {(j.status === "failed" || j.status === "cancelled") && (
            <IconBtn label="Retry" onClick={() => run(a.retryTransfer(j.id))}>
              <RotateCw size={13} />
            </IconBtn>
          )}
          {j.status === "done" && j.dest && !j.openAfter && (
            <IconBtn label="Show in new tab" onClick={reveal}>
              <FolderOpen size={13} />
            </IconBtn>
          )}
          {final && (
            <IconBtn label="Remove from list" onClick={() => run(a.removeTransfer(j.id))}>
              <X size={14} />
            </IconBtn>
          )}
        </div>
      </div>
      {!final && (
        <div className="mt-2.5 ml-11">
          <div className={`relative h-[5px] overflow-hidden rounded-full bg-surface-3 ${j.status === "scanning" || j.status === "queued" ? "indeterminate" : ""}`}>
            <motion.div
              className={`absolute inset-y-0 left-0 rounded-full ${running ? "stripes" : ""}`}
              style={{ backgroundColor: tone }}
              animate={{ width: `${Math.max(progress * 100, running ? 1.5 : 0)}%` }}
              transition={{ type: "tween", ease: "easeOut", duration: 0.2 }}
            />
          </div>
        </div>
      )}
      <div className={`num mt-1.5 ml-11 truncate text-2xs ${j.status === "failed" ? "text-danger" : "text-ink-3"}`} title={j.error ?? undefined}>
        {statusLine(j)}
        {running && j.current && j.totalFiles > 1 && <span className="text-ink-4"> · {j.current}</span>}
      </div>
    </motion.li>
  );
}

function IconBtn({ label, onClick, children }: { label: string; onClick: () => void; children: React.ReactNode }) {
  return (
    <button onClick={onClick} title={label} aria-label={label} className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-surface-3 hover:text-ink">
      {children}
    </button>
  );
}

export function TransferPanel() {
  const jobs = useTransfers((s) => s.jobs);
  const active = jobs.filter(isActive);
  const finished = jobs.filter((j) => !isActive(j)).reverse();
  const agg = aggregate(jobs);
  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between px-5 pt-5 pb-3">
        <div>
          <h2 className="text-[15px] font-semibold tracking-tight">Transfers</h2>
          <p className="num text-2xs text-ink-3">
            {agg ? `${plural(agg.count, "active job")} · ${formatRate(agg.speed)}${agg.eta ? ` · ${formatDuration(agg.eta)} left` : ""}` : "Nothing in progress"}
          </p>
        </div>
        {finished.length > 0 && (
          <button onClick={() => api().clearTransfers()} className="rounded-md px-2 py-1 text-2xs font-medium text-ink-3 hover:bg-surface-3 hover:text-ink">
            Clear finished
          </button>
        )}
      </div>
      <div className="scroll flex-1 overflow-y-auto px-4 pb-4">
        {jobs.length === 0 ? (
          <div className="mt-16 flex flex-col items-center text-center">
            <div className="mb-3 flex size-12 items-center justify-center rounded-2xl bg-surface-3 text-ink-3">
              <ArrowRightLeft size={20} />
            </div>
            <p className="font-medium">No transfers yet</p>
            <p className="mt-1 max-w-[230px] text-2xs text-ink-3">Copies, moves and deletes show up here with live progress. Drag files between tabs to start one.</p>
          </div>
        ) : (
          <ul className="space-y-2">
            <AnimatePresence initial={false}>
              {active.map((j) => (
                <JobRow key={j.id} j={j} />
              ))}
              {finished.length > 0 && active.length > 0 && (
                <motion.li layout key="sep" className="px-1 pt-3 pb-1 text-2xs font-semibold tracking-wide text-ink-4 uppercase">
                  Finished
                </motion.li>
              )}
              {finished.map((j) => (
                <JobRow key={j.id} j={j} />
              ))}
            </AnimatePresence>
          </ul>
        )}
      </div>
    </div>
  );
}
