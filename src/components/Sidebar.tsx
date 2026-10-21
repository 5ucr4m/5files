import {
  Cloud,
  Download,
  FileText,
  HardDrive,
  Home,
  Image,
  Monitor,
  Music2,
  Plus,
  Server,
  Star,
  Usb,
  Video,
  type LucideIcon,
} from "lucide-react";
import { motion } from "motion/react";
import { useState, type DragEvent, type ReactNode } from "react";
import { api } from "../lib/api";
import { canDrop, dropEffect, handleDrop, isInternalDrag } from "../lib/dnd";
import { formatBytes } from "../lib/format";
import { openConnection } from "../lib/ops";
import type { Connection, Loc, Place } from "../lib/types";
import { useDrives, useSyncs } from "../store/live";
import { SyncGlyph, syncStatus } from "./Sync";
import { useSettings } from "../store/settings";
import { useActiveTab, useTabs } from "../store/tabs";
import { confirmDialog, toast, useUi } from "../store/ui";

const PLACE_ICON: Record<Place["id"], LucideIcon> = {
  home: Home,
  desktop: Monitor,
  documents: FileText,
  downloads: Download,
  pictures: Image,
  music: Music2,
  videos: Video,
};

export const KIND_META: Record<Connection["kind"], { label: string; icon: LucideIcon }> = {
  s3: { label: "Amazon S3", icon: Cloud },
  gdrive: { label: "Google Drive", icon: Cloud },
  nextcloud: { label: "Nextcloud", icon: Cloud },
  webdav: { label: "WebDAV", icon: Cloud },
  sftp: { label: "SFTP", icon: Server },
  ftp: { label: "FTP", icon: Server },
};

function Section({ title, children, action }: { title: string; children: ReactNode; action?: ReactNode }) {
  return (
    <div className="mb-4">
      <div className="mb-1 flex h-6 items-center justify-between pr-1 pl-2.5">
        <span className="text-[10.5px] font-semibold tracking-[0.08em] text-ink-4 uppercase">{title}</span>
        {action}
      </div>
      <div className="space-y-px">{children}</div>
    </div>
  );
}

interface ItemProps {
  icon: ReactNode;
  label: string;
  active?: boolean;
  dest?: Loc;
  trailing?: ReactNode;
  below?: ReactNode;
  onClick: (e: React.MouseEvent) => void;
  onContextMenu?: (e: React.MouseEvent) => void;
  title?: string;
}

function Item({ icon, label, active, dest, trailing, below, onClick, onContextMenu, title }: ItemProps) {
  const [over, setOver] = useState(false);
  const dragProps = dest
    ? {
        onDragOver: (e: DragEvent) => {
          if (!isInternalDrag(e) || !canDrop(dest)) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = dropEffect(e, dest);
          setOver(true);
        },
        onDragLeave: () => setOver(false),
        onDrop: (e: DragEvent) => {
          setOver(false);
          void handleDrop(e, dest);
        },
      }
    : {};
  return (
    <button
      onClick={onClick}
      onContextMenu={onContextMenu}
      title={title}
      {...dragProps}
      className={`relative flex w-full flex-col rounded-[8px] px-2.5 py-[5px] text-left transition-colors ${
        over ? "bg-sel-strong ring-1 ring-accent" : active ? "text-ink" : "text-ink-2 hover:bg-chrome-active"
      }`}
    >
      {active && !over && <motion.span layoutId="side-active" className="absolute inset-0 rounded-[8px] bg-chrome-active" transition={{ type: "spring", stiffness: 600, damping: 42 }} />}
      <span className="relative flex w-full items-center gap-2.5">
        <span className={`flex size-4 shrink-0 items-center justify-center ${active ? "text-accent" : "text-ink-3"}`}>{icon}</span>
        <span className="min-w-0 flex-1 truncate text-[13px]">{label}</span>
        {trailing}
      </span>
      {below && <span className="relative mt-1 ml-[26px] block">{below}</span>}
    </button>
  );
}

