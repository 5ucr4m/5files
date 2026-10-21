import { Clipboard, Loader2, Settings2 } from "lucide-react";
import { formatBytes } from "../lib/format";
import { plural, selectedEntries } from "../lib/ops";
import { platform } from "../lib/platform";
import { useDrives } from "../store/live";
import { useSettings } from "../store/settings";
import type { Tab } from "../store/tabs";
import { useUi } from "../store/ui";
import { visibleEntries } from "./FileView";

/** The volume a path lives on: longest mount that is the path or a parent of it. */
export function volumeFor<V extends { mount: string }>(volumes: V[], path: string, os: string): V | undefined {
  const fold = os === "linux" ? (x: string) => x : (x: string) => x.toLowerCase();
  const p = fold(path);
  return volumes
    .filter((v) => {
      const m = fold(v.mount).replace(/[\\/]+$/, "");
      return p === m || m === "" || p.startsWith(m + "/") || p.startsWith(m + "\\") || (m.endsWith(":") && p.startsWith(m));
    })
    .sort((a, b) => b.mount.length - a.mount.length)[0];
}

export function StatusBar({ tab }: { tab: Tab }) {
  const showHidden = useSettings((s) => s.showHidden);
  const foldersFirst = useSettings((s) => s.foldersFirst);
  const volumes = useDrives((s) => s.volumes);
  const clip = useUi((s) => s.clipboard);
  const items = visibleEntries(tab, showHidden, foldersFirst);
  const sel = selectedEntries(tab);
  const selBytes = sel.reduce((t, e) => t + (e.isDir ? 0 : e.size), 0);
  // Longest mount point that prefixes the current path = the volume we're on.
  const vol = tab.fs === "local" ? volumeFor(volumes, tab.path, platform().os) : undefined;
  const hidden = (tab.listing?.entries.length ?? 0) - (tab.listing?.entries.filter((e) => !e.hidden).length ?? 0);

  return (
    <footer className="num flex h-[28px] shrink-0 items-center gap-4 border-t border-line bg-surface px-3 text-[11px] text-ink-3">
      <span>
        {tab.search ? (
          <>
            {tab.search.running && <Loader2 size={11} className="mr-1 inline animate-spin" />}
            {plural(tab.search.results.length, "result")}
            {tab.search.truncated && " (limit reached)"}
            {tab.search.running && ` · ${tab.search.scanned.toLocaleString()} checked`}
          </>
        ) : (
          <>
            {plural(items.length, "item")}
            {!showHidden && hidden > 0 && <span className="text-ink-4"> · {hidden} hidden</span>}
          </>
        )}
      </span>
      {sel.length > 0 && (
        <span className="text-ink-2">
          {sel.length} selected{selBytes > 0 && ` · ${formatBytes(selBytes)}`}
        </span>
      )}
      {clip && (
        <span className="flex items-center gap-1 text-ink-4" title="Clipboard">
          <Clipboard size={11} /> {clip.items.length} {clip.mode === "cut" ? "to move" : "to copy"}
        </span>
      )}
      <span className="flex-1" />
      {vol && <span>{formatBytes(vol.free)} available</span>}
      <button onClick={() => useUi.getState().set({ panel: "settings" })} className="flex items-center gap-1 rounded px-1 hover:text-ink" title="Settings" aria-label="Settings">
        <Settings2 size={12} />
      </button>
    </footer>
  );
}
