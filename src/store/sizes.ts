// Folder sizes shown in the Size column, filled by background measurements.
import { create } from "zustand";
import type { FsId } from "../lib/types";

export interface FolderSize {
  bytes: number;
  files: number;
  /** False while still counting (bytes is a running total). */
  done: boolean;
}

const key = (fs: FsId, path: string) => `${fs}\n${path}`;

interface SizesState {
  sizes: Record<string, FolderSize>;
  /** Bumped by "Refresh" so views re-measure instead of using cached results. */
  nonce: number;
  set: (fs: FsId, path: string, s: FolderSize) => void;
  bump: () => void;
}

export const useSizes = create<SizesState>((set) => ({
  sizes: {},
  nonce: 0,
  set: (fs, path, s) => set((st) => ({ sizes: { ...st.sizes, [key(fs, path)]: s } })),
  bump: () => set((st) => ({ nonce: st.nonce + 1 })),
}));

export const folderSize = (sizes: Record<string, FolderSize>, fs: FsId, path: string): FolderSize | undefined => sizes[key(fs, path)];
