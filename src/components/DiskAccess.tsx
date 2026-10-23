import { ShieldCheck, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect } from "react";
import { create } from "zustand";
import { api } from "../lib/api";
import { useSettings } from "../store/settings";

// macOS asks for permission folder by folder (Documents, Desktop, Downloads, external and
// network drives) unless 5files has Full Disk Access. This offers the one switch that
// makes all of those prompts go away.

interface DiskAccessState {
  /** null: not applicable (Windows / Linux) or not checked yet. */
  granted: boolean | null;
  /** System Settings was opened from here: macOS needs a relaunch to apply the switch. */
  opened: boolean;
  check: () => Promise<void>;
  open: () => Promise<void>;
}

export const useDiskAccess = create<DiskAccessState>((set) => ({
  granted: null,
  opened: false,
  async check() {
    set({ granted: await api().fullDiskAccess().catch(() => null) });
  },
  async open() {
    await api().openPrivacySettings();
    set({ opened: true });
  },
}));

/** Keeps the status fresh: on start and whenever the window comes back to the front. */
export function useDiskAccessWatch() {
  useEffect(() => {
    const check = () => void useDiskAccess.getState().check();
    check();
    window.addEventListener("focus", check);
    return () => window.removeEventListener("focus", check);
  }, []);
}

const HOW = "Turn on 5files in System Settings → Privacy & Security → Full Disk Access. When macOS asks, choose Quit & Reopen.";

/** A card that floats above the workspace until access is granted or dismissed. */
export function DiskAccessNotice() {
  const { granted, opened, open } = useDiskAccess();
  const dismissed = useSettings((s) => s.diskAccessNoticeDismissed);
  const show = granted === false && !dismissed;
  return (
    <AnimatePresence>
      {show && (
        <motion.div
          initial={{ opacity: 0, y: 16 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: 12 }}
          transition={{ type: "spring", stiffness: 420, damping: 36 }}
          role="region"
          aria-label="Full Disk Access"
          className="fixed bottom-5 left-1/2 z-[60] flex w-[min(560px,calc(100vw-32px))] -translate-x-1/2 items-start gap-3 rounded-2xl border border-line bg-elevated p-4 shadow-pop"
        >
          <span className="flex size-9 shrink-0 items-center justify-center rounded-xl bg-[color-mix(in_oklab,var(--accent)_14%,transparent)] text-accent">
            <ShieldCheck size={18} />
          </span>
          <div className="min-w-0 flex-1">
            <p className="text-[13.5px] font-semibold tracking-tight">{opened ? "Almost there" : "Stop the permission prompts"}</p>
            <p className="mt-0.5 text-[12.5px] leading-relaxed text-ink-3">
              {opened
                ? HOW
                : "macOS asks again for every protected folder until 5files has Full Disk Access. Turn it on once and every folder and drive just opens."}
            </p>
            <div className="mt-3 flex gap-2">
              <button onClick={() => void open()} className="h-8 rounded-lg bg-accent px-3 text-[12.5px] font-medium text-white hover:brightness-110">
                {opened ? "Open System Settings again" : "Open System Settings"}
              </button>
              <button
                onClick={() => useSettings.getState().set({ diskAccessNoticeDismissed: true })}
                className="h-8 rounded-lg px-3 text-[12.5px] font-medium text-ink-3 hover:bg-surface-3 hover:text-ink"
              >
                Not now
              </button>
            </div>
          </div>
          <button
            onClick={() => useSettings.getState().set({ diskAccessNoticeDismissed: true })}
            className="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-4 hover:bg-surface-3 hover:text-ink"
            aria-label="Dismiss"
          >
            <X size={15} />
          </button>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

/** Status + button for the Settings panel (shown on macOS only). */
export function DiskAccessSetting() {
  const { granted, open } = useDiskAccess();
  if (granted === null) return null;
  return granted ? (
    <span className="flex items-center gap-1.5 text-[12px] font-medium text-success">
      <ShieldCheck size={14} /> Allowed
    </span>
  ) : (
    <button onClick={() => void open()} className="h-7 rounded-lg border border-line-strong px-2.5 text-[12px] font-medium hover:bg-surface-2">
      Open System Settings
    </button>
  );
}
