import type {
  Connection,
  ConnectionInput,
  DirSize,
  Entry,
  FsId,
  Job,
  JobRequest,
  Listing,
  Loc,
  MetricsSnapshot,
  Place,
  Platform,
  SearchEvent,
  SizeEvent,
  SyncInput,
  SyncPair,
  SyncPreview,
  TextPreview,
  Volume,
} from "./types";

export type Unlisten = () => void;

/** Everything the UI needs from the backend. Implemented by Tauri, and by an in-memory mock for the browser/tests. */
export interface Backend {
  listDir(fs: FsId, path?: string | null): Promise<Listing>;
  stat(fs: FsId, path: string): Promise<Entry>;
  createDir(fs: FsId, parent: string, name: string): Promise<Entry>;
  createFile(fs: FsId, parent: string, name: string): Promise<Entry>;
  rename(fs: FsId, path: string, newName: string): Promise<Entry>;
  trash(paths: string[]): Promise<void>;
  checkConflicts(sources: Loc[], dest: Loc): Promise<string[]>;
  enqueue(req: JobRequest): Promise<string>;
  transfers(): Promise<Job[]>;
  pauseTransfer(id: string, paused: boolean): Promise<void>;
  cancelTransfer(id: string): Promise<void>;
  retryTransfer(id: string): Promise<string>;
  removeTransfer(id: string): Promise<void>;
  clearTransfers(): Promise<void>;
  readText(fs: FsId, path: string, maxBytes?: number): Promise<TextPreview>;
  readDataUrl(fs: FsId, path: string): Promise<string>;
  dirSize(fs: FsId, path: string): Promise<DirSize>;
  volumes(): Promise<Volume[]>;
  /** Ejects a removable volume by its mount point. */
  eject(mount: string): Promise<void>;
  places(): Promise<Place[]>;
  platform(): Promise<Platform>;
  connections(): Promise<Connection[]>;
  saveConnection(input: ConnectionInput): Promise<Connection>;
  deleteConnection(id: string): Promise<void>;
  connect(id: string): Promise<string>;
  disconnect(id: string): Promise<void>;
  testConnection(input: ConnectionInput): Promise<number>;
  gdriveSignIn(id: string): Promise<void>;
  watch(paths: string[]): Promise<void>;
  search(fs: FsId, root: string, query: string, includeHidden: boolean, onEvent: (e: SearchEvent) => void): Promise<string>;
  cancelSearch(id: string): Promise<void>;
  /** Measures folders in the background; events stream per folder. Stop with `cancelSearch`. */
  folderSizes(fs: FsId, paths: string[], force: boolean, onEvent: (e: SizeEvent) => void): Promise<string>;
  open(fs: FsId, path: string): Promise<string | null>;
  /** Streams a browser File (OS drop without a path, i.e. Windows) into `dir`; returns the new path. */
  uploadFile(fs: FsId, dir: string, file: File, onProgress?: (done: number) => void): Promise<string>;
  reveal(path: string): Promise<void>;
  syncList(): Promise<SyncPair[]>;
  syncPreview(input: SyncInput): Promise<SyncPreview>;
  syncSave(input: SyncInput): Promise<SyncPair>;
  syncDelete(id: string): Promise<void>;
  syncRun(id: string, allowDeletes: boolean): Promise<void>;
  syncPause(id: string, paused: boolean): Promise<void>;
  onSyncs(cb: (syncs: SyncPair[]) => void): Promise<Unlisten>;
  /** URL the <img>/<video> tags can load for a local file. */
  fileUrl(path: string): string;
  thumbUrl(path: string, size: number): string;

