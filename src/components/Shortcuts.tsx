import { Search, X } from "lucide-react";
import { useMemo, useState } from "react";
import { isMac } from "../lib/platform";
import { SHORTCUTS } from "../lib/shortcuts";
import { useUi } from "../store/ui";

const MAC: Record<string, string> = { Mod: "⌘", Shift: "⇧", Alt: "⌥", Ctrl: "⌃", Enter: "↩", Backspace: "⌫", Delete: "⌦", Tab: "⇥" };
const PC: Record<string, string> = { Mod: "Ctrl" };

/** "Mod+Shift+\" -> keycaps for this platform. */
export function Keys({ combo, small = false }: { combo: string; small?: boolean }) {
  const map = isMac() ? MAC : PC;
  // Split on "+" but keep a literal trailing "+" key if there ever is one.
  const parts = combo.split(/\+(?!$)/).map((p) => map[p] ?? p);
  return (
    <span className="inline-flex items-center gap-[3px]">
      {parts.map((p, i) => (
        <kbd
          key={i}
          className={`num inline-flex min-w-[20px] items-center justify-center rounded-[5px] border border-line border-b-[2px] bg-surface px-1 font-medium text-ink-2 ${small ? "h-[18px] text-[10.5px]" : "h-[22px] text-[11.5px]"}`}
        >
          {p}
        </kbd>
      ))}
    </span>
  );
}

export function ShortcutsSheet() {
  const [q, setQ] = useState("");
  const groups = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return SHORTCUTS.map((g) => ({
      ...g,
      items: g.items.filter((s) => !needle || s.label.toLowerCase().includes(needle) || g.title.toLowerCase().includes(needle)),
    })).filter((g) => g.items.length);
  }, [q]);

  return (
    <div className="flex max-h-[min(720px,88vh)] flex-col">
      <div className="flex h-14 shrink-0 items-center gap-3 border-b border-line px-5">
        <h2 className="text-[15px] font-semibold tracking-tight">Keyboard shortcuts</h2>
        <div className="ml-auto flex h-8 w-[220px] items-center gap-2 rounded-lg border border-line bg-surface-2/70 px-2.5 focus-within:border-accent">
          <Search size={13} className="text-ink-3" />
          <input
            autoFocus
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => e.stopPropagation()}
            placeholder="Find a shortcut"
            className="min-w-0 flex-1 bg-transparent text-[12.5px] outline-none placeholder:text-ink-4"
            aria-label="Find a shortcut"
          />
        </div>
        <button onClick={() => useUi.getState().set({ panel: null })} className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-surface-3 hover:text-ink" aria-label="Close">
          <X size={16} />
        </button>
      </div>
      <div className="scroll overflow-y-auto px-5 py-4">
        {groups.length === 0 && <p className="py-10 text-center text-[12.5px] text-ink-3">No shortcut matches “{q}”.</p>}
        <div className="columns-2 gap-6 [column-fill:_balance]">
          {groups.map((g) => (
            <section key={g.title} className="mb-5 break-inside-avoid">
              <div className="mb-1.5 flex items-baseline gap-2">
                <h3 className="text-[11px] font-semibold tracking-[0.08em] text-ink-4 uppercase">{g.title}</h3>
                {g.note && <span className="text-2xs text-ink-4">{g.note}</span>}
              </div>
              <div className="divide-y divide-line rounded-xl border border-line">
                {g.items.map((s) => (
                  <div key={s.label} className="flex items-center justify-between gap-3 px-3 py-[7px]">
                    <span className="min-w-0 text-[12.5px] text-ink-2">{s.label}</span>
                    <span className="flex shrink-0 items-center gap-1.5">
                      {s.prefix && (
                        <>
                          <Keys combo="Ctrl+B" small />
                          <span className="text-[10px] text-ink-4">then</span>
                        </>
                      )}
                      {s.keys.map((k, i) => (
                        <span key={k} className="flex items-center gap-1.5">
                          {i > 0 && <span className="text-[10px] text-ink-4">or</span>}
                          <Keys combo={k} small={!!s.prefix} />
                        </span>
                      ))}
                    </span>
                  </div>
                ))}
              </div>
            </section>
          ))}
        </div>
      </div>
    </div>
  );
}
