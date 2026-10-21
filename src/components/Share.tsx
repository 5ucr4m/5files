import { ArrowLeft, Check, Copy, Download, ExternalLink, Eye, FileArchive, Laptop, Radio, TriangleAlert, Upload, Users, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useState } from "react";
import { api, errorMessage } from "../lib/api";
import { formatBytes } from "../lib/format";
import { fsName, plural } from "../lib/ops";
import type { FsId, ShareActivity, ShareOptions, ShareView } from "../lib/types";
import { useShares } from "../store/live";
import { confirmDialog, toast, useUi } from "../store/ui";
import { FolderGlyph } from "./FileIcon";
import { Segmented, Toggle } from "./Settings";
import { relativeTime } from "./Sync";

const DEFAULTS: ShareOptions = { allowUpload: false, showHidden: false, expiresInMins: null };

/** Shares a folder on the local network (or shows its link if it already is). */
export async function shareFolder(fs: FsId, path: string) {
  const existing = useShares.getState().shares.find((s) => s.fs === fs && s.path === path);
  if (existing) return useUi.getState().set({ panel: "share", shareView: existing.id });
  try {
    const v = await api().shareStart(fs, path, DEFAULTS);
    const list = useShares.getState().shares;
    if (!list.some((s) => s.id === v.id)) useShares.getState().set([...list, v]);
    useUi.getState().set({ panel: "share", shareView: v.id });
  } catch (e) {
    toast({ tone: "error", title: "Couldn't share this folder", body: errorMessage(e) });
  }
}

export async function stopShare(s: ShareView) {
  if (s.active > 0) {
    const ok = await confirmDialog({
      title: `Stop sharing “${s.name}”?`,
      body: `${plural(s.active, "transfer")} to another device will stop, and the link will stop working.`,
      confirm: "Stop sharing",
      danger: true,
    });
    if (!ok) return;
  }
  await api().shareStop(s.id);
  useShares.getState().set(useShares.getState().shares.filter((x) => x.id !== s.id));
  toast({ tone: "info", title: `Stopped sharing “${s.name}”`, body: "Its link no longer works." });
}

export async function copyShareLink(s: ShareView) {
  try {
    await navigator.clipboard.writeText(s.url);
    toast({ tone: "success", title: "Link copied", body: s.url });
  } catch {
    toast({ tone: "error", title: "Clipboard unavailable" });
  }
}

/** Re-render every few seconds so "2 min ago" and countdowns stay true. */
function useNow(ms = 15_000) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const iv = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(iv);
  }, [ms]);
  return now;
}

// ---------------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------------

/** Crisp QR from the module matrix the backend computed (quiet zone included). */
export function QrCode({ qr, size }: { qr: ShareView["qr"]; size: number }) {
  const path = useMemo(() => {
    let d = "";
    for (let r = 0; r < qr.size; r++)
      for (let c = 0; c < qr.size; c++) if (qr.bits[r * qr.size + c] === "1") d += `M${c + 2} ${r + 2}h1v1h-1z`;
    return d;
  }, [qr]);
  const n = qr.size + 4;
  return (
    <svg width={size} height={size} viewBox={`0 0 ${n} ${n}`} shapeRendering="crispEdges" role="img" aria-label="QR code with the link">
      <rect width={n} height={n} rx={2} fill="#fff" />
      <path d={path} fill="#15171c" />
    </svg>
  );
}

/** The 5files pipe in miniature: this folder on the left, the devices on the right;
 *  orbs run along it while something is being transferred. */
