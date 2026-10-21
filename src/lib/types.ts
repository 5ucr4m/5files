// Mirrors the Rust types (serde camelCase).

export type FsId = "local" | (string & {});

export interface Entry {
  name: string;
  path: string;
  isDir: boolean;
  isSymlink: boolean;
  size: number;
  modified: number | null;
  created: number | null;
  hidden: boolean;
  readonly: boolean;
  ext: string | null;
}

export interface Crumb {
  name: string;
  path: string;
}

export interface Caps {
  rename: boolean;
  trash: boolean;
  thumbnails: boolean;
  serverCopy: boolean;
}

export interface Listing {
  fs: FsId;
  path: string;
  parent: string | null;
  crumbs: Crumb[];
  entries: Entry[];
  caps: Caps;
}

export interface Loc {
  fs: FsId;
  path: string;
}

export type JobKind = "copy" | "move" | "delete";
export type Conflict = "overwrite" | "skip" | "keepBoth";
export type JobStatus = "queued" | "scanning" | "running" | "paused" | "done" | "failed" | "cancelled";

export interface JobRequest {
  kind: JobKind;
  sources: Loc[];
  dest?: Loc | null;
  conflict?: Conflict;
  openAfter?: boolean;
}

export interface Job {
  id: string;
  kind: JobKind;
  status: JobStatus;
  title: string;
  srcFs: FsId;
  dest: Loc | null;
  totalBytes: number;
  doneBytes: number;
  totalFiles: number;
  doneFiles: number;
  skipped: number;
  current: string | null;
  speed: number;
  etaSecs: number | null;
  error: string | null;
  createdAt: number;
  finishedAt: number | null;
  openAfter: boolean;
  outputs: string[];
}

export interface Rates {
  diskRead: number;
  diskWrite: number;
  netRx: number;
  netTx: number;
}

export interface MetricsSnapshot {
  rates: Rates;
  totals: Rates;
  peak: Rates;
}

export interface Volume {
  name: string;
  mount: string;
  total: number;
  free: number;
  kind: "ssd" | "hdd" | "unknown";
  removable: boolean;
  fsType: string;
  isRoot: boolean;
  /** Can be ejected / safely removed (external disks, SD cards, disk images). */
  ejectable: boolean;
}

export interface Place {
  id: "home" | "desktop" | "documents" | "downloads" | "pictures" | "music" | "videos";
  name: string;
  path: string;
}

export type ConnKind = "s3" | "gdrive" | "nextcloud" | "webdav" | "sftp" | "ftp";
export type ConnStatus = "disconnected" | "connecting" | "connected" | "error";

export interface Connection {
  id: string;
  name: string;
  kind: ConnKind;
  options: Record<string, string>;
  color: string | null;
  createdAt: number;
  insecureSecrets: boolean;
  status: ConnStatus;
  error: string | null;
  storedSecrets: string[];
}

export interface ConnectionInput {
  id?: string | null;
  name: string;
  kind: ConnKind;
  options: Record<string, string>;
  color?: string | null;
}

export interface TextPreview {
  text: string;
  truncated: boolean;
  binary: boolean;
  size: number;
}

export interface DirSize {
  bytes: number;
  files: number;
  dirs: number;
}

export interface Platform {
  os: "macos" | "windows" | "linux" | (string & {});
  sep: "/" | "\\";
  home: string | null;
}

export type SearchEvent =
  | { type: "results"; entries: Entry[] }
  | { type: "progress"; scanned: number }
  | { type: "done"; total: number; truncated: boolean; cancelled: boolean };

export interface AppError {
  kind: string;
  message: string;
}

export type SizeEvent =
  | { type: "progress"; path: string; bytes: number; files: number }
  | { type: "done"; path: string; bytes: number; files: number }
  | { type: "error"; path: string; message: string };

export type SyncMode = "mirror" | "keepDeleted";
export type SyncSide = "source" | "dest";

export interface SyncRun {
  at: number;
  ok: boolean;
  copied: number;
  deleted: number;
  conflicts: number;
  bytes: number;
  failed: number;
  error: string | null;
}

export interface SyncSummary {
  copies: number;
  copyBytes: number;
  toSource: number;
  deletes: number;
  deletedFiles: number;
  conflicts: number;
  mkdirs: number;
}

export type SyncState =
  | { state: "idle" }
  | { state: "scanning" }
  | { state: "syncing"; done: number; total: number; bytes: number; totalBytes: number; current: string | null }
  | { state: "review"; summary: SyncSummary }
  | { state: "error"; message: string }
  | { state: "paused" };

export type SyncPair = {
  id: string;
  name: string;
  source: Loc;
  dest: Loc;
  mode: SyncMode;
  twoWay: boolean;
  paused: boolean;
  createdAt: number;
  lastRun: SyncRun | null;
} & SyncState;

export interface SyncInput {
  id?: string | null;
  name: string;
  source: Loc;
  dest: Loc;
  mode: SyncMode;
  twoWay: boolean;
}

export type SyncOp =
  | { op: "mkDir"; to: SyncSide; rel: string }
  | { op: "copy"; to: SyncSide; rel: string; size: number }
  | { op: "delete"; on: SyncSide; rel: string }
  | { op: "conflict"; rel: string; winner: SyncSide; size: number };

export interface SyncPreview {
  summary: SyncSummary;
  needsReview: boolean;
  sample: SyncOp[];
}

// ---------------------------------------------------------------- sharing on the network

export interface ShareOptions {
  /** Visitors may add files (never delete or replace). */
  allowUpload: boolean;
  showHidden: boolean;
  /** Stop sharing after this many minutes; null = until stopped. */
  expiresInMins: number | null;
}

export interface ShareActivity {
  at: number;
  /** Visitor's address on the network. */
  who: string;
  action: "opened" | "downloaded" | "zip" | "uploaded";
  name: string;
}

export interface ShareView {
  id: string;
  name: string;
  fs: FsId;
  path: string;
  url: string;
  altUrls: string[];
  /** False when this computer has no network: only it can open the link. */
  online: boolean;
  qr: { size: number; bits: string };
  options: ShareOptions;
  expiresAt: number | null;
  created: number;
  visitors: number;
  lastSeen: number | null;
  bytesOut: number;
  bytesIn: number;
  /** Downloads / uploads running right now. */
  active: number;
  activity: ShareActivity[];
}
