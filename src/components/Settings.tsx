import { Check, Monitor, Moon, Sun, X } from "lucide-react";
import { motion } from "motion/react";
import { ACCENTS, resolvedTheme, useSettings, type AccentKey, type ThemePref } from "../store/settings";
import { useUi } from "../store/ui";

function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-6 py-3">
      <div>
        <p className="text-[13px] font-medium">{label}</p>
        {hint && <p className="text-2xs text-ink-3">{hint}</p>}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

export function Toggle({ on, onChange, label }: { on: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button role="switch" aria-checked={on} aria-label={label} onClick={() => onChange(!on)} className={`relative h-[20px] w-[36px] rounded-full transition-colors ${on ? "bg-accent" : "bg-line-strong"}`}>
      <motion.span className="absolute top-[2px] size-[16px] rounded-full bg-white shadow" animate={{ left: on ? 18 : 2 }} transition={{ type: "spring", stiffness: 700, damping: 35 }} />
    </button>
  );
}

export function Segmented<T extends string>({ value, options, onChange, id }: { value: T; options: { value: T; label: string; icon?: React.ReactNode }[]; onChange: (v: T) => void; id: string }) {
  return (
    <div className="flex rounded-[9px] bg-surface-3 p-[2px]">
      {options.map((o) => (
        <button key={o.value} onClick={() => onChange(o.value)} className={`relative flex h-[26px] items-center gap-1.5 rounded-[7px] px-2.5 text-[12px] ${value === o.value ? "text-ink" : "text-ink-3 hover:text-ink-2"}`}>
          {value === o.value && <motion.span layoutId={id} className="absolute inset-0 rounded-[7px] bg-surface shadow-soft" transition={{ type: "spring", stiffness: 600, damping: 40 }} />}
          <span className="relative flex items-center gap-1.5">
            {o.icon}
            {o.label}
          </span>
        </button>
      ))}
    </div>
  );
}

export function SettingsPanel() {
  const s = useSettings();
  const dark = resolvedTheme(s.theme) === "dark";
  return (
    <div className="flex max-h-[min(640px,85vh)] flex-col">
      <div className="flex h-14 shrink-0 items-center border-b border-line px-5">
        <h2 className="flex-1 text-[15px] font-semibold tracking-tight">Settings</h2>
        <button onClick={() => useUi.getState().set({ panel: null })} className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-surface-3 hover:text-ink" aria-label="Close">
          <X size={16} />
        </button>
      </div>
      <div className="scroll divide-y divide-line overflow-y-auto px-5 pb-4">
        <Row label="Appearance">
          <Segmented<ThemePref>
            id="theme"
            value={s.theme}
            onChange={(theme) => s.set({ theme })}
            options={[
              { value: "light", label: "Light", icon: <Sun size={13} /> },
              { value: "dark", label: "Dark", icon: <Moon size={13} /> },
              { value: "system", label: "System", icon: <Monitor size={13} /> },
            ]}
          />
        </Row>
        <Row label="Accent color">
          <div className="flex gap-1.5">
            {(Object.keys(ACCENTS) as AccentKey[]).map((k) => (
              <button
                key={k}
                onClick={() => s.set({ accent: k })}
                title={ACCENTS[k].label}
                aria-label={ACCENTS[k].label}
                className="flex size-6 items-center justify-center rounded-full ring-offset-2 ring-offset-elevated transition-transform hover:scale-110"
                style={{ background: dark ? ACCENTS[k].dark : ACCENTS[k].light, boxShadow: s.accent === k ? `0 0 0 2px var(--elevated), 0 0 0 4px ${dark ? ACCENTS[k].dark : ACCENTS[k].light}` : undefined }}
              >
                {s.accent === k && <Check size={12} strokeWidth={3} className={k === "graphite" && dark ? "text-black" : "text-white"} />}
              </button>
            ))}
          </div>
        </Row>
        <Row label="Density" hint="Row height in list view">
          <Segmented
            id="density"
            value={s.density}
            onChange={(density) => s.set({ density })}
            options={[
              { value: "comfortable", label: "Comfortable" },
              { value: "compact", label: "Compact" },
            ]}
          />
        </Row>
        <Row label="Icon size" hint="In icon view">
          <input type="range" min={64} max={192} step={8} value={s.gridSize} onChange={(e) => s.set({ gridSize: Number(e.target.value) })} className="w-40 accent-[var(--accent)]" aria-label="Icon size" />
        </Row>
        <Row label="New tabs open as">
          <Segmented
            id="defview"
            value={s.defaultView}
            onChange={(defaultView) => s.set({ defaultView })}
            options={[
              { value: "list", label: "List" },
              { value: "grid", label: "Icons" },
            ]}
          />
        </Row>
        <Row label="Show folder sizes" hint="Measured in the background for folders on this computer. Off at a drive's top level; right-click → Calculate size works anywhere.">
          <Toggle label="Show folder sizes" on={s.folderSizes} onChange={(folderSizes) => s.set({ folderSizes })} />
        </Row>
        <Row label="Keep running when the window is closed" hint="5files stays in the menu bar / system tray, so syncs and transfers continue. Quit from the tray icon.">
          <Toggle label="Keep running when the window is closed" on={s.keepRunning} onChange={(keepRunning) => s.set({ keepRunning })} />
        </Row>
        <Row label="Show hidden files">
          <Toggle label="Show hidden files" on={s.showHidden} onChange={(showHidden) => s.set({ showHidden })} />
        </Row>
        <Row label="Keep folders on top">
          <Toggle label="Keep folders on top" on={s.foldersFirst} onChange={(foldersFirst) => s.set({ foldersFirst })} />
        </Row>
        <Row label="Ask before moving to Trash">
          <Toggle label="Ask before moving to Trash" on={s.confirmTrash} onChange={(confirmTrash) => s.set({ confirmTrash })} />
        </Row>
        <Row label="Reduce motion" hint="Turns off most animations">
          <Toggle label="Reduce motion" on={s.reduceMotion} onChange={(reduceMotion) => s.set({ reduceMotion })} />
        </Row>
      </div>
    </div>
  );
}
