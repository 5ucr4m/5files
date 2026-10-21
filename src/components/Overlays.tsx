import { AlertTriangle, CheckCircle2, Info, X, XCircle } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Conflict } from "../lib/types";
import { useUi } from "../store/ui";

export function ContextMenu() {
  const menu = useUi((s) => s.menu);
  const close = useUi((s) => s.closeMenu);
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x: 0, y: 0 });
  const [hi, setHi] = useState(-1);

  useLayoutEffect(() => {
    if (!menu || !ref.current) return;
    const r = ref.current.getBoundingClientRect();
    setPos({ x: Math.min(menu.x, window.innerWidth - r.width - 8), y: Math.min(menu.y, window.innerHeight - r.height - 8) });
    setHi(-1);
  }, [menu]);

  useEffect(() => {
    if (!menu) return;
    const items = menu.items;
    const selectable = items.map((it, i) => (!it.divider && !it.disabled ? i : -1)).filter((i) => i >= 0);
    const onKey = (e: KeyboardEvent) => {
      e.stopPropagation();
      if (e.key === "Escape") return close();
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        setHi((h) => {
          const at = selectable.indexOf(h);
          const next = e.key === "ArrowDown" ? (at + 1) % selectable.length : (at - 1 + selectable.length) % selectable.length;
          return selectable[at < 0 && e.key === "ArrowUp" ? selectable.length - 1 : next];
        });
      }
      if (e.key === "Enter") {
        e.preventDefault();
        setHi((h) => {
          if (h >= 0) {
            close();
            items[h].run?.();
          }
          return h;
        });
      }
    };
    const onDown = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && close();
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
    };
  }, [menu, close]);

  return (
    <AnimatePresence>
      {menu && (
        <motion.div
          ref={ref}
          role="menu"
          initial={{ opacity: 0, scale: 0.96 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0, scale: 0.98, transition: { duration: 0.08 } }}
          transition={{ type: "spring", stiffness: 700, damping: 40 }}
          style={{ left: pos.x, top: pos.y, transformOrigin: "top left" }}
          className="fixed z-[100] min-w-[220px] rounded-xl border border-line bg-elevated p-1 shadow-pop"
          onContextMenu={(e) => e.preventDefault()}
        >
          {menu.items.map((it, i) =>
            it.divider ? (
              <div key={i} className="mx-2 my-1 h-px bg-line" />
            ) : (
              <button
                key={i}
                role="menuitem"
                disabled={it.disabled}
                onMouseEnter={() => setHi(i)}
                onClick={() => {
                  close();
                  it.run?.();
                }}
                className={`flex h-[28px] w-full items-center gap-2.5 rounded-[7px] px-2.5 text-left text-[12.5px] disabled:opacity-35 ${
                  hi === i ? (it.danger ? "bg-danger text-white" : "bg-accent text-white") : it.danger ? "text-danger" : ""
                }`}
              >
                <span className="flex w-4 justify-center">{it.icon && <it.icon size={14} />}</span>
                <span className="flex-1 whitespace-pre">{it.label}</span>
                {it.shortcut && <span className={`num text-[11px] ${hi === i ? "text-white/80" : "text-ink-4"}`}>{it.shortcut}</span>}
              </button>
            ),
          )}
        </motion.div>
      )}
    </AnimatePresence>
  );
}

export function Modal({ open, onClose, children, width = 420 }: { open: boolean; onClose: () => void; children: React.ReactNode; width?: number }) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, onClose]);
  return (
    <AnimatePresence>
      {open && (
        <motion.div className="fixed inset-0 z-[90] flex items-center justify-center p-6" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.15 }}>
          <div className="absolute inset-0 bg-black/30 backdrop-blur-[2px]" onMouseDown={onClose} />
          <motion.div
            role="dialog"
            aria-modal
            initial={{ opacity: 0, y: 12, scale: 0.97 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 6, scale: 0.98 }}
            transition={{ type: "spring", stiffness: 520, damping: 36 }}
            className="relative max-h-full overflow-hidden rounded-2xl border border-line bg-elevated shadow-pop"
            style={{ width }}
          >
            {children}
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

export function ConfirmDialog() {
  const req = useUi((s) => s.confirm);
  const done = (ok: boolean) => {
    req?.resolve(ok);
    useUi.getState().set({ confirm: null });
  };
  const btn = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (req) setTimeout(() => btn.current?.focus(), 30);
  }, [req]);
  return (
    <Modal open={!!req} onClose={() => done(false)} width={400}>
      {req && (
        <div className="p-5">
          <div className="flex gap-3.5">
            {req.danger && (
              <div className="flex size-9 shrink-0 items-center justify-center rounded-xl bg-[color-mix(in_oklab,var(--danger)_14%,transparent)] text-danger">
                <AlertTriangle size={18} />
              </div>
            )}
            <div>
              <h3 className="text-[14px] font-semibold tracking-tight">{req.title}</h3>
              {req.body && <p className="mt-1 text-[12.5px] text-ink-3">{req.body}</p>}
            </div>
          </div>
          <div className="mt-5 flex justify-end gap-2">
            <button onClick={() => done(false)} className="h-8 rounded-lg px-3.5 text-[12.5px] font-medium text-ink-2 hover:bg-surface-3">
              Cancel
            </button>
            <button
              ref={btn}
              onClick={() => done(true)}
              className={`h-8 rounded-lg px-3.5 text-[12.5px] font-medium text-white ${req.danger ? "bg-danger hover:brightness-110" : "bg-accent hover:brightness-110"}`}
            >
              {req.confirm}
            </button>
          </div>
        </div>
      )}
    </Modal>
  );
}

