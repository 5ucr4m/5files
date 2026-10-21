import { create } from "zustand";
import type { Loc } from "../lib/types";

export type ThemePref = "system" | "light" | "dark";
export type ViewMode = "list" | "grid";
export type Density = "comfortable" | "compact";

export const ACCENTS = {
  ultramarine: { light: "#3b4cf5", dark: "#7b88ff", label: "Ultramarine" },
  lagoon: { light: "#0b8f8f", dark: "#2cc7c0", label: "Lagoon" },
  orchid: { light: "#a23ad9", dark: "#c47cf5", label: "Orchid" },
  ember: { light: "#d9480f", dark: "#ff8a4c", label: "Ember" },
  moss: { light: "#3f8a1f", dark: "#7fcf4f", label: "Moss" },
  graphite: { light: "#3a3f4b", dark: "#c3c8d4", label: "Graphite" },
} as const;
export type AccentKey = keyof typeof ACCENTS;

export interface Favorite extends Loc {
  name: string;
}

interface SettingsState {
  theme: ThemePref;
  accent: AccentKey;
  showHidden: boolean;
  foldersFirst: boolean;
  defaultView: ViewMode;
  density: Density;
  gridSize: number;
  confirmTrash: boolean;
  previewPane: boolean;
  sidebarWidth: number;
  reduceMotion: boolean;
  /** Measure folder sizes automatically (local folders, not at a drive's top level). */
  folderSizes: boolean;
  favorites: Favorite[];
  set: (patch: Partial<Omit<SettingsState, "set" | "toggleFavorite">>) => void;
  toggleFavorite: (f: Favorite) => void;
}

const KEY = "5files.settings.v1";

function load(): Partial<SettingsState> {
  try {
    return JSON.parse(localStorage.getItem(KEY) || "{}");
  } catch {
    return {};
  }
}

export const useSettings = create<SettingsState>((set, get) => ({
  theme: "system",
  accent: "ultramarine",
  showHidden: false,
  foldersFirst: true,
  defaultView: "list",
  density: "comfortable",
  gridSize: 112,
  confirmTrash: false,
  previewPane: false,
  sidebarWidth: 232,
  reduceMotion: false,
  folderSizes: true,
  favorites: [],
  ...load(),
  set: (patch) => set(patch),
  toggleFavorite: (f) => {
    const favs = get().favorites;
    const exists = favs.some((x) => x.fs === f.fs && x.path === f.path);
    set({ favorites: exists ? favs.filter((x) => !(x.fs === f.fs && x.path === f.path)) : [...favs, f] });
  },
}));

useSettings.subscribe((s) => {
  const { set: _s, toggleFavorite: _t, ...data } = s;
  try {
    localStorage.setItem(KEY, JSON.stringify(data));
  } catch {
    /* storage unavailable */
  }
});

export function resolvedTheme(pref: ThemePref): "light" | "dark" {
  if (pref !== "system") return pref;
  return typeof matchMedia !== "undefined" && matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}
