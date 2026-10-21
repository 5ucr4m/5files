// In-memory backend used when running in a plain browser (vite dev) and in tests.
// Simulates a home folder, two remote drives, the transfer queue and I/O metrics.
import type { Backend, Unlisten } from "./api";
import type { Connection, Entry, FsId, Job, JobRequest, MetricsSnapshot, Rates, ShareView, SyncPair, Volume } from "./types";

interface Node {
  name: string;
  dir: boolean;
  size: number;
  modified: number;
  children?: Map<string, Node>;
  content?: string;
}

const DAY = 86_400_000;
const now = Date.now();
let seed = 7;
const rnd = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);

function dir(name: string, kids: Node[] = [], age = 3): Node {
  return { name, dir: true, size: 0, modified: now - age * DAY, children: new Map(kids.map((k) => [k.name, k])) };
}
function file(name: string, size: number, age = rnd() * 60, content?: string): Node {
  return { name, dir: false, size, modified: now - age * DAY, content };
}

const README = `# 5files\n\nA modern, fast file explorer.\n\n- Tabs\n- Transfer queue\n- Remote drives\n`;
const CODE = `import { useState } from "react";\n\nexport function Counter() {\n  const [n, setN] = useState(0);\n  return <button onClick={() => setN(n + 1)}>{n}</button>;\n}\n`;

function homeTree(): Node {
  const photos = Array.from({ length: 24 }, (_, i) => file(`IMG_${2400 + i}.jpg`, 2_400_000 + Math.round(rnd() * 3_000_000), i * 2));
  return dir("/", [
    dir("Users", [
      dir("demo", [
        dir("Desktop", [file("Screenshot 2026-10-12.png", 1_820_331, 9), file("todo.md", 812, 1, "- ship 5files\n- sleep")]),
        dir("Documents", [
          dir("Invoices", [file("2026-08.pdf", 120_220, 50), file("2026-09.pdf", 118_002, 20)]),
          file("Proposal — Q4.docx", 482_110, 4),
          file("Budget 2026.xlsx", 92_330, 12),
          file("notes.txt", 2_201, 0.2, "Remember to back up the NAS on Friday."),
          file("README.md", README.length, 2, README),
        ]),
        dir("Downloads", [
          file("ubuntu-26.04-desktop-amd64.iso", 6_100_000_000, 6),
          file("5files-0.1.0.dmg", 9_800_000, 1),
          file("dataset.zip", 712_000_000, 14),
          file("podcast-ep-42.mp3", 61_000_000, 3),
          file("talk.mp4", 1_400_000_000, 8),
        ]),
        dir("Pictures", [dir("Trips", photos.slice(0, 10), 30), ...photos.slice(10), file("logo.svg", 3_402, 40)]),
        dir("Music", [file("Night Drive.flac", 41_200_000, 90), file("Morning.m4a", 8_100_000, 80)]),
        dir("Projects", [
          dir("5files", [
            dir("src", [file("App.tsx", CODE.length, 0.1, CODE), file("main.tsx", 420, 0.3, CODE)]),
            dir("src-tauri", [file("Cargo.toml", 1_532, 0.5, "[package]\nname = \"5files\"\n")]),
            file("package.json", 812, 0.4, '{\n  "name": "5files"\n}\n'),
            file("README.md", README.length, 0.5, README),
            file(".gitignore", 120, 10, "node_modules\ndist\n"),
          ], 0.1),
        ]),
        file(".zshrc", 3_120, 100, "export PATH=$HOME/bin:$PATH"),
      ], 0),
    ]),
  ]);
}

function bucketTree(): Node {
  return dir("/", [
    dir("backups", [file("db-2026-10-20.sql.gz", 340_000_000, 1), file("db-2026-10-19.sql.gz", 338_000_000, 2)]),
    dir("media", [file("hero.webp", 220_000, 5), file("intro.mp4", 88_000_000, 12)]),
    file("index.html", 4_210, 3, "<!doctype html><h1>Hello</h1>"),
  ]);
}