function Usage({ total, free }: { total: number; free: number }) {
  const used = total > 0 ? (total - free) / total : 0;
  const tone = used > 0.92 ? "var(--danger)" : used > 0.8 ? "var(--warning)" : "var(--accent)";
  return (
    <span className="block">
      <span className="block h-[3px] overflow-hidden rounded-full bg-line">
        <motion.span className="block h-full rounded-full" style={{ background: tone }} initial={{ width: 0 }} animate={{ width: `${used * 100}%` }} transition={{ duration: 0.6, ease: "easeOut" }} />
      </span>
      <span className="num mt-0.5 block text-[10.5px] text-ink-4">{formatBytes(free)} free of {formatBytes(total)}</span>
    </span>
  );
}

function StatusDot({ c }: { c: Connection }) {
  const color = c.status === "connected" ? "var(--success)" : c.status === "error" ? "var(--danger)" : c.status === "connecting" ? "var(--warning)" : "transparent";
  return (
    <span
      className={`size-[7px] shrink-0 rounded-full ${c.status === "connecting" ? "animate-pulse" : ""} ${c.status === "disconnected" ? "border border-ink-4" : ""}`}
      style={{ background: color }}
      title={c.error ?? c.status}
    />
  );
}

export function Sidebar() {
  const tab = useActiveTab();
  const { volumes, places, connections, roots } = useDrives();
  const syncs = useSyncs((s) => s.syncs);
  const favorites = useSettings((s) => s.favorites);
  const width = useSettings((s) => s.sidebarWidth);

  const go = (loc: Loc, e: React.MouseEvent) => {
    const tabs = useTabs.getState();
    if (e.metaKey || e.ctrlKey || e.button === 1) tabs.newTab(loc, { background: true, afterId: tab?.id });
    else if (tab) void tabs.navigate(tab.id, loc);
  };
  const isHere = (loc: Loc) => tab?.fs === loc.fs && tab?.path === loc.path;

  const connMenu = (e: React.MouseEvent, c: Connection) => {
    e.preventDefault();
    useUi.getState().openMenu(e.clientX, e.clientY, [
      { label: "Open", run: () => openConnection(c.id) },
      { label: "Open in new tab", run: () => openConnection(c.id, { newTab: true }) },
      { divider: true },
      { label: "Edit connection…", run: () => useUi.getState().set({ panel: "connections", editing: { id: c.id } }) },
      {
        label: "Disconnect",
        disabled: c.status !== "connected",
        run: async () => {
          await api().disconnect(c.id);
          useDrives.getState().setStatus(c.id, "disconnected");
        },
      },
      { divider: true },
      {
        label: "Remove drive…",
        danger: true,
        run: async () => {
          const ok = await confirmDialog({ title: `Remove “${c.name}”?`, body: "5files forgets this connection and its saved credentials. Files on the server aren't touched.", confirm: "Remove", danger: true });
          if (!ok) return;
          await api().deleteConnection(c.id);
          await useDrives.getState().refreshConnections();
          toast({ tone: "success", title: `Removed ${c.name}` });
        },
      },
    ]);
  };

  return (
    <aside className="scroll flex shrink-0 flex-col overflow-y-auto px-2 pt-1 pb-3" style={{ width }}>
      <Section title="Places">
        {places.map((p) => {
          const Icon = PLACE_ICON[p.id] ?? Home;
          const loc = { fs: "local", path: p.path };
          return <Item key={p.id} icon={<Icon size={15} />} label={p.name} active={isHere(loc)} dest={loc} onClick={(e) => go(loc, e)} title={p.path} />;
        })}
      </Section>

      {favorites.length > 0 && (
        <Section title="Favorites">
          {favorites.map((f) => (
            <Item
              key={f.fs + f.path}
              icon={<Star size={14} />}
              label={f.name}
              active={isHere(f)}
              dest={f}
              title={f.path}
              onClick={(e) => go(f, e)}
              onContextMenu={(e) => {
                e.preventDefault();
                useUi.getState().openMenu(e.clientX, e.clientY, [{ label: "Remove from Favorites", run: () => useSettings.getState().toggleFavorite(f) }]);
              }}
            />
          ))}
        </Section>
      )}

      <Section title="Drives">
        {volumes.map((v) => {
          const loc = { fs: "local", path: v.mount };
          const Icon = v.removable ? Usb : HardDrive;
          return <Item key={v.mount} icon={<Icon size={15} />} label={v.name} active={isHere(loc)} dest={loc} onClick={(e) => go(loc, e)} below={<Usage total={v.total} free={v.free} />} title={v.mount} />;
        })}
      </Section>

      <Section
        title="Remote"
        action={
          <button
            onClick={() => useUi.getState().set({ panel: "connections", editing: null })}
            className="flex size-5 items-center justify-center rounded-md text-ink-3 hover:bg-surface-3 hover:text-ink"
            title="Add a remote drive"
            aria-label="Add a remote drive"
          >
            <Plus size={14} />
          </button>
        }
      >
        {connections.map((c) => {
          const Icon = KIND_META[c.kind].icon;
          const here = tab?.fs === c.id;
          return (
            <Item
              key={c.id}
              icon={<Icon size={15} style={{ color: c.color ?? undefined }} />}
              label={c.name}
              active={here && tab?.listing?.crumbs.length === 1}
              dest={c.status === "connected" ? { fs: c.id, path: roots[c.id] ?? "/" } : undefined}
              trailing={<StatusDot c={c} />}
              title={`${KIND_META[c.kind].label}${c.error ? ` — ${c.error}` : ""}`}
              onClick={(e) => void openConnection(c.id, { newTab: e.metaKey || e.ctrlKey })}
              onContextMenu={(e) => connMenu(e, c)}
            />
          );
        })}
        {connections.length === 0 && (
          <button
            onClick={() => useUi.getState().set({ panel: "connections", editing: null })}
            className="mx-1 mt-1 flex w-[calc(100%-8px)] items-center gap-2 rounded-lg border border-dashed border-line-strong px-2.5 py-2 text-left text-2xs text-ink-3 hover:border-accent hover:text-ink-2"
          >
            <Plus size={13} /> Connect S3, Drive, Nextcloud, SFTP or FTP
          </button>
        )}
      </Section>

      <Section
        title="Sync"
        action={
          <button
            onClick={() => useUi.getState().set({ panel: "sync", syncView: "new" })}
            className="flex size-5 items-center justify-center rounded-md text-ink-3 hover:bg-surface-3 hover:text-ink"
            title="Keep two folders in step"
            aria-label="New sync"
          >
            <Plus size={14} />
          </button>
        }
      >
        {syncs.map((p) => {
          const st = syncStatus(p);
          return (
            <Item
              key={p.id}
              icon={<SyncGlyph pair={p} size={15} />}
              label={p.name}
              title={`${p.source.path} ${p.twoWay ? "⇄" : "→"} ${p.dest.path}\n${st.text}`}
              onClick={() => useUi.getState().set({ panel: "sync", syncView: p.id })}
              below={
                <span className="block truncate text-[10.5px]" style={{ color: st.tone === "error" ? "var(--danger)" : st.tone === "warn" ? "var(--warning)" : "var(--ink-4)" }}>
                  {p.state === "syncing" ? `${Math.round((p.done / Math.max(1, p.total)) * 100)}% · ${p.current ?? ""}` : st.text}
                </span>
              }
              onContextMenu={(e) => {
                e.preventDefault();
                useUi.getState().openMenu(e.clientX, e.clientY, [
                  { label: "Sync now", run: () => void api().syncRun(p.id, false) },
                  { label: p.paused ? "Resume" : "Pause", run: () => void api().syncPause(p.id, !p.paused) },
                  { divider: true },
                  { label: "Show details…", run: () => useUi.getState().set({ panel: "sync", syncView: p.id }) },
                ]);
              }}
            />
          );
        })}
        {syncs.length === 0 && (
          <button
            onClick={() => useUi.getState().set({ panel: "sync", syncView: "new" })}
            className="mx-1 mt-1 flex w-[calc(100%-8px)] items-center gap-2 rounded-lg border border-dashed border-line-strong px-2.5 py-2 text-left text-2xs text-ink-3 hover:border-accent hover:text-ink-2"
          >
            <Plus size={13} /> Keep two folders in step, on any drive
          </button>
        )}
      </Section>
    </aside>
  );
}
