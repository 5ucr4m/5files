# Atlas

A modern, fast file explorer for **macOS, Windows and Linux**, built with Tauri 2 (Rust) and React.

Cloud storage and servers mount as drives next to your disks, every copy goes through a visible
queue with real progress, and a live monitor in the title bar shows how much disk and network
Atlas itself is using.

## Features

**Browsing**
- Tabs (drag to reorder, middle-click to close, session restored on launch), back/forward history per tab
- List and icon views, both virtualized (100k+ entries stay smooth), sortable columns, natural sort
- Editable path bar (click empty space or `⌘L`/`Ctrl+L`, `~` expands), breadcrumbs that accept drops
- Instant filter as you type; `Enter` searches all subfolders, streaming results (globs like `*.pdf` work)
- Quick Look (`Space`), preview pane with images, video, audio, PDF and code/text, folder size on demand
- Image thumbnails generated in Rust, cached on disk and decoded off the UI thread
- Live refresh when files change on disk (watcher), hidden files toggle, favorites, OS trash

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

**Activity monitor** — the "seismograph" in the title bar plots Atlas's own disk reads/writes and
network download/upload in real time (mirrored traces, last 60 s). It counts the bytes flowing through
Atlas's own I/O paths, so it reflects only this app, identically on all three platforms. Click it for
totals and peaks.

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
ATLAS_E2E=/path/to/empty/dir ./target/debug/atlas   # prints "[webview:e2e] PASS"
```
(The smoke run expects `seed.bin` and `pic.png` in that directory.)

CI (`.github/workflows/ci.yml`) runs the test suites on macOS, Windows and Ubuntu, the Docker
integration suite on Ubuntu, and builds installers for all three platforms on `main`.

Environment overrides: `ATLAS_CONFIG_DIR` (config location, e.g. portable installs),
`ATLAS_NO_KEYCHAIN=1` (store secrets in the fallback file).