function Pipe({ share }: { share: ShareView }) {
  const flowing = share.active > 0;
  const incoming = share.activity[0]?.action === "uploaded" && flowing;
  return (
    <div className="flex items-center gap-0">
      <span className="z-[1] flex h-8 shrink-0 items-center gap-2 rounded-full bg-ink pr-3.5 pl-2.5 text-[12px] font-medium text-bg">
        <Laptop size={14} className="opacity-80" /> This computer
      </span>
      <div className="relative h-[7px] min-w-10 flex-1 overflow-hidden" style={{ background: "linear-gradient(to bottom, var(--line-strong), var(--line))" }}>
        <AnimatePresence>
          {flowing &&
            [0, 1, 2].map((i) => (
              <motion.span
                key={i}
                className="absolute top-1/2 size-[9px] -translate-y-1/2 rounded-full"
                style={{
                  background: "radial-gradient(circle at 35% 32%, rgb(255 255 255 / .9), transparent 45%), var(--accent)",
                  boxShadow: "0 0 10px color-mix(in oklab, var(--accent) 60%, transparent)",
                }}
                initial={{ left: incoming ? "100%" : "-5%", opacity: 0 }}
                animate={{ left: incoming ? ["100%", "-5%"] : ["-5%", "100%"], opacity: [0, 1, 1, 0] }}
                exit={{ opacity: 0 }}
                transition={{ duration: 1.6, repeat: Infinity, delay: i * 0.53, ease: "easeInOut" }}
              />
            ))}
        </AnimatePresence>
      </div>
      <span
        className={`z-[1] flex h-8 shrink-0 items-center gap-2 rounded-[9px] border px-3 text-[12px] font-medium ${share.visitors ? "border-[color-mix(in_oklab,var(--accent)_45%,var(--line-strong))] text-accent" : "border-line-strong text-ink-3"} bg-surface`}
      >
        <Users size={14} /> {share.visitors ? plural(share.visitors, "device") : "No visitors yet"}
      </span>
    </div>
  );
}

const ACTION: Record<ShareActivity["action"], { icon: typeof Eye; text: (a: ShareActivity) => React.ReactNode }> = {
  opened: { icon: Eye, text: () => "opened the link" },
  downloaded: { icon: Download, text: (a) => <>downloaded <b className="font-medium text-ink">{a.name}</b></> },
  zip: { icon: FileArchive, text: (a) => <>downloaded <b className="font-medium text-ink">{a.name}</b> as .zip</> },
  uploaded: { icon: Upload, text: (a) => <>added <b className="font-medium text-ink">{a.name}</b></> },
};

const EXPIRY = [
  { value: "never", label: "Never" },
  { value: "60", label: "In 1 hour" },
  { value: "1440", label: "In 1 day" },
] as const;

// ---------------------------------------------------------------------------------
// Detail
// ---------------------------------------------------------------------------------

