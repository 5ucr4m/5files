// Starts / stops background folder measurements and feeds the size store.
import { api } from "./api";
import type { FsId, Volume } from "./types";
import { useSizes } from "../store/sizes";

interface Batch {
  id: string | null;
  keys: string[];
  cancelled: boolean;
}

/** Folder measurements in progress, shared by every pane that shows the folder. */
const inflight = new Map<string, { batch: Batch; refs: number }>();
const key = (fs: FsId, path: string) => `${fs}\n${path}`;

/** Grace period before an unwanted measurement is cancelled: a listing refresh or a
 * second pane re-requesting the same folders takes over the running count instead of
 * restarting it from zero. */
const CANCEL_AFTER_MS = 1500;

function cancelIfUnwanted(batch: Batch) {
  if (batch.cancelled) return;
  const wanted = batch.keys.some((k) => {
    const e = inflight.get(k);
    return e?.batch === batch && e.refs > 0;
  });
  if (wanted) return;
  batch.cancelled = true;
  for (const k of batch.keys) if (inflight.get(k)?.batch === batch) inflight.delete(k);
  if (batch.id) void api().cancelSearch(batch.id);
}

/**
 * Measures `paths` on `fs` (joining measurements already running for the same folders).
 * Returns a function that releases interest; the count stops only when nobody needs it.
 */
export function measureFolders(fs: FsId, paths: string[], force = false): () => void {
  if (!paths.length) return () => {};
  const mine: string[] = [];
  const fresh: string[] = [];
  for (const p of paths) {
    const k = key(fs, p);
    const running = inflight.get(k);
    mine.push(k);
    if (running && !running.batch.cancelled) running.refs++;
    else fresh.push(p);
  }
  if (fresh.length) {
    const batch: Batch = { id: null, keys: fresh.map((p) => key(fs, p)), cancelled: false };
    for (const k of batch.keys) inflight.set(k, { batch, refs: 1 });
    const store = useSizes.getState();
    void api()
      .folderSizes(fs, fresh, force, (e) => {
        if (batch.cancelled) return;
        const k = key(fs, e.path);
        if (e.type !== "error") store.set(fs, e.path, { bytes: e.bytes, files: e.files, done: e.type === "done" });
        if (e.type !== "progress" && inflight.get(k)?.batch === batch) inflight.delete(k);
      })
      .then((taskId) => {
        batch.id = taskId;
        if (batch.cancelled) void api().cancelSearch(taskId);
      })
      .catch(() => {
        for (const k of batch.keys) if (inflight.get(k)?.batch === batch) inflight.delete(k);
      });
  }
  let released = false;
  return () => {
    if (released) return;
    released = true;
    const batches = new Set<Batch>();
    for (const k of mine) {
      const e = inflight.get(k);
      if (e) {
        e.refs--;
        batches.add(e.batch);
      }
    }
    setTimeout(() => batches.forEach(cancelIfUnwanted), CANCEL_AFTER_MS);
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
