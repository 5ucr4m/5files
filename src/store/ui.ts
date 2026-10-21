import { create } from "zustand";
import type { ConnKind, Conflict, Entry, FsId, Loc } from "../lib/types";

export interface Toast {
  id: number;
  title: string;
  body?: string;
  tone: "info" | "success" | "error";
  action?: { label: string; run: () => void };
}

export interface MenuItem {
  label?: string;
  icon?: React.ComponentType<{ size?: number; className?: string }>;
  shortcut?: string;
  danger?: boolean;
  disabled?: boolean;
  divider?: boolean;
  run?: () => void;
}

export interface ConfirmRequest {
  title: string;
  body?: string;
  confirm: string;
  danger?: boolean;
  resolve: (ok: boolean) => void;
}

export interface ConflictRequest {
  names: string[];
  destName: string;
  resolve: (c: Conflict | null) => void;
}

export type Panel = null | "transfers" | "connections" | "settings";

interface UiState {
  toasts: Toast[];
  menu: { x: number; y: number; items: MenuItem[] } | null;
  confirm: ConfirmRequest | null;
  conflict: ConflictRequest | null;
  palette: boolean;
  panel: Panel;
  /** Connection being edited in the connections panel (id), or a kind for a new one. */
  editing: { id?: string; kind?: ConnKind } | null;
  quickLook: { fs: FsId; entry: Entry } | null;
  clipboard: { mode: "copy" | "cut"; items: Loc[] } | null;
  dropTarget: string | null;
  toast: (t: Omit<Toast, "id">) => void;
  dismiss: (id: number) => void;
  openMenu: (x: number, y: number, items: MenuItem[]) => void;
  closeMenu: () => void;
  set: (patch: Partial<UiState>) => void;
}

let toastId = 0;

export const useUi = create<UiState>((set, get) => ({
  toasts: [],
  menu: null,
  confirm: null,
  conflict: null,
  palette: false,
  panel: null,
  editing: null,
  quickLook: null,
  clipboard: null,
  dropTarget: null,
  toast(t) {
    const id = ++toastId;
    set((s) => ({ toasts: [...s.toasts.slice(-3), { ...t, id }] }));
    setTimeout(() => get().dismiss(id), t.tone === "error" ? 7000 : 3800);
  },
  dismiss: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
  openMenu: (x, y, items) => set({ menu: { x, y, items } }),
  closeMenu: () => set({ menu: null }),
  set: (patch) => set(patch),
}));

export function confirmDialog(req: Omit<ConfirmRequest, "resolve">): Promise<boolean> {
  return new Promise((resolve) => useUi.getState().set({ confirm: { ...req, resolve } }));
}

export function conflictDialog(names: string[], destName: string): Promise<Conflict | null> {
  return new Promise((resolve) => useUi.getState().set({ conflict: { names, destName, resolve } }));
}

export const toast = (t: Omit<Toast, "id">) => useUi.getState().toast(t);
