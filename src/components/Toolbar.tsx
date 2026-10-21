import {
  ArrowDownWideNarrow,
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  ChevronRight,
  Eye,
  EyeOff,
  FilePlus,
  FolderPlus,
  LayoutGrid,
  List,
  Loader2,
  PanelRight,
  RefreshCw,
  Search,
  Star,
  X,
} from "lucide-react";
import { motion } from "motion/react";
import { useEffect, useRef, useState, type DragEvent } from "react";
import { api, errorMessage } from "../lib/api";
import { canDrop, dropEffect, handleDrop, isInternalDrag } from "../lib/dnd";
import type { SortKey } from "../lib/format";
import { newFile, newFolder } from "../lib/ops";
import { kbd } from "../lib/platform";
import { useSettings } from "../store/settings";
import { useSizes } from "../store/sizes";
import { useTabs, type Tab } from "../store/tabs";
import { toast, useUi } from "../store/ui";

function Btn({ label, onClick, disabled, active, children }: { label: string; onClick: (e: React.MouseEvent) => void; disabled?: boolean; active?: boolean; children: React.ReactNode }) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      title={label}
      aria-label={label}
      className={`flex size-[30px] shrink-0 items-center justify-center rounded-[8px] transition-colors disabled:opacity-30 ${active ? "bg-surface-3 text-ink" : "text-ink-2 hover:bg-surface-2 hover:text-ink"}`}
    >
      {children}
    </button>
  );
}

function Crumb({ tab, name, path, last }: { tab: Tab; name: string; path: string; last: boolean }) {
  const [over, setOver] = useState(false);
  const dest = { fs: tab.fs, path };
  return (
    <button
      onClick={() => !last && useTabs.getState().navigate(tab.id, dest)}
      onDragOver={(e: DragEvent) => {
        if (!isInternalDrag(e) || !canDrop(dest)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = dropEffect(e, dest);
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        setOver(false);
        void handleDrop(e, dest);
      }}
      className={`max-w-[200px] shrink-0 truncate rounded-[6px] px-1.5 py-0.5 transition-colors ${
        over ? "bg-sel-strong ring-1 ring-accent" : last ? "font-medium text-ink" : "text-ink-3 hover:bg-surface-3 hover:text-ink"
      }`}
    >
      {name}
    </button>
  );
}

function PathBar({ tab }: { tab: Tab }) {
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState(tab.path);
  const input = useRef<HTMLInputElement>(null);
  const scroller = useRef<HTMLDivElement>(null);
  useEffect(() => setValue(tab.path), [tab.path]);
  useEffect(() => {
    if (editing) input.current?.select();
  }, [editing]);
  useEffect(() => {
    scroller.current?.scrollTo({ left: scroller.current.scrollWidth, behavior: "smooth" });
  }, [tab.listing?.crumbs.length, tab.path]);
  useEffect(() => {
    const onKey = () => setEditing(true);
    window.addEventListener("5files:edit-path", onKey);
    return () => window.removeEventListener("5files:edit-path", onKey);
  }, []);

  const crumbs = tab.listing?.crumbs ?? [];
  const submit = async () => {
    setEditing(false);
    const p = value.trim().replace(/^~(?=$|[\\/])/, (await api().platform()).home ?? "~");
    if (!p || p === tab.path) return;
    try {
      const e = await api().stat(tab.fs, p);
      if (e.isDir) await useTabs.getState().navigate(tab.id, { fs: tab.fs, path: e.path });
      else {
        let parent = e.path.replace(/[\\/][^\\/]+$/, "") || "/";
        // "D:\\notes.txt" -> "D:\\" (a bare "D:" would mean "current dir on D:").
        if (/^[A-Za-z]:$/.test(parent)) parent += "\\";
        await useTabs.getState().navigate(tab.id, { fs: tab.fs, path: parent }, { select: e.path });
      }
    } catch (err) {
      toast({ tone: "error", title: "Can't go there", body: errorMessage(err) });
    }
  };

  return (
    <div className="relative flex h-[32px] min-w-0 flex-1 items-center rounded-[9px] border border-line bg-surface-2/70 transition-colors focus-within:border-accent focus-within:bg-surface">
      {editing ? (
        <input
          ref={input}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onBlur={() => setEditing(false)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void submit();
            if (e.key === "Escape") {
              setValue(tab.path);
              setEditing(false);
            }
          }}
          spellCheck={false}
          className="num h-full w-full bg-transparent px-3 text-[12.5px] outline-none"
          aria-label="Path"
        />
      ) : (
        <div ref={scroller} className="flex h-full min-w-0 flex-1 items-center overflow-x-auto px-1.5 [scrollbar-width:none]" onClick={(e) => e.target === e.currentTarget && setEditing(true)}>
          {crumbs.map((c, i) => (
            <motion.span key={c.path} className="flex shrink-0 items-center" initial={{ opacity: 0, x: -4 }} animate={{ opacity: 1, x: 0 }} transition={{ duration: 0.16, delay: Math.min(i * 0.02, 0.12) }}>
              {i > 0 && <ChevronRight size={13} className="mx-px shrink-0 text-ink-4" />}
              <Crumb tab={tab} name={c.name || c.path} path={c.path} last={i === crumbs.length - 1} />
            </motion.span>
          ))}
          <span className="h-full min-w-8 flex-1" onClick={() => setEditing(true)} />
        </div>
      )}
      {tab.loading && <Loader2 size={13} className="absolute right-2.5 animate-spin text-ink-3" />}
    </div>
  );
}

