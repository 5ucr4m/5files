// Starts / stops background folder measurements and feeds the size store.
import { api } from "./api";
import type { FsId, Volume } from "./types";
import { useSizes } from "../store/sizes";

/** Measures `paths` on `fs`; returns a function that stops the measurement. */
export function measureFolders(fs: FsId, paths: string[], force = false): () => void {
  if (!paths.length) return () => {};
  let id: string | null = null;
  let stopped = false;
  const store = useSizes.getState();
  void api()
    .folderSizes(fs, paths, force, (e) => {
      if (stopped) return;
      if (e.type === "error") return;
      store.set(fs, e.path, { bytes: e.bytes, files: e.files, done: e.type === "done" });
    })
    .then((taskId) => {
      id = taskId;
      if (stopped) void api().cancelSearch(taskId);
    })
    .catch(() => {});
  return () => {
    stopped = true;
    if (id) void api().cancelSearch(id);
  };
}

const trim = (p: string) => (p.length > 1 ? p.replace(/[\\/]+$/, "") : p);

/** A drive's top level (`/`, `C:\`, `/Volumes/USB`): too big to measure automatically. */
export function isVolumeRoot(fs: FsId, path: string, parent: string | null, volumes: Volume[]): boolean {
  if (fs !== "local") return parent === null;
  if (parent === null) return true;
  const p = trim(path).toLowerCase();
  return volumes.some((v) => trim(v.mount).toLowerCase() === p);
}