const CONFLICT_OPTIONS: { value: Conflict; title: string; body: string }[] = [
  { value: "keepBoth", title: "Keep both", body: "New copies get a number, like “report (2).pdf”." },
  { value: "overwrite", title: "Replace", body: "Overwrite files with the same name. Folders are merged." },
  { value: "skip", title: "Skip", body: "Leave existing items untouched and transfer the rest." },
];

export function ConflictDialog() {
  const req = useUi((s) => s.conflict);
  const [choice, setChoice] = useState<Conflict>("keepBoth");
  const done = (c: Conflict | null) => {
    req?.resolve(c);
    useUi.getState().set({ conflict: null });
  };
  useEffect(() => setChoice("keepBoth"), [req]);
  const n = req?.names.length ?? 0;
  return (
    <Modal open={!!req} onClose={() => done(null)} width={440}>
      {req && (
        <div className="p-5">
          <h3 className="text-[14px] font-semibold tracking-tight">
            {n === 1 ? `“${req.names[0]}” already exists in “${req.destName}”` : `${n} items already exist in “${req.destName}”`}
          </h3>
          {n > 1 && (
            <p className="num mt-1 truncate text-2xs text-ink-3" title={req.names.join(", ")}>
              {req.names.slice(0, 4).join(", ")}
              {n > 4 ? ` and ${n - 4} more` : ""}
            </p>
          )}
          <div className="mt-4 space-y-1.5" role="radiogroup">
            {CONFLICT_OPTIONS.map((o) => (
              <button
                key={o.value}
                role="radio"
                aria-checked={choice === o.value}
                onClick={() => setChoice(o.value)}
                onDoubleClick={() => done(o.value)}
                className={`flex w-full items-start gap-3 rounded-xl border p-3 text-left transition-colors ${choice === o.value ? "border-accent bg-sel" : "border-line hover:bg-surface-2"}`}
              >
                <span className={`mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full border-[1.5px] ${choice === o.value ? "border-accent" : "border-line-strong"}`}>
                  {choice === o.value && <motion.span layoutId="conflict-dot" className="size-2 rounded-full bg-accent" />}
                </span>
                <span>
                  <span className="block text-[13px] font-medium">{o.title}</span>
                  <span className="block text-2xs text-ink-3">{o.body}</span>
                </span>
              </button>
            ))}
          </div>
          <div className="mt-5 flex justify-end gap-2">
            <button onClick={() => done(null)} className="h-8 rounded-lg px-3.5 text-[12.5px] font-medium text-ink-2 hover:bg-surface-3">
              Cancel
            </button>
            <button autoFocus onClick={() => done(choice)} className="h-8 rounded-lg bg-accent px-3.5 text-[12.5px] font-medium text-white hover:brightness-110">
              Continue
            </button>
          </div>
        </div>
      )}
    </Modal>
  );
}

export function Toasts() {
  const toasts = useUi((s) => s.toasts);
  const dismiss = useUi((s) => s.dismiss);
  return (
    <div className="pointer-events-none fixed right-4 bottom-10 z-[95] flex w-[340px] flex-col items-end gap-2">
      <AnimatePresence initial={false}>
        {toasts.map((t) => {
          const Icon = t.tone === "success" ? CheckCircle2 : t.tone === "error" ? XCircle : Info;
          const color = t.tone === "success" ? "var(--success)" : t.tone === "error" ? "var(--danger)" : "var(--accent)";
          return (
            <motion.div
              key={t.id}
              layout
              initial={{ opacity: 0, y: 16, scale: 0.96 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, x: 40, transition: { duration: 0.15 } }}
              transition={{ type: "spring", stiffness: 520, damping: 36 }}
              className="pointer-events-auto flex w-full items-start gap-3 rounded-xl border border-line bg-elevated p-3 shadow-pop"
              role="status"
            >
              <Icon size={16} style={{ color }} className="mt-px shrink-0" />
              <div className="min-w-0 flex-1">
                <p className="text-[12.5px] font-medium">{t.title}</p>
                {t.body && <p className="mt-0.5 line-clamp-3 text-2xs break-words text-ink-3">{t.body}</p>}
                {t.action && (
                  <button
                    onClick={() => {
                      t.action!.run();
                      dismiss(t.id);
                    }}
                    className="mt-1.5 text-2xs font-semibold text-accent hover:underline"
                  >
                    {t.action.label}
                  </button>
                )}
              </div>
              <button onClick={() => dismiss(t.id)} className="text-ink-4 hover:text-ink" aria-label="Dismiss">
                <X size={13} />
              </button>
            </motion.div>
          );
        })}
      </AnimatePresence>
    </div>
  );
}