let searchRuns = 0;

function SearchBox({ tab }: { tab: Tab }) {
  const input = useRef<HTMLInputElement>(null);
  const [value, setValue] = useState(tab.filter || tab.search?.query || "");
  const showHidden = useSettings((s) => s.showHidden);
  useEffect(() => setValue(tab.search?.query ?? tab.filter), [tab.id, tab.nav]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    const focus = () => input.current?.focus();
    window.addEventListener("5files:focus-search", focus);
    return () => window.removeEventListener("5files:focus-search", focus);
  }, []);

  const stopSearch = () => {
    const t = useTabs.getState().tabs.find((x) => x.id === tab.id);
    if (t?.search?.id && t.search.running) void api().cancelSearch(t.search.id);
  };
  const clear = () => {
    stopSearch();
    setValue("");
    useTabs.getState().patch(tab.id, { filter: "", search: null });
  };
  const deep = async () => {
    const query = value.trim();
    if (!query) return clear();
    stopSearch();
    const run = ++searchRuns;
    // Only the run that is still current may write (late events from a cancelled or
    // replaced search are dropped, even for the same query).
    const patch = (fn: (s: NonNullable<Tab["search"]>) => Partial<NonNullable<Tab["search"]>>) => {
      const t = useTabs.getState().tabs.find((x) => x.id === tab.id);
      if (t?.search && t.search.run === run) useTabs.getState().patch(tab.id, { search: { ...t.search, ...fn(t.search) } });
    };
    useTabs.getState().patch(tab.id, { filter: "", selection: new Set(), search: { run, id: null, query, results: [], running: true, scanned: 0, truncated: false } });
    try {
      const id = await api().search(tab.fs, tab.path, query, showHidden, (ev) => {
        if (ev.type === "results") patch((s) => ({ results: [...s.results, ...ev.entries] }));
        else if (ev.type === "progress") patch(() => ({ scanned: ev.scanned }));
        else patch(() => ({ running: false, truncated: ev.truncated }));
      });
      const t = useTabs.getState().tabs.find((x) => x.id === tab.id);
      // Superseded while the backend was starting it: stop it right away.
      if (t?.search?.run !== run) void api().cancelSearch(id);
      else patch(() => ({ id }));
    } catch (e) {
      toast({ tone: "error", title: "Search failed", body: errorMessage(e) });
      patch(() => ({ running: false }));
    }
  };

  return (
    <div className="flex h-[32px] w-[min(280px,28vw)] shrink-0 items-center gap-2 rounded-[9px] border border-line bg-surface-2/70 px-2.5 transition-[border-color,background-color] focus-within:border-accent focus-within:bg-surface">
      {tab.search?.running ? <Loader2 size={14} className="shrink-0 animate-spin text-accent" /> : <Search size={14} className="shrink-0 text-ink-3" />}
      <input
        ref={input}
        value={value}
        placeholder="Filter · ↩ to search subfolders"
        onChange={(e) => {
          setValue(e.target.value);
          if (!tab.search) useTabs.getState().patch(tab.id, { filter: e.target.value });
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") void deep();
          if (e.key === "Escape") {
            clear();
            input.current?.blur();
          }
          if (e.key === "ArrowDown") {
            input.current?.blur();
            window.dispatchEvent(new Event("5files:focus-list"));
          }
          e.stopPropagation();
        }}
        spellCheck={false}
        className="min-w-0 flex-1 bg-transparent text-[12.5px] outline-none placeholder:text-ink-4"
        aria-label="Filter or search"
      />
      {(value || tab.search) && (
        <button onClick={clear} className="flex size-4 shrink-0 items-center justify-center rounded-full bg-ink-4/40 text-surface hover:bg-ink-3" aria-label="Clear search">
          <X size={10} strokeWidth={3} />
        </button>
      )}
    </div>
  );
}

const SORTS: { key: SortKey; label: string }[] = [
  { key: "name", label: "Name" },
  { key: "modified", label: "Date modified" },
  { key: "size", label: "Size" },
  { key: "kind", label: "Kind" },
];