function serverTree(): Node {
  return dir("/", [
    dir("home", [dir("deploy", [dir("app", [file("docker-compose.yml", 1_220, 2, "services:\n  web:\n    image: nginx\n")]), file(".bashrc", 3_771, 200)])]),
    dir("var", [dir("log", [file("syslog", 12_220_110, 0.01, "Oct 21 08:00:01 host CRON[1]: tick\n")])]),
  ]);
}

const trees = new Map<FsId, Node>([
  ["local", homeTree()],
  ["c-s3", bucketTree()],
  ["c-sftp", serverTree()],
]);

const HOME = "/Users/demo";

function split(path: string) {
  return path.split("/").filter(Boolean);
}
function lookup(fs: FsId, path: string): Node {
  let n = trees.get(fs);
  if (!n) throw { kind: "notFound", message: `unknown drive ${fs}` };
  for (const seg of split(path)) {
    const next: Node | undefined = n?.children?.get(seg);
    if (!next) throw { kind: "notFound", message: `not found: ${path}` };
    n = next;
  }
  return n!;
}
function join(parent: string, name: string) {
  return parent === "/" ? `/${name}` : `${parent.replace(/\/$/, "")}/${name}`;
}
function parentOf(path: string): string | null {
  const s = split(path);
  if (!s.length) return null;
  return "/" + s.slice(0, -1).join("/");
}
function toEntry(n: Node, path: string): Entry {
  const dot = n.name.lastIndexOf(".");
  return {
    name: n.name,
    path,
    isDir: n.dir,
    isSymlink: false,
    size: n.dir ? 0 : n.size,
    modified: n.modified,
    created: n.modified - DAY,
    hidden: n.name.startsWith("."),
    readonly: false,
    ext: !n.dir && dot > 0 ? n.name.slice(dot + 1).toLowerCase() : null,
  };
}
function treeBytes(n: Node): number {
  if (!n.dir) return n.size;
  let t = 0;
  n.children!.forEach((c) => (t += treeBytes(c)));
  return t;
}
function clone(n: Node): Node {
  return { ...n, modified: Date.now(), children: n.children ? new Map([...n.children].map(([k, v]) => [k, clone(v)])) : undefined };
}
function uniqueName(parent: Node, name: string, isDir: boolean) {
  if (!parent.children!.has(name)) return name;
  const dot = name.lastIndexOf(".");
  for (let i = 2; ; i++) {
    const cand = !isDir && dot > 0 ? `${name.slice(0, dot)} (${i})${name.slice(dot)}` : `${name} (${i})`;
    if (!parent.children!.has(cand)) return cand;
  }
}

type Cb<T> = (v: T) => void;
function emitter<T>() {
  const subs = new Set<Cb<T>>();
  return {
    on(cb: Cb<T>): Promise<Unlisten> {
      subs.add(cb);
      return Promise.resolve(() => subs.delete(cb));
    },
    emit(v: T) {
      subs.forEach((s) => s(v));
    },
  };
}