  onTransfers(cb: (jobs: Job[]) => void): Promise<Unlisten>;
  onTransferDone(cb: (job: Job) => void): Promise<Unlisten>;
  onMetrics(cb: (m: MetricsSnapshot) => void): Promise<Unlisten>;
  onFsChanged(cb: (dirs: string[]) => void): Promise<Unlisten>;
  onDropFiles(cb: (paths: string[], position: { x: number; y: number }) => void): Promise<Unlisten>;
  /** Native window chrome actions. */
  window: {
    show(): Promise<void>;
    minimize(): Promise<void>;
    toggleMaximize(): Promise<void>;
    close(): Promise<void>;
    startDragging(): Promise<void>;
  };
}

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function tauriBackend(): Promise<Backend> {
  const { invoke, convertFileSrc, Channel } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  const win = getCurrentWindow();

  return {
    listDir: (fs, path) => invoke("list_dir", { fs, path: path ?? null }),
    stat: (fs, path) => invoke("stat", { fs, path }),
    createDir: (fs, parent, name) => invoke("create_dir", { fs, parent, name }),
    createFile: (fs, parent, name) => invoke("create_file", { fs, parent, name }),
    rename: (fs, path, newName) => invoke("rename", { fs, path, newName }),
    trash: (paths) => invoke("trash", { paths }),
    checkConflicts: (sources, dest) => invoke("check_conflicts", { sources, dest }),
    enqueue: (req) => invoke("transfer_enqueue", { req }),
    transfers: () => invoke("transfers_list"),
    pauseTransfer: (id, paused) => invoke("transfer_pause", { id, paused }),
    cancelTransfer: (id) => invoke("transfer_cancel", { id }),
    retryTransfer: (id) => invoke("transfer_retry", { id }),
    removeTransfer: (id) => invoke("transfer_remove", { id }),
    clearTransfers: () => invoke("transfers_clear"),
    readText: (fs, path, maxBytes) => invoke("read_text", { fs, path, maxBytes: maxBytes ?? null }),
    readDataUrl: (fs, path) => invoke("read_data_url", { fs, path }),
    dirSize: (fs, path) => invoke("dir_size", { fs, path }),
    volumes: () => invoke("get_volumes"),
    eject: (mount) => invoke("eject_volume", { mount }),
    places: () => invoke("get_places"),
    platform: () => invoke("platform"),
    connections: () => invoke("connections_list"),
    saveConnection: (input) => invoke("connection_save", { input }),
    deleteConnection: (id) => invoke("connection_delete", { id }),
    connect: (id) => invoke("connection_connect", { id }),
    disconnect: (id) => invoke("connection_disconnect", { id }),
    testConnection: (input) => invoke("connection_test", { input }),
    gdriveSignIn: (id) => invoke("gdrive_sign_in", { id }),
    watch: (paths) => invoke("watch_set", { paths }),
    search: (fs, root, query, includeHidden, onEvent) => {
      const ch = new Channel<SearchEvent>();
      ch.onmessage = onEvent;
      return invoke("search_start", { fs, root, query, includeHidden, onEvent: ch });
    },
    cancelSearch: (id) => invoke("search_cancel", { id }),
    folderSizes: (fs, paths, force, onEvent) => {
      const ch = new Channel<SizeEvent>();
      ch.onmessage = onEvent;
      return invoke("folder_sizes", { fs, paths, force, onEvent: ch });
    },
    open: (fs, path) => invoke("open_item", { fs, path }),
    async uploadFile(fs, dir, file, onProgress) {
      const id = await invoke<string>("upload_begin", { fs, dir, name: file.name });
      const CHUNK = 4 * 1024 * 1024;
      try {
        for (let off = 0; off < file.size; off += CHUNK) {
          const bytes = new Uint8Array(await file.slice(off, off + CHUNK).arrayBuffer());
          await invoke("upload_chunk", bytes, { headers: { "x-upload-id": id } });
          onProgress?.(Math.min(file.size, off + CHUNK));
        }
      } catch (e) {
        await invoke("upload_end", { id, cancel: true }).catch(() => {});
        throw e;
      }
      return invoke<string>("upload_end", { id, cancel: false });
    },
    reveal: (path) => invoke("reveal_item", { path }),
    syncList: () => invoke("sync_list"),
    syncPreview: (input) => invoke("sync_preview", { input }),
    syncSave: (input) => invoke("sync_save", { input }),
    syncDelete: (id) => invoke("sync_delete", { id }),
    syncRun: (id, allowDeletes) => invoke("sync_run", { id, allowDeletes }),
    syncPause: (id, paused) => invoke("sync_pause", { id, paused }),
    onSyncs: (cb) => listen<SyncPair[]>("syncs", (e) => cb(e.payload)),
    fileUrl: (path) => convertFileSrc(path),
    thumbUrl: (path, size) => `${convertFileSrc(path, "thumb")}?s=${size}`,
    onTransfers: (cb) => listen<Job[]>("transfers", (e) => cb(e.payload)),
    onTransferDone: (cb) => listen<Job>("transfer-done", (e) => cb(e.payload)),
    onMetrics: (cb) => listen<MetricsSnapshot>("metrics", (e) => cb(e.payload)),
    onFsChanged: (cb) => listen<string[]>("fs-changed", (e) => cb(e.payload)),
    onDropFiles: (cb) =>
      getCurrentWebview().onDragDropEvent((e) => {
        if (e.payload.type === "drop") {
          const ratio = window.devicePixelRatio || 1;
          cb(e.payload.paths, { x: e.payload.position.x / ratio, y: e.payload.position.y / ratio });
        }
      }),
    window: {
      show: () => win.show(),
      minimize: () => win.minimize(),
      toggleMaximize: () => win.toggleMaximize(),
      close: () => win.close(),
      startDragging: () => win.startDragging(),
    },
  };
}

let backend: Backend | null = null;

export async function initBackend(): Promise<Backend> {
  if (backend) return backend;
  backend = isTauri ? await tauriBackend() : (await import("./mock")).createMockBackend();
  return backend;
}

/** Synchronous accessor once `initBackend` resolved (the app awaits it before first render). */
export function api(): Backend {
  if (!backend) throw new Error("backend not initialised");
  return backend;
}

/** For tests. */
export function setBackend(b: Backend) {
  backend = b;
}

export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return "Something went wrong";
}

export function errorKind(e: unknown): string | null {
  if (e && typeof e === "object" && "kind" in e) return String((e as { kind: unknown }).kind);
  return null;
}
