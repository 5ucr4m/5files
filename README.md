# 5files

A modern, fast file explorer for **macOS, Windows and Linux**, built with Tauri 2 (Rust) and React.

Cloud storage and servers mount as drives next to your disks, every copy goes through a visible
queue with real progress, and a live monitor in the title bar shows how much disk and network
5files itself is using.

## Features

**Browsing**
- Tabs (drag to reorder, middle-click to close, session restored on launch), back/forward history per tab
- List and icon views, both virtualized (100k+ entries stay smooth), sortable columns, natural sort
- Editable path bar (click empty space or `⌘L`/`Ctrl+L`, `~` expands), breadcrumbs that accept drops
- Instant filter as you type; `Enter` searches all subfolders, streaming results (globs like `*.pdf` work)
- Quick Look (`Space`), preview pane with images, video, audio, PDF and code/text, folder size on demand
- Folder sizes in the Size column, measured in the background with a parallel walker (about 4× faster
  than `du` on large trees), streamed while counting, cached and kept fresh. Automatic below a drive's
  top level; right-click → Calculate size anywhere, remote drives included
- Image thumbnails generated in Rust, cached on disk and decoded off the UI thread
- Live refresh when files change on disk (watcher), hidden files toggle, favorites, OS trash

**Split panes (tmux-style)**
- Split any tab side by side or stacked, as many times as fits; drag the gutters to resize
  (double-click evens them out); each pane has its own folder, history, selection and search
- tmux keys after the `Ctrl+B` prefix (a hint bar shows them): `%` split right, `"` split down,
  arrows move between panes, `o` next pane, `z` zoom, `x` close pane, `c` new tab, `n`/`p` next/previous tab
- Direct shortcuts: `⌘\`/`Ctrl+\` split right, `⇧⌘\`/`Ctrl+Shift+\` split down,
  `⌥⌘ arrows`/`Ctrl+Alt+arrows` move, `⇧⌘↩` zoom, `⌘W`/`Ctrl+W` closes the pane (the tab when it's the last one)
- Moving files between panes: `F5` copies and `F6` moves the selection into the other pane's folder
  (also in the context menu as "Copy to …" / "Move to …"), or drag across
- Panes spring open from the edge and collapse on close; a light bar glides to the focused pane

**Folder sync** (sidebar → Sync → `+`)
- Keep two folders in step on any drives (local, S3, Drive, Nextcloud, SFTP, FTP)
- **Mirror**: the destination becomes an exact copy, deletions included.
  **Mirror, keep deleted**: everything is copied, nothing is ever deleted from the destination
- **Sync both ways** (either mode): changes made in the destination flow back to the source; the state
  after each run is recorded, so a deletion is never mistaken for a new file
- Review before starting shows exactly what the first sync will do; local deletions go to the Trash;
  conflicts keep the newest version and save the other as "(conflicted copy …)"
- A run that would delete a large share of the files (e.g. an unplugged source) is held for review
- Local folders are watched (syncs ~2 s after changes); remote ones are re-checked every 5 minutes

**Working with files**
- Copy / cut / paste across tabs **and across drives** (local ⇄ S3 ⇄ SFTP …), duplicate, rename inline (F2)
- Drag & drop between views, tabs, breadcrumbs and the sidebar (`⌥`/`Ctrl` forces copy), drop files in from the OS
  (on Windows, where WebView2 hides dropped paths, dropped files are streamed in; dropping folders from
  Explorer isn't supported yet — use copy & paste)
- Name conflicts ask once: keep both, replace (folders merge) or skip
- Rubber-band selection, Shift/⌘ ranges, type-ahead, full keyboard navigation, context menus, command palette (`⌘K`/`Ctrl+K`)

**Transfer queue**
- Every copy/move/delete is a job with byte-accurate progress, speed, ETA, pause/resume, cancel, retry
- 3 jobs run in parallel; moves on the same drive are instant renames; small files use the OS fast path
  (APFS clones on macOS); cancelled copies never leave half-written files, and "Replace" writes to a
  temporary file first, so the original survives a failed or cancelled copy
- Safety rails: names from remote listings are validated for the destination OS (no `../`, `..\`,
  NTFS streams or reserved device names), case-only clashes never overwrite each other, folders can't
  be copied into themselves or replaced by something they contain

**Remote drives** (sidebar → Remote → `+`)
| Integration | Notes |
| --- | --- |
| S3 | AWS, Cloudflare R2, MinIO, Wasabi, Backblaze B2, DigitalOcean Spaces presets |
| Google Drive | Sign in with Google (OAuth loopback + PKCE); token kept in the OS keychain |
| Nextcloud | Server URL + app password; streaming uploads that work behind nginx/php-fpm |
| WebDAV | Synology, QNAP, Box and any WebDAV server |
| SSH / SFTP | Password or key (file or pasted, with passphrase); trust-on-first-use host keys |
| FTP / FTPS | Explicit TLS optional |

Credentials live in the OS keychain (macOS Keychain, Windows Credential Manager, Secret Service on
Linux), never in the config file and never sent back to the UI. If no keychain is available they fall
back to a `0600` file and the drive is flagged.

**Activity monitor** — the "seismograph" in the title bar plots 5files's own disk reads/writes and
network download/upload in real time (mirrored traces, last 60 s). It counts the bytes flowing through
5files's own I/O paths, so it reflects only this app, identically on all three platforms. Click it for
totals and peaks.

**Menu bar / system tray** — live transfer and sync status (click to open the panel), Sync all now,
Pause syncs, Quit. Closing the window keeps 5files running there so syncs continue (setting; on macOS
the Dock icon brings the window back). The icon gains a dot while something is copying or syncing.

**Look & feel** — light, dark or system theme, six accent colors, comfortable/compact density, icon
size slider, spring animations throughout (respects "reduce motion").

## Integrations roadmap

Next candidates, roughly by demand (several already work through the S3 or WebDAV options):
Dropbox · OneDrive/SharePoint · SMB/Windows shares · Azure Blob · Google Cloud Storage · Box ·
iCloud Drive · pCloud / MEGA / Proton Drive · Git repositories (browse any branch/commit) ·
Docker volumes & containers · Kubernetes PVCs · IPFS / HDFS.

Adding one means implementing the `Provider` trait (`src-tauri/src/vfs/mod.rs`); anything OpenDAL
supports is a few lines in `connections.rs` plus a form schema in `src/lib/integrations.ts`.

## Architecture

```
src-tauri/src
├── vfs/            Provider trait + backends
│   ├── local.rs      local disks (hidden flags per OS, fast copy)
│   ├── dal.rs        OpenDAL: S3, Google Drive, WebDAV/Nextcloud, FTP
│   ├── sftp.rs       pure-Rust SSH/SFTP (russh), parallel reads, TOFU host keys
│   ├── spool.rs      streaming PUT uploads for WebDAV (temp spool + Content-Length)
│   └── ftp_empty.rs  zero-byte files on FTP
├── transfer.rs     job queue: scan → copy/move/delete, progress, pause/cancel/retry
├── connections.rs  connection store, keychain secrets, provider factory
├── metrics.rs      app-scoped I/O counters (CountingReader/Writer) → rates
├── thumbs.rs       thumb:// protocol (decode, resize, disk cache)
├── watcher.rs      folder watching (notify) → refresh events
├── search.rs       streaming recursive search
├── sizes.rs        folder sizes: parallel walk, streaming, cache + invalidation
├── sync/           folder sync: plan.rs (pure rules, heavily tested) + engine (scan, run, watch)
├── oauth.rs        Google OAuth (loopback + PKCE)
└── commands.rs     the IPC surface