export function createMockBackend(): Backend {
  const volumesList: Volume[] = [
    { name: "Macintosh HD", mount: "/", total: 494e9, free: 182e9, kind: "ssd", removable: false, fsType: "apfs", isRoot: true, ejectable: false },
    { name: "Backup", mount: "/Volumes/Backup", total: 2e12, free: 1.42e12, kind: "hdd", removable: true, fsType: "exfat", isRoot: false, ejectable: true },
  ];
  const syncs: SyncPair[] = [
    {
      id: "s1",
      name: "Photos backup",
      source: { fs: "local", path: "/Users/demo/Pictures" },
      dest: { fs: "c-s3", path: "/media" },
      mode: "keepDeleted",
      twoWay: false,
      paused: false,
      createdAt: Date.now() - 5 * DAY,
      lastRun: { at: Date.now() - 4 * 60_000, ok: true, copied: 3, deleted: 0, conflicts: 0, bytes: 12_400_000, failed: 0, error: null },
      state: "idle",
    },
  ];
  const syncEv = emitter<SyncPair[]>();
  const pushSyncs = () => syncEv.emit(syncs.map((x) => ({ ...x })));
  const fakeRun = (id: string) => {
    const x = syncs.find((y) => y.id === id);
    if (!x || x.paused) return;
    let done = 0;
    const total = 24;
    const iv = setInterval(() => {
      done += 2;
      Object.assign(x, { state: "syncing", done, total, bytes: done * 2_000_000, totalBytes: total * 2_000_000, current: `IMG_${2400 + done}.jpg` });
      if (done >= total) {
        clearInterval(iv);
        Object.assign(x, { state: "idle", lastRun: { at: Date.now(), ok: true, copied: total, deleted: 0, conflicts: 0, bytes: total * 2_000_000, failed: 0, error: null } });
      }
      pushSyncs();
    }, 250);
  };
  // Folder sharing: a believable link, a QR-shaped pattern and a visitor who shows up.
  const shares: ShareView[] = [];
  const sharesEv = emitter<ShareView[]>();
  const pushShares = () => sharesEv.emit(shares.map((x) => ({ ...x, activity: [...x.activity] })));
  const fakeQr = (seed: string) => {
    const n = 33;
    let h = 0;
    for (const c of seed) h = (h * 31 + c.charCodeAt(0)) >>> 0;
    const finder = (r: number, c: number) => {
      for (const [y, x] of [[0, 0], [0, n - 7], [n - 7, 0]]) {
        const dy = r - y, dx = c - x;
        if (dy >= 0 && dy < 7 && dx >= 0 && dx < 7) return dy === 0 || dy === 6 || dx === 0 || dx === 6 || (dy >= 2 && dy <= 4 && dx >= 2 && dx <= 4) ? 1 : 0;
        if (dy >= -1 && dy <= 7 && dx >= -1 && dx <= 7) return 0;
      }
      return -1;
    };
    let bits = "";
    for (let r = 0; r < n; r++)
      for (let c = 0; c < n; c++) {
        const f = finder(r, c);
        if (f >= 0) bits += f;
        else if (r === 6 || c === 6) bits += (r + c) % 2 ? 0 : 1;
        else {
          h = (h * 1103515245 + 12345) >>> 0;
          bits += (h >>> 16) & 1;
        }
      }
    return { size: n, bits };
  };
  const jobs: Job[] = [];
  const paused = new Set<string>();
  const cancelled = new Set<string>();
  const transfersEv = emitter<Job[]>();
  const doneEv = emitter<Job>();
  const metricsEv = emitter<MetricsSnapshot>();
  const fsEv = emitter<string[]>();
  const totals: Rates = { diskRead: 0, diskWrite: 0, netRx: 0, netTx: 0 };
  let tickBytes: Rates = { diskRead: 0, diskWrite: 0, netRx: 0, netTx: 0 };
  const peak: Rates = { diskRead: 1, diskWrite: 1, netRx: 1, netTx: 1 };
  const conns: Connection[] = [
    {
      id: "c-s3",
      name: "Media bucket",
      kind: "s3",
      options: { bucket: "5files-media", region: "eu-west-1", accessKeyId: "AKIA…" },
      color: "#f59e0b",
      createdAt: now - 20 * DAY,
      insecureSecrets: false,
      status: "disconnected",
      error: null,
      storedSecrets: ["secretAccessKey"],
    },
    {
      id: "c-sftp",
      name: "homelab",
      kind: "sftp",
      options: { host: "192.168.1.20", port: "22", username: "deploy" },
      color: "#10b981",
      createdAt: now - 40 * DAY,
      insecureSecrets: false,
      status: "disconnected",
      error: null,
      storedSecrets: ["password"],
    },
  ];
  const remote = (fs: FsId) => fs !== "local";
  const delay = <T,>(v: T, ms = 60 + rnd() * 80) => new Promise<T>((r) => setTimeout(() => r(v), ms));

  // Metrics ticker (500 ms) — mirrors the Rust sampler.
  setInterval(() => {
    const rates: Rates = {
      diskRead: tickBytes.diskRead * 2,
      diskWrite: tickBytes.diskWrite * 2,
      netRx: tickBytes.netRx * 2,
      netTx: tickBytes.netTx * 2,
    };
    (Object.keys(rates) as (keyof Rates)[]).forEach((k) => {
      totals[k] += tickBytes[k];
      peak[k] = Math.max(peak[k] * 0.995, rates[k]);
    });
    tickBytes = { diskRead: 0, diskWrite: 0, netRx: 0, netTx: 0 };
    metricsEv.emit({ rates, totals: { ...totals }, peak: { ...peak } });
  }, 500);

  const pushJobs = () => transfersEv.emit(jobs.map((j) => ({ ...j })));

  function runJob(job: Job, req: JobRequest) {
    const srcFs = req.sources[0].fs;
    const nodes = req.sources.map((s) => ({ s, n: lookup(s.fs, s.path) }));
    job.totalBytes = nodes.reduce((t, x) => t + treeBytes(x.n), 0);
    job.totalFiles = nodes.length;
    job.status = "running";
    const speed = remote(srcFs) || (req.dest && remote(req.dest.fs)) ? 38e6 : 420e6;
    const iv = setInterval(() => {
      if (cancelled.has(job.id)) {
        clearInterval(iv);
        job.status = "cancelled";
        job.finishedAt = Date.now();
        job.speed = 0;
        pushJobs();
        return;
      }
      if (paused.has(job.id)) {
        job.status = "paused";
        job.speed = 0;
        pushJobs();
        return;
      }
      job.status = "running";
      const step = Math.min(job.totalBytes - job.doneBytes, speed * 0.1 * (0.85 + rnd() * 0.3));
      job.doneBytes += step;
      const readCh = remote(srcFs) ? "netRx" : "diskRead";
      const writeCh = req.dest && remote(req.dest.fs) ? "netTx" : "diskWrite";
      if (req.kind !== "delete") {
        tickBytes[readCh] += step;
        tickBytes[writeCh] += step;
      }
      job.speed = step * 10;
      job.etaSecs = job.speed > 0 ? (job.totalBytes - job.doneBytes) / job.speed : null;
      job.current = nodes[Math.min(nodes.length - 1, Math.floor((job.doneBytes / Math.max(1, job.totalBytes)) * nodes.length))]?.n.name ?? null;
      job.doneFiles = Math.floor((job.doneBytes / Math.max(1, job.totalBytes)) * job.totalFiles);
      if (job.doneBytes >= job.totalBytes) {
        clearInterval(iv);
        // Apply the effect on the tree.
        for (const { s, n } of nodes) {
          const parent = lookup(s.fs, parentOf(s.path)!);
          if (req.kind === "delete") {
            parent.children!.delete(n.name);
            continue;
          }
          const destNode = lookup(req.dest!.fs, req.dest!.path);
          const name = uniqueName(destNode, n.name, n.dir);
          destNode.children!.set(name, { ...clone(n), name });
          job.outputs.push(join(req.dest!.path, name));
          if (req.kind === "move") parent.children!.delete(n.name);
        }
        job.status = "done";
        job.doneFiles = job.totalFiles;
        job.finishedAt = Date.now();
        job.speed = 0;
        job.etaSecs = null;
        job.current = null;
        doneEv.emit({ ...job });
        const dirs = [...req.sources.map((s) => parentOf(s.path)!), ...(req.dest ? [req.dest.path] : [])];
        fsEv.emit(dirs);
      }
      pushJobs();
    }, 100);
  }

  const b: Backend = {
    async listDir(fs, path) {
      const p = path || (fs === "local" ? HOME : "/");
      const n = lookup(fs, p);
      if (!n.dir) throw { kind: "invalid", message: "not a folder" };
      const s = split(p);
      const crumbs = [{ name: fs === "local" ? "/" : conns.find((c) => c.id === fs)?.name ?? fs, path: "/" }, ...s.map((seg, i) => ({ name: seg, path: "/" + s.slice(0, i + 1).join("/") }))];
      const entries = [...n.children!.values()].map((c) => toEntry(c, join(p, c.name)));
      return delay({ fs, path: p, parent: parentOf(p), crumbs, entries, caps: { rename: true, trash: fs === "local", thumbnails: fs === "local", serverCopy: false } }, remote(fs) ? 220 : 30);
    },
    async stat(fs, path) {
      return toEntry(lookup(fs, path), path);
    },
    async createDir(fs, parent, name) {
      const p = lookup(fs, parent);
      if (p.children!.has(name)) throw { kind: "alreadyExists", message: `"${name}" already exists` };
      p.children!.set(name, dir(name, [], 0));
      fsEv.emit([parent]);
      return toEntry(p.children!.get(name)!, join(parent, name));
    },
    async createFile(fs, parent, name) {
      const p = lookup(fs, parent);
      if (p.children!.has(name)) throw { kind: "alreadyExists", message: `"${name}" already exists` };
      p.children!.set(name, file(name, 0, 0, ""));
      fsEv.emit([parent]);
      return toEntry(p.children!.get(name)!, join(parent, name));
    },
    async rename(fs, path, newName) {
      const parentPath = parentOf(path)!;
      const p = lookup(fs, parentPath);
      const n = lookup(fs, path);
      if (p.children!.has(newName) && newName !== n.name) throw { kind: "alreadyExists", message: `"${newName}" already exists` };
      p.children!.delete(n.name);
      n.name = newName;
      p.children!.set(newName, n);
      fsEv.emit([parentPath]);
      return toEntry(n, join(parentPath, newName));
    },
    async trash(paths) {
      for (const path of paths) lookup("local", parentOf(path)!).children!.delete(split(path).pop()!);
      fsEv.emit([...new Set(paths.map((p) => parentOf(p)!))]);
    },
    async checkConflicts(sources, dest) {
      const d = lookup(dest.fs, dest.path);
      return sources.map((s) => split(s.path).pop()!).filter((n, i) => d.children!.has(n) && !(sources[i].fs === dest.fs && parentOf(sources[i].path) === dest.path));
    },
    async enqueue(req) {
      const id = Math.random().toString(36).slice(2);
      const names = req.sources.map((s) => split(s.path).pop()!);
      const job: Job = {
        id,
        kind: req.kind,
        status: "queued",
        title: names.length === 1 ? names[0] : `${names.length} items`,
        srcFs: req.sources[0].fs,
        dest: req.dest ?? null,
        totalBytes: 0,
        doneBytes: 0,
        totalFiles: 0,
        doneFiles: 0,
        skipped: 0,
        current: null,
        speed: 0,
        etaSecs: null,
        error: null,
        createdAt: Date.now(),
        finishedAt: null,
        openAfter: !!req.openAfter,
        outputs: [],
      };
      jobs.push(job);
      (job as Job & { req?: JobRequest }).req = req;
      pushJobs();
      setTimeout(() => runJob(job, req), 150);
      return id;
    },
    async transfers() {
      return jobs.map((j) => ({ ...j }));
    },
    async pauseTransfer(id, p) {
      p ? paused.add(id) : paused.delete(id);
    },
    async cancelTransfer(id) {
      cancelled.add(id);
    },
    async retryTransfer(id) {
      const j = jobs.find((x) => x.id === id) as (Job & { req?: JobRequest }) | undefined;
      await b.removeTransfer(id);
      return b.enqueue(j!.req!);
    },
    async removeTransfer(id) {
      const i = jobs.findIndex((j) => j.id === id);
      if (i >= 0) jobs.splice(i, 1);
      pushJobs();
    },
    async clearTransfers() {
      for (let i = jobs.length - 1; i >= 0; i--) if (["done", "failed", "cancelled"].includes(jobs[i].status)) jobs.splice(i, 1);
      pushJobs();
    },
    async readText(fs, path) {
      const n = lookup(fs, path);
      tickBytes[remote(fs) ? "netRx" : "diskRead"] += Math.min(n.size, 256 * 1024);
      const text = n.content ?? "";
      return delay({ text, truncated: false, binary: n.content === undefined, size: n.size });
    },
    async readDataUrl(_fs, path) {
      return svgPlaceholder(path, 640);
    },
    async dirSize(fs, path) {
      const n = lookup(fs, path);
      let files = 0,
        dirs = 0;
      const walk = (x: Node) => x.children!.forEach((c) => (c.dir ? (dirs++, walk(c)) : files++));
      walk(n);
      return delay({ bytes: treeBytes(n), files, dirs }, 300);
    },
    async volumes() {
      return volumesList.map((v) => ({ ...v }));
    },
    async setBackgroundMode() {},
    onTrayOpen: () => Promise.resolve(() => {}),
    async eject(mount) {
      await delay(null, 900);
      const i = volumesList.findIndex((v) => v.mount === mount);
      if (i >= 0) volumesList.splice(i, 1);
    },
    async places() {
      return [
        { id: "home", name: "Home", path: HOME },
        { id: "desktop", name: "Desktop", path: `${HOME}/Desktop` },
        { id: "documents", name: "Documents", path: `${HOME}/Documents` },
        { id: "downloads", name: "Downloads", path: `${HOME}/Downloads` },
        { id: "pictures", name: "Pictures", path: `${HOME}/Pictures` },
        { id: "music", name: "Music", path: `${HOME}/Music` },
      ];
    },
    async platform() {
      const ua = navigator.userAgent;
      return { os: ua.includes("Win") ? "windows" : ua.includes("Mac") ? "macos" : "linux", sep: "/", home: HOME };
    },
    async connections() {
      return conns.map((c) => ({ ...c }));
    },
    async saveConnection(input) {
      let c = conns.find((x) => x.id === input.id);
      const secretKeys = ["password", "secretAccessKey", "clientSecret", "passphrase", "privateKey", "refreshToken"];
      const options = Object.fromEntries(Object.entries(input.options).filter(([k]) => !secretKeys.includes(k)));
      const secrets = Object.keys(input.options).filter((k) => secretKeys.includes(k) && input.options[k]);
      if (c) Object.assign(c, { name: input.name, options, color: input.color ?? c.color, storedSecrets: [...new Set([...c.storedSecrets, ...secrets])] });
      else {
        c = { id: `c-${Math.random().toString(36).slice(2, 8)}`, name: input.name, kind: input.kind, options, color: input.color ?? null, createdAt: Date.now(), insecureSecrets: false, status: "disconnected", error: null, storedSecrets: secrets };
        conns.push(c);
        trees.set(c.id, dir("/", [dir("welcome", [file("hello.txt", 12, 0, "hello there")])]));
      }
      return { ...c };
    },
    async deleteConnection(id) {
      const i = conns.findIndex((c) => c.id === id);
      if (i >= 0) conns.splice(i, 1);
    },
    async connect(id) {
      const c = conns.find((x) => x.id === id)!;
      c.status = "connecting";
      await delay(null, 500);
      c.status = "connected";
      return "/";
    },
    async disconnect(id) {
      const c = conns.find((x) => x.id === id);
      if (c) c.status = "disconnected";
    },
    async testConnection(input) {
      await delay(null, 700);
      if (!input.options.host && !input.options.bucket && !input.options.url && input.kind !== "gdrive") throw { kind: "invalid", message: "missing required field: host" };
      return 3;
    },
    async gdriveSignIn() {
      await delay(null, 1200);
    },
    async watch() {},
    async search(fs, root, query, includeHidden, onEvent) {
      const q = query.toLowerCase();
      const out: Entry[] = [];
      const walk = (n: Node, p: string) =>
        n.children?.forEach((c) => {
          const cp = join(p, c.name);
          if (c.name.startsWith(".") && !includeHidden) return;
          if (c.name.toLowerCase().includes(q)) out.push(toEntry(c, cp));
          if (c.dir) walk(c, cp);
        });
      walk(lookup(fs, root), root);
      setTimeout(() => {
        onEvent({ type: "results", entries: out });
        onEvent({ type: "done", total: out.length, truncated: false, cancelled: false });
      }, 120);
      return "s1";
    },
    async cancelSearch() {},
    async folderSizes(fs, paths, _force, onEvent) {
      paths.forEach((path, i) => {
        let n: Node;
        try {
          n = lookup(fs, path);
        } catch {
          return onEvent({ type: "error", path, message: "not found" });
        }
        let files = 0;
        const walk = (x: Node) => x.children?.forEach((c) => (c.dir ? walk(c) : files++));
        walk(n);
        const bytes = treeBytes(n);
        // Big folders "count up" for a moment, like the real thing.
        setTimeout(() => onEvent({ type: "progress", path, bytes: Math.round(bytes * 0.4), files }), 120 + i * 60);
        setTimeout(() => onEvent({ type: "done", path, bytes, files }), 500 + i * 120);
      });
      return "sizes";
    },
    async open() {
      return null;
    },
    async uploadFile(fs, dir, file) {
      const d = lookup(fs, dir);
      const name = uniqueName(d, file.name, false);
      d.children!.set(name, { name, dir: false, size: file.size, modified: Date.now() });
      fsEv.emit([dir]);
      return join(dir, name);
    },
    async reveal() {},
    async syncList() {
      return syncs.map((x) => ({ ...x }));
    },
    async syncPreview(input) {
      await delay(null, 500);
      const n = treeBytes(lookup(input.source.fs, input.source.path));
      return {
        summary: { copies: 18, copyBytes: n, toSource: input.twoWay ? 2 : 0, deletes: input.mode === "mirror" ? 3 : 0, deletedFiles: input.mode === "mirror" ? 3 : 0, conflicts: 0, mkdirs: 2 },
        needsReview: false,
        sample: [
          { op: "copy", to: "dest", rel: "Trips/IMG_2401.jpg", size: 3_200_000 },
          { op: "copy", to: "dest", rel: "IMG_2412.jpg", size: 4_900_000 },
          ...(input.mode === "mirror" ? [{ op: "delete" as const, on: "dest" as const, rel: "old-export.zip" }] : []),
        ],
      };
    },
    async syncSave(input) {
      let x = syncs.find((y) => y.id === input.id);
      if (x) Object.assign(x, { ...input });
      else {
        x = { ...input, id: `s${Date.now()}`, paused: false, createdAt: Date.now(), lastRun: null, state: "scanning" } as SyncPair;
        syncs.push(x);
      }
      pushSyncs();
      setTimeout(() => fakeRun(x!.id), 400);
      return { ...x };
    },
    async syncDelete(id) {
      const i = syncs.findIndex((y) => y.id === id);
      if (i >= 0) syncs.splice(i, 1);
      pushSyncs();
    },
    async syncRun(id) {
      fakeRun(id);
    },
    async syncPause(id, paused) {
      const x = syncs.find((y) => y.id === id);
      if (x) Object.assign(x, { paused, state: paused ? "paused" : "idle" });
      pushSyncs();
    },
    onSyncs: (cb) => syncEv.on(cb),
    async shareList() {
      return shares.map((x) => ({ ...x }));
    },
    async shareStart(fs, path, options) {
      const existing = shares.find((x) => x.fs === fs && x.path === path);
      if (existing) return { ...existing };
      const id = ["k7m2p9xq4tav", "h3ws8cne6fzu", "b9rj4kdy2mpq"][shares.length % 3];
      const url = `http://192.168.1.20:5155/s/${id}/`;
      const v: ShareView = {
        id,
        name: path.split("/").filter(Boolean).pop() || "Home",
        fs,
        path,
        url,
        altUrls: [`http://demo-mbp.local:5155/s/${id}/`],
        online: true,
        qr: fakeQr(url),
        options,
        expiresAt: options.expiresInMins ? Date.now() + options.expiresInMins * 60_000 : null,
        created: Date.now(),
        visitors: 0,
        lastSeen: null,
        bytesOut: 0,
        bytesIn: 0,
        active: 0,
        activity: [],
      };
      shares.push(v);
      pushShares();
      // Someone opens it a moment later and grabs a few files.
      const visit = [
        { after: 1200, a: { who: "192.168.1.34", action: "opened", name: "" }, out: 0 },
        { after: 2600, a: { who: "192.168.1.34", action: "downloaded", name: "IMG_2041.jpg" }, out: 4_200_000 },
        { after: 3600, a: { who: "192.168.1.51", action: "opened", name: "" }, out: 0 },
        { after: 5200, a: { who: "192.168.1.51", action: "zip", name: v.name }, out: 48_000_000 },
      ] as const;
      for (const step of visit)
        setTimeout(() => {
          const x = shares.find((s) => s.id === id);
          if (!x) return;
          x.activity = [{ at: Date.now(), ...step.a }, ...x.activity];
          x.visitors = new Set(x.activity.map((a) => a.who)).size;
          x.lastSeen = Date.now();
          x.bytesOut += step.out;
          x.active = step.a.action === "zip" ? 1 : 0;
          pushShares();
        }, step.after);
      return { ...v };
    },
    async shareUpdate(id, options) {
      const x = shares.find((s) => s.id === id);
      if (!x) throw { kind: "notFound", message: "share" };
      if (x.options.expiresInMins !== options.expiresInMins) x.expiresAt = options.expiresInMins ? Date.now() + options.expiresInMins * 60_000 : null;
      x.options = options;
      pushShares();
      return { ...x };
    },
    async shareStop(id) {
      const i = shares.findIndex((s) => s.id === id);
      if (i >= 0) shares.splice(i, 1);
      pushShares();
    },
    onShares: (cb) => sharesEv.on(cb),
    onMouseNav: () => Promise.resolve(() => {}),
    async openUrl(url) {
      window.open(url, "_blank", "noopener");
    },
    fileUrl: (path) => svgPlaceholder(path, 1200),
    thumbUrl: (path, size) => svgPlaceholder(path, size),
    onTransfers: (cb) => transfersEv.on(cb),
    onTransferDone: (cb) => doneEv.on(cb),
    onMetrics: (cb) => metricsEv.on(cb),
    onFsChanged: (cb) => fsEv.on(cb),
    onDropFiles: () => Promise.resolve(() => {}),
    window: { show: async () => {}, minimize: async () => {}, toggleMaximize: async () => {}, close: async () => {}, startDragging: async () => {} },
  };
  return b;
}

/** Deterministic, pleasant gradient "photo" so the grid view looks real in the browser preview. */
function svgPlaceholder(path: string, size: number) {
  let h = 0;
  for (const c of path) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  const a = h % 360;
  const b = (a + 40 + (h % 60)) % 360;
  const svg = `<svg xmlns='http://www.w3.org/2000/svg' width='${size}' height='${Math.round(size * 0.75)}' viewBox='0 0 400 300'><defs><linearGradient id='g' x1='0' y1='0' x2='1' y2='1'><stop offset='0' stop-color='hsl(${a} 70% 62%)'/><stop offset='1' stop-color='hsl(${b} 65% 38%)'/></linearGradient></defs><rect width='400' height='300' fill='url(#g)'/><circle cx='${80 + (h % 240)}' cy='${60 + (h % 90)}' r='34' fill='hsl(${a} 90% 88%)' opacity='.8'/><path d='M0 300 L${120 + (h % 80)} ${150 + (h % 60)} L220 230 L${300 - (h % 50)} 140 L400 300 Z' fill='hsl(${b} 40% 22%)' opacity='.75'/></svg>`;
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}