export function Toolbar({ tab }: { tab: Tab }) {
  const tabs = useTabs.getState();
  const s = useSettings();
  const isFav = s.favorites.some((f) => f.fs === tab.fs && f.path === tab.path);
  const crumbs = tab.listing?.crumbs ?? [];

  const sortMenu = (e: React.MouseEvent) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    useUi.getState().openMenu(r.left, r.bottom + 6, [
      ...SORTS.map((o) => ({
        label: `${tab.sort.key === o.key ? "✓  " : "    "}${o.label}`,
        run: () => tabs.patch(tab.id, { sort: { key: o.key, dir: tab.sort.key === o.key ? tab.sort.dir : o.key === "name" || o.key === "kind" ? "asc" : "desc" } }),
      })),
      { divider: true },
      { label: `${tab.sort.dir === "asc" ? "✓  " : "    "}Ascending`, run: () => tabs.patch(tab.id, { sort: { ...tab.sort, dir: "asc" } }) },
      { label: `${tab.sort.dir === "desc" ? "✓  " : "    "}Descending`, run: () => tabs.patch(tab.id, { sort: { ...tab.sort, dir: "desc" } }) },
      { divider: true },
      { label: `${s.foldersFirst ? "✓  " : "    "}Folders first`, run: () => s.set({ foldersFirst: !s.foldersFirst }) },
    ]);
  };

  return (
    <div className="flex h-[48px] shrink-0 items-center gap-1.5 border-b border-line px-2.5">
      <div className="flex items-center">
        <Btn label={`Back (${kbd("Mod+[")})`} disabled={tab.index === 0} onClick={() => tabs.back(tab.id)}>
          <ArrowLeft size={16} />
        </Btn>
        <Btn label={`Forward (${kbd("Mod+]")})`} disabled={tab.index >= tab.history.length - 1} onClick={() => tabs.forward(tab.id)}>
          <ArrowRight size={16} />
        </Btn>
        <Btn label={`Enclosing folder (${kbd("Mod+↑")})`} disabled={!tab.listing?.parent} onClick={() => tabs.up(tab.id)}>
          <ArrowUp size={16} />
        </Btn>
      </div>
      <PathBar tab={tab} />
      <Btn
        label={isFav ? "Remove from Favorites" : "Add to Favorites"}
        active={isFav}
        onClick={() => s.toggleFavorite({ fs: tab.fs, path: tab.path, name: crumbs[crumbs.length - 1]?.name || tab.path })}
      >
        <Star size={15} className={isFav ? "fill-accent text-accent" : ""} />
      </Btn>
      <Btn
        label="Refresh"
        onClick={() => {
          useSizes.getState().bump();
          void tabs.refresh(tab.id);
        }}
      >
        <RefreshCw size={14} />
      </Btn>
      <span className="mx-0.5 h-5 w-px bg-line" />
      <Btn label={`New folder (${kbd("Mod+Shift+N")})`} onClick={() => newFolder()}>
        <FolderPlus size={16} />
      </Btn>
      <Btn label="New file" onClick={() => newFile()}>
        <FilePlus size={15} />
      </Btn>
      <span className="mx-0.5 h-5 w-px bg-line" />
      <div className="relative flex rounded-[9px] bg-surface-2 p-[2px]">
        {(["list", "grid"] as const).map((v) => (
          <button
            key={v}
            onClick={() => tabs.patch(tab.id, { view: v })}
            className={`relative flex h-[26px] w-[30px] items-center justify-center rounded-[7px] ${tab.view === v ? "text-ink" : "text-ink-3 hover:text-ink-2"}`}
            title={v === "list" ? `List (${kbd("Mod+1")})` : `Icons (${kbd("Mod+2")})`}
            aria-label={v === "list" ? "List view" : "Icon view"}
          >
            {tab.view === v && <motion.span layoutId={`view-${tab.id}`} className="absolute inset-0 rounded-[7px] bg-surface shadow-soft" transition={{ type: "spring", stiffness: 600, damping: 40 }} />}
            <span className="relative">{v === "list" ? <List size={15} /> : <LayoutGrid size={14} />}</span>
          </button>
        ))}
      </div>
      <Btn label="Sort" onClick={sortMenu}>
        <ArrowDownWideNarrow size={15} />
      </Btn>
      <Btn label={s.showHidden ? `Hide hidden files (${kbd("Mod+Shift+.")})` : `Show hidden files (${kbd("Mod+Shift+.")})`} active={s.showHidden} onClick={() => s.set({ showHidden: !s.showHidden })}>
        {s.showHidden ? <Eye size={15} /> : <EyeOff size={15} />}
      </Btn>
      <Btn label={`Preview pane (${kbd("Mod+Alt+P")})`} active={s.previewPane} onClick={() => s.set({ previewPane: !s.previewPane })}>
        <PanelRight size={15} />
      </Btn>
      <SearchBox tab={tab} />
    </div>
  );
}