src/
├── lib/            api (Tauri + in-memory mock), ops, selection, dnd, formatting
├── store/          zustand stores: tabs, settings, live data, UI
└── components/     title bar, sidebar, file views, transfers, connections, …
```

Paths are `(fs, path)` pairs: `fs` is `"local"` or a connection id. Remote paths are POSIX; local
paths are native (`C:\Users\…` on Windows).

## Development

Requirements: Node 22 + pnpm, Rust stable, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/)
for your OS (WebView2 on Windows, `libwebkit2gtk-4.1-dev` on Linux).

```sh
pnpm install
pnpm tauri dev          # run the app
pnpm dev                # UI only, in a browser, against an in-memory mock backend
pnpm tauri build        # installers: .dmg/.app, .msi/.exe, .deb/.AppImage/.rpm
```

### Tests

```sh
pnpm typecheck && pnpm test                    # frontend unit tests
cd src-tauri && cargo test                     # Rust unit tests + IPC end-to-end tests

# Real servers in Docker (SFTP, FTP, S3, WebDAV):
sh tests/docker/up.sh
cargo test --test remote_providers -- --ignored --test-threads=1
sh tests/docker/down.sh

# In-app smoke run inside the real webview (native events, watcher, queue, thumbnails):
FIVEFILES_E2E=/path/to/empty/dir ./target/debug/5files   # prints "[webview:e2e] PASS"
```
(The smoke run expects `seed.bin` and `pic.png` in that directory.)

CI (`.github/workflows/ci.yml`) runs the test suites on macOS, Windows and Ubuntu, the Docker
integration suite on Ubuntu, and builds installers for all three platforms on `main`.

Environment overrides: `FIVEFILES_CONFIG_DIR` (config location, e.g. portable installs),
`FIVEFILES_NO_KEYCHAIN=1` (store secrets in the fallback file).