function Detail({ share }: { share: ShareView }) {
  const now = useNow();
  const [copied, setCopied] = useState(false);
  const set = (patch: Partial<ShareOptions>) => {
    const options = { ...share.options, ...patch };
    useShares.getState().set(useShares.getState().shares.map((s) => (s.id === share.id ? { ...s, options } : s)));
    api()
      .shareUpdate(share.id, options)
      .catch((e) => toast({ tone: "error", title: "Couldn't change sharing", body: errorMessage(e) }));
  };
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(share.url);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      toast({ tone: "error", title: "Clipboard unavailable" });
    }
  };
  const where = share.fs === "local" ? share.path : `${fsName(share.fs)} · ${share.path}`;
  const expiry = share.options.expiresInMins == null ? "never" : String(share.options.expiresInMins);
  const until = share.expiresAt ? new Date(share.expiresAt).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }) : null;
  const untilDay = share.expiresAt && new Date(share.expiresAt).toDateString() !== new Date(now).toDateString();

  return (
    <div className="flex h-full flex-col">
      <div className="scroll min-h-0 flex-1 overflow-y-auto px-6 pt-5 pb-4">
        {/* The link, the code, and where it comes from */}
        <div className="flex gap-6">
          <div className="shrink-0 rounded-2xl border border-line bg-white p-2 shadow-soft">
            <QrCode qr={share.qr} size={156} />
          </div>
          <div className="flex min-w-0 flex-1 flex-col">
            <div className="flex min-w-0 items-center gap-2.5">
              <FolderGlyph size={22} />
              <h3 className="truncate text-[17px] font-semibold tracking-tight">{share.name}</h3>
            </div>
            <p className="num mt-0.5 truncate text-2xs text-ink-4" title={where}>
              {where}
            </p>
            <div className="mt-4 flex items-center gap-1.5 rounded-xl border border-line bg-surface-2 p-1 pl-3">
              <span className="num min-w-0 flex-1 truncate text-[12.5px] text-ink select-text" title={share.url}>
                {share.url.replace(/^http:\/\//, "")}
              </span>
              <button onClick={copy} className="flex h-8 shrink-0 items-center gap-1.5 rounded-lg bg-accent px-3 text-[12.5px] font-medium text-white hover:brightness-110">
                {copied ? <Check size={14} /> : <Copy size={14} />} {copied ? "Copied" : "Copy link"}
              </button>
              <button
                onClick={() => void api().openUrl(share.url)}
                className="flex size-8 shrink-0 items-center justify-center rounded-lg text-ink-3 hover:bg-surface-3 hover:text-ink"
                title="Open in your browser"
                aria-label="Open in your browser"
              >
                <ExternalLink size={15} />
              </button>
            </div>
            {share.altUrls[0] && (
              <p className="mt-1.5 truncate text-2xs text-ink-4">
                Also works: <span className="num text-ink-3 select-text">{share.altUrls[0].replace(/^http:\/\//, "")}</span>
              </p>
            )}
            <p className="mt-auto pt-3 text-[12px] leading-relaxed text-ink-3">
              Scan the code with a phone camera, or open the link on any computer on the same Wi-Fi. No app needed — it opens in the browser.
            </p>
          </div>
        </div>

        {!share.online && (
          <div className="mt-4 flex items-start gap-2.5 rounded-xl border border-[color-mix(in_oklab,var(--warning)_40%,transparent)] bg-[color-mix(in_oklab,var(--warning)_10%,transparent)] px-3 py-2.5 text-[12px] text-ink-2">
            <TriangleAlert size={15} className="mt-px shrink-0 text-warning" />
            This computer isn't on a network right now, so only it can open the link. Connect to Wi-Fi and the link updates.
          </div>
        )}

        <div className="mt-5">
          <Pipe share={share} />
          <p className="num mt-2 text-2xs text-ink-4">
            {formatBytes(share.bytesOut)} sent · {formatBytes(share.bytesIn)} received
            {share.lastSeen ? ` · last visit ${relativeTime(share.lastSeen, now)}` : ""}
          </p>
        </div>

        {/* Options */}
        <div className="mt-5 divide-y divide-line rounded-xl border border-line">
          <label className="flex items-center gap-3 px-3.5 py-3">
            <div className="min-w-0 flex-1">
              <div className="text-[13px] font-medium">Let visitors add files</div>
              <div className="text-2xs text-ink-3">They can drop files into this folder. They can't delete or replace anything.</div>
            </div>
            <Toggle on={share.options.allowUpload} onChange={(v) => set({ allowUpload: v })} label="Let visitors add files" />
          </label>
          <label className="flex items-center gap-3 px-3.5 py-3">
            <div className="min-w-0 flex-1">
              <div className="text-[13px] font-medium">Include hidden files</div>
              <div className="text-2xs text-ink-3">Files like .DS_Store and .git stay private unless this is on.</div>
            </div>
            <Toggle on={share.options.showHidden} onChange={(v) => set({ showHidden: v })} label="Include hidden files" />
          </label>
          <div className="flex items-center gap-3 px-3.5 py-3">
            <div className="min-w-0 flex-1">
              <div className="text-[13px] font-medium">Stop sharing</div>
              <div className="text-2xs text-ink-3">
                {until ? `The link stops working ${untilDay ? "tomorrow at" : "at"} ${until}.` : "Stays on until you stop it or quit 5files."}
              </div>
            </div>
            <Segmented
              id={`exp-${share.id}`}
              value={expiry}
              options={EXPIRY.map((o) => ({ ...o }))}
              onChange={(v) => set({ expiresInMins: v === "never" ? null : Number(v) })}
            />
          </div>
        </div>

        {/* Activity */}
        <div className="mt-5">
          <p className="mb-2 text-[11px] font-semibold tracking-[0.08em] text-ink-4 uppercase">Activity</p>
          {share.activity.length === 0 ? (
            <p className="rounded-xl border border-dashed border-line-strong px-3 py-4 text-center text-[12px] text-ink-3">No one has opened the link yet.</p>
          ) : (
            <ul className="space-y-0.5">
              <AnimatePresence initial={false}>
                {share.activity.map((a) => {
                  const A = ACTION[a.action];
                  return (
                    <motion.li
                      key={`${a.at}-${a.who}-${a.action}-${a.name}`}
                      layout
                      initial={{ opacity: 0, y: -6 }}
                      animate={{ opacity: 1, y: 0 }}
                      className="flex items-center gap-2.5 rounded-lg px-2 py-1.5 text-[12.5px] text-ink-3"
                    >
                      <span className="flex size-6 shrink-0 items-center justify-center rounded-md bg-surface-3 text-ink-2">
                        <A.icon size={13} />
                      </span>
                      <span className="min-w-0 flex-1 truncate">
                        <span className="num text-ink-2">{a.who}</span> {A.text(a)}
                      </span>
                      <span className="shrink-0 text-2xs text-ink-4">{relativeTime(a.at, now)}</span>
                    </motion.li>
                  );
                })}
              </AnimatePresence>
            </ul>
          )}
        </div>
      </div>

      <div className="flex shrink-0 items-center gap-3 border-t border-line px-6 py-3">
        <p className="min-w-0 flex-1 text-2xs text-ink-4">Only people with the link can open it. Sharing keeps going while 5files is in the menu bar.</p>
        <button
          onClick={() => void stopShare(share)}
          className="h-8 shrink-0 rounded-lg border border-line-strong px-3 text-[12.5px] font-medium text-danger hover:border-danger hover:bg-[color-mix(in_oklab,var(--danger)_8%,transparent)]"
        >
          Stop sharing
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------------
// List (more than one shared folder)
// ---------------------------------------------------------------------------------

function List({ shares, onOpen }: { shares: ShareView[]; onOpen: (id: string) => void }) {
  if (!shares.length)
    return (
      <div className="flex h-full flex-col items-center justify-center px-10 text-center">
        <span className="flex size-12 items-center justify-center rounded-2xl bg-surface-3 text-ink-3">
          <Radio size={22} />
        </span>
        <h3 className="mt-4 text-[15px] font-semibold">No folders shared</h3>
        <p className="mt-1 max-w-[360px] text-[12.5px] text-ink-3">
          Right-click a folder and choose <b className="font-medium text-ink-2">Share on network…</b> to open it in a browser on another computer or phone.
        </p>
      </div>
    );
  return (
    <div className="scroll h-full space-y-2 overflow-y-auto p-4">
      {shares.map((s) => (
        <button key={s.id} onClick={() => onOpen(s.id)} className="flex w-full items-center gap-3 rounded-xl border border-line p-3 text-left hover:bg-surface-2">
          <FolderGlyph size={28} />
          <div className="min-w-0 flex-1">
            <div className="truncate text-[13.5px] font-medium">{s.name}</div>
            <div className="num truncate text-2xs text-ink-4">{s.url.replace(/^http:\/\//, "")}</div>
          </div>
          <span className="text-2xs text-ink-3">{s.visitors ? plural(s.visitors, "device") : "No visitors yet"}</span>
        </button>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------------

export function SharePanel() {
  const view = useUi((s) => s.shareView);
  const shares = useShares((s) => s.shares);
  const share = shares.find((s) => s.id === view) ?? (shares.length === 1 ? shares[0] : undefined);
  const close = () => useUi.getState().set({ panel: null, shareView: null });
  const go = (id: string | null) => useUi.getState().set({ shareView: id });
  const live = !!share && (share.active > 0 || (share.lastSeen != null && Date.now() - share.lastSeen < 60_000));

  return (
    <div className="flex h-[min(700px,90vh)] flex-col">
      <div className="flex h-14 shrink-0 items-center gap-2 border-b border-line px-4">
        {share && shares.length > 1 && (
          <button onClick={() => go(null)} className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-surface-3 hover:text-ink" aria-label="All shared folders">
            <ArrowLeft size={16} />
          </button>
        )}
        <div className="min-w-0 flex-1 pl-1">
          <h2 className="truncate text-[15px] font-semibold tracking-tight">{share ? "Shared on your network" : "Shared folders"}</h2>
          {!share && <p className="text-2xs text-ink-3">Folders other devices can open in a browser.</p>}
        </div>
        {share && (
          <span className={`mr-1 flex items-center gap-1.5 rounded-full px-2 py-0.5 text-[11px] font-medium ${live ? "bg-[color-mix(in_oklab,var(--success)_14%,transparent)] text-success" : "bg-surface-3 text-ink-3"}`}>
            <span className={`size-1.5 rounded-full ${live ? "animate-pulse bg-success" : "bg-ink-4"}`} />
            {share.active > 0 ? "Transferring" : live ? "In use" : "Ready"}
          </span>
        )}
        <button onClick={close} className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-surface-3 hover:text-ink" aria-label="Close">
          <X size={16} />
        </button>
      </div>
      <div className="relative min-h-0 flex-1">
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.div
            key={share?.id ?? "list"}
            initial={{ opacity: 0, x: share ? 20 : -20 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: share ? -20 : 20 }}
            transition={{ type: "spring", stiffness: 500, damping: 40 }}
            className="absolute inset-0"
          >
            {share ? <Detail share={share} /> : <List shares={shares} onOpen={go} />}
          </motion.div>
        </AnimatePresence>
      </div>
    </div>
  );
}
