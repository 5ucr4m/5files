// Every keyboard shortcut, in one place: the shortcuts sheet, menus and the command
// palette read from here so what's documented is what's wired.
// "Mod" is ⌘ on macOS and Ctrl elsewhere (see platform.kbd).

export interface Shortcut {
  keys: string[]; // one or more alternatives, e.g. ["Mod+T"]
  label: string;
  /** Typed after the tmux prefix (Ctrl+B). */
  prefix?: boolean;
}

export interface ShortcutGroup {
  title: string;
  note?: string;
  items: Shortcut[];
}

export const SHORTCUTS: ShortcutGroup[] = [
  {
    title: "Tabs",
    items: [
      { keys: ["Mod+T"], label: "New tab" },
      { keys: ["Mod+W"], label: "Close pane (the tab when it's the last one)" },
      { keys: ["Ctrl+Tab", "Mod+Shift+]"], label: "Next tab" },
      { keys: ["Ctrl+Shift+Tab", "Mod+Shift+["], label: "Previous tab" },
      { keys: ["Mod+Alt+1…9"], label: "Go to tab 1–9" },
    ],
  },
  {
    title: "Panes",
    items: [
      { keys: ["Mod+\\"], label: "Split right" },
      { keys: ["Mod+Shift+\\"], label: "Split down" },
      { keys: ["Mod+Alt+←↑→↓"], label: "Move to the pane in that direction" },
      { keys: ["Mod+Shift+Enter"], label: "Zoom pane (toggle)" },
      { keys: ["F5"], label: "Copy selection to the other pane" },
      { keys: ["F6"], label: "Move selection to the other pane" },
    ],
  },
  {
    title: "tmux keys",
    note: "Press Ctrl+B, release, then the key.",
    items: [
      { keys: ["%"], label: "Split right", prefix: true },
      { keys: ['"'], label: "Split down", prefix: true },
      { keys: ["←↑→↓"], label: "Move between panes", prefix: true },
      { keys: ["o"], label: "Next pane", prefix: true },
      { keys: [";"], label: "Previous pane", prefix: true },
      { keys: ["z"], label: "Zoom pane", prefix: true },
      { keys: ["x"], label: "Close pane", prefix: true },
      { keys: ["c"], label: "New tab", prefix: true },
      { keys: ["n", "p"], label: "Next / previous tab", prefix: true },
      { keys: ["1…9"], label: "Go to tab", prefix: true },
      { keys: ["?"], label: "Show this list", prefix: true },
    ],
  },
  {
    title: "Navigation",
    items: [
      { keys: ["Mod+[", "Alt+←"], label: "Back" },
      { keys: ["Mod+]", "Alt+→"], label: "Forward" },
      { keys: ["Mod+↑", "Alt+↑"], label: "Enclosing folder" },
      { keys: ["Mod+↓", "Enter"], label: "Open selection" },
      { keys: ["Mod+L"], label: "Go to folder (edit path)" },
      { keys: ["Mod+F"], label: "Filter / search subfolders" },
      { keys: ["Mod+K"], label: "Command palette" },
    ],
  },
  {
    title: "Files",
    items: [
      { keys: ["Mod+C", "Mod+X", "Mod+V"], label: "Copy, cut, paste" },
      { keys: ["Mod+D"], label: "Duplicate" },
      { keys: ["F2"], label: "Rename" },
      { keys: ["Mod+Shift+N"], label: "New folder" },
      { keys: ["Mod+Backspace", "Delete"], label: "Move to Trash" },
      { keys: ["Space"], label: "Quick Look" },
      { keys: ["Mod+A"], label: "Select all" },
      { keys: ["Mod+Alt+C"], label: "Copy path" },
      { keys: ["Mod+E"], label: "Eject this drive" },
    ],
  },
  {
    title: "View",
    items: [
      { keys: ["Mod+1"], label: "List" },
      { keys: ["Mod+2"], label: "Icons" },
      { keys: ["Mod+Shift+."], label: "Show hidden files" },
      { keys: ["Mod+I", "Mod+Alt+P"], label: "Preview pane" },
      { keys: ["Mod+R", "F5"], label: "Refresh (F5 copies when split)" },
      { keys: ["Mod+,"], label: "Settings" },
      { keys: ["Mod+/"], label: "This list" },
    ],
  },
];
