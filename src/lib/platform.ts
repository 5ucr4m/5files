import type { Platform } from "./types";

let current: Platform = { os: "linux", sep: "/", home: null };

export function setPlatform(p: Platform) {
  current = p;
  document.documentElement.dataset.os = p.os;
}

export const platform = () => current;
export const isMac = () => current.os === "macos";

/** "⌘" on macOS, "Ctrl" elsewhere — for shortcut hints. */
export const mod = () => (isMac() ? "⌘" : "Ctrl+");
export const kbd = (combo: string) =>
  isMac()
    ? combo.replace(/Mod\+/g, "⌘").replace(/Ctrl\+/g, "⌃").replace(/Shift\+/g, "⇧").replace(/Alt\+/g, "⌥").replace(/Backspace/, "⌫").replace(/Enter/, "↩")
    : combo.replace(/Mod\+/g, "Ctrl+");

/** True when the platform's primary modifier is held (⌘ on macOS, Ctrl elsewhere). */
export const isMod = (e: { metaKey: boolean; ctrlKey: boolean }) => (isMac() ? e.metaKey : e.ctrlKey);
