//! Tauri command surface. Thin: validation + delegation to the modules.

use crate::connections::{self, ConnectionInput, ConnectionView, Connections, Kind};
use crate::error::{AppError, AppResult};
use crate::search::{self, SearchEvent};
use crate::transfer::{JobRequest, JobSnapshot, Loc, Manager};
use crate::vfs::{self, Caps, Crumb, DynProvider, Entry, local};
use crate::volumes::{self, Place, Volume};
use crate::watcher::Watcher;
use base64::Engine;
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_opener::OpenerExt;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

/// In-flight uploads of files dropped in from the OS through HTML5 (Windows, where the
/// webview doesn't expose dropped files' paths): id -> (target path, provider, writer).
pub type Uploads = tokio::sync::Mutex<HashMap<String, (String, DynProvider, crate::vfs::BoxWriter)>>;

pub struct AppState {
    pub local: DynProvider,
    pub conns: Arc<Connections>,
    pub transfers: Arc<Manager>,
    pub syncs: Arc<crate::sync::Engine>,
    pub watcher: Watcher,
    /// Cancellable background tasks (searches, folder sizes) by id.
    pub searches: Arc<Mutex<HashMap<String, CancellationToken>>>,
    pub sizes: Arc<crate::sizes::SizeCache>,
    pub temp_dir: std::path::PathBuf,
    pub uploads: Uploads,
}

impl AppState {
    pub async fn provider(&self, fs: &str) -> AppResult<DynProvider> {
        if fs == "local" { Ok(self.local.clone()) } else { self.conns.provider(fs).await }
    }
}

type S<'a> = State<'a, AppState>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub fs: String,
    pub path: String,
    pub parent: Option<String>,
    pub crumbs: Vec<Crumb>,
    pub entries: Vec<Entry>,
    pub caps: Caps,
}

fn root_label(state: &AppState, fs: &str) -> String {
    if fs == "local" { String::new() } else { state.conns.get(fs).map(|c| c.name).unwrap_or_default() }
}

#[tauri::command]
pub async fn list_dir(state: S<'_>, fs: String, path: Option<String>) -> AppResult<Listing> {
    let p = state.provider(&fs).await?;
    let path = match path.filter(|p| !p.is_empty()) {
        Some(p) => p,
        // A new tab on this computer opens Home, like Finder / Explorer.
        None if fs == "local" => dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_else(|| p.root()),
        None => p.root(),
    };
    let entries = p.list(&path).await?;
    Ok(Listing { crumbs: p.crumbs(&path, &root_label(&state, &fs)), parent: p.parent(&path), caps: p.caps(), fs, path, entries })
}

#[tauri::command]
pub async fn stat(state: S<'_>, fs: String, path: String) -> AppResult<Entry> {
    state.provider(&fs).await?.stat(&path).await
}

#[tauri::command]
pub async fn create_dir(state: S<'_>, fs: String, parent: String, name: String) -> AppResult<Entry> {
    let p = state.provider(&fs).await?;
    p.check_name(&name)?;
    let target = p.join(&parent, &name);
    if p.exists(&target).await? {
        return Err(AppError::AlreadyExists(name));
    }
    p.mkdir(&target).await?;
    p.stat(&target).await
}

#[tauri::command]
pub async fn create_file(state: S<'_>, fs: String, parent: String, name: String) -> AppResult<Entry> {
    use tokio::io::AsyncWriteExt;
    let p = state.provider(&fs).await?;
    p.check_name(&name)?;
    let target = p.join(&parent, &name);
    if p.exists(&target).await? {
        return Err(AppError::AlreadyExists(name));
    }
    let mut w = p.writer(&target).await?;
    w.shutdown().await?;
    p.stat(&target).await
}

#[tauri::command]
pub async fn rename(state: S<'_>, fs: String, path: String, new_name: String) -> AppResult<Entry> {
    let p = state.provider(&fs).await?;
    p.check_name(&new_name)?;
    let parent = p.parent(&path).ok_or_else(|| AppError::Invalid("cannot rename root".into()))?;
    let target = p.join(&parent, &new_name);
    if target == path {
        return p.stat(&path).await;
    }
    p.rename(&path, &target).await?;
    p.stat(&target).await
}

/// Moves local items to the OS trash / recycle bin.
#[tauri::command]
pub async fn trash(paths: Vec<String>) -> AppResult<()> {
    tokio::task::spawn_blocking(move || trash::delete_all(&paths).map_err(|e| AppError::Other(e.to_string()))).await?
}

/// Names (of the sources) that already exist in `dest`.
#[tauri::command]
pub async fn check_conflicts(state: S<'_>, sources: Vec<Loc>, dest: Loc) -> AppResult<Vec<String>> {
    let Some(first) = sources.first() else { return Ok(vec![]) };
    let src = state.provider(&first.fs).await?;
    let dst = state.provider(&dest.fs).await?;
    let mut out = Vec::new();
    for s in &sources {
        let name = src.file_name(&s.path);
        let target = dst.join(&dest.path, &name);
        // Pasting into the same folder duplicates, which is never a conflict.
        if s.fs == dest.fs && src.same_entry(&target, &s.path).await {
            continue;
        }
        if dst.exists(&target).await? {
            out.push(name);
        }
    }
    Ok(out)
}

#[tauri::command]
pub fn transfer_enqueue(state: S<'_>, req: JobRequest) -> AppResult<String> {
    state.transfers.enqueue(req)
}

#[tauri::command]
pub fn transfers_list(state: S<'_>) -> Vec<JobSnapshot> {
    state.transfers.snapshots()
}

#[tauri::command]
pub fn transfer_pause(state: S<'_>, id: String, paused: bool) -> AppResult<()> {
    state.transfers.pause(&id, paused)
}

#[tauri::command]
pub fn transfer_cancel(state: S<'_>, id: String) -> AppResult<()> {
    state.transfers.cancel(&id)
}

#[tauri::command]
pub fn transfer_retry(state: S<'_>, id: String) -> AppResult<String> {
    state.transfers.retry(&id)
}

#[tauri::command]
pub fn transfer_remove(state: S<'_>, id: String) {
    state.transfers.remove(&id)
}

#[tauri::command]
pub fn transfers_clear(state: S<'_>) {
    state.transfers.clear_finished()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPreview {
    pub text: String,
    pub truncated: bool,
    pub binary: bool,
    pub size: u64,
}

async fn read_head(p: &DynProvider, path: &str, max: u64) -> AppResult<(Vec<u8>, u64)> {
    let size = p.stat(path).await?.size;
    let mut r = p.reader(path).await?;
    let mut buf = Vec::with_capacity(max.min(size) as usize);
    (&mut r).take(max).read_to_end(&mut buf).await?;
    Ok((buf, size))
}

#[tauri::command]
pub async fn read_text(state: S<'_>, fs: String, path: String, max_bytes: Option<u64>) -> AppResult<TextPreview> {
    let p = state.provider(&fs).await?;
    let max = max_bytes.unwrap_or(256 * 1024).min(4 * 1024 * 1024);
    let (buf, size) = read_head(&p, &path, max).await?;
    let sample = &buf[..buf.len().min(8192)];
    let binary =
        sample.contains(&0) || std::str::from_utf8(sample).is_err() && String::from_utf8_lossy(sample).matches('\u{FFFD}').count() > 8;
    Ok(TextPreview {
        text: if binary { String::new() } else { String::from_utf8_lossy(&buf).into_owned() },
        truncated: size > buf.len() as u64,
        binary,
        size,
    })
}

/// For remote previews (images, PDFs…): returns a data: URL.
#[tauri::command]
pub async fn read_data_url(state: S<'_>, fs: String, path: String) -> AppResult<String> {
    const MAX: u64 = 25 * 1024 * 1024;
    let p = state.provider(&fs).await?;
    let (buf, size) = read_head(&p, &path, MAX).await?;
    if size > MAX {
        return Err(AppError::Unsupported("file too large to preview".into()));
    }
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    Ok(format!("data:{};base64,{}", mime, base64::engine::general_purpose::STANDARD.encode(buf)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirSize {
    pub bytes: u64,
    pub files: u64,
    pub dirs: u64,
}

#[tauri::command]
pub async fn dir_size(state: S<'_>, fs: String, path: String) -> AppResult<DirSize> {
    let p = state.provider(&fs).await?;
    if !p.is_remote() {
        let (bytes, files, dirs) = tokio::task::spawn_blocking(move || local::tree_size(std::path::Path::new(&path))).await?;
        return Ok(DirSize { bytes, files, dirs });
    }
    let (mut bytes, mut files, mut dirs) = (0, 0, 0);
    let mut stack = vec![path];
    while let Some(d) = stack.pop() {
        for e in p.list(&d).await? {
            // Don't follow folder links: a `self -> .` link would loop forever.
            if e.is_dir && !e.is_symlink {
                dirs += 1;
                stack.push(e.path);
            } else {
                files += 1;
                bytes += e.size;
            }
        }
    }
    Ok(DirSize { bytes, files, dirs })
}

#[tauri::command]
pub fn get_volumes() -> Vec<Volume> {
    volumes::list()
}

#[tauri::command]
pub fn get_places() -> Vec<Place> {
    volumes::places()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Platform {
    pub os: &'static str,
    pub sep: char,
    pub home: Option<String>,
}

#[tauri::command]
pub fn platform() -> Platform {
    Platform { os: std::env::consts::OS, sep: std::path::MAIN_SEPARATOR, home: dirs::home_dir().map(|h| h.to_string_lossy().into_owned()) }
}

// ----------------------------- connections -----------------------------

#[tauri::command]
pub fn connections_list(state: S<'_>) -> Vec<ConnectionView> {
    state.conns.views()
}

#[tauri::command]
pub fn connection_save(state: S<'_>, input: ConnectionInput) -> AppResult<ConnectionView> {
    let c = state.conns.upsert(input)?;
    state.conns.views().into_iter().find(|v| v.conn.id == c.id).ok_or_else(|| AppError::Other("save failed".into()))
}

#[tauri::command]
pub fn connection_delete(state: S<'_>, id: String) -> AppResult<()> {
    state.conns.remove(&id)
}

/// Connects (if needed) and returns the root path to open.
#[tauri::command]
pub async fn connection_connect(state: S<'_>, id: String) -> AppResult<String> {
    Ok(state.conns.provider(&id).await?.root())
}

#[tauri::command]
pub fn connection_disconnect(state: S<'_>, id: String) {
    state.conns.disconnect(&id)
}

/// Tries a configuration without saving it (blank secrets fall back to stored ones).
#[tauri::command]
pub async fn connection_test(state: S<'_>, input: ConnectionInput) -> AppResult<usize> {
    let (options, mut secrets) = connections::split_secrets(&input.kind, input.options.clone());
    if let Some(id) = &input.id {
        let mut stored = state.conns.secrets_for(id);
        stored.extend(secrets);
        secrets = stored;
    }
    let conn = connections::Connection {
        id: input.id.clone().unwrap_or_default(),
        name: input.name.clone(),
        kind: input.kind.clone(),
        options,
        color: None,
        created_at: 0,
        insecure_secrets: false,
    };
    let probe = async {
        let built = connections::build(&conn, &secrets).await?;
        built.provider.list(&built.provider.root()).await.map(|e| e.len())
    };
    tokio::time::timeout(connections::CONNECT_TIMEOUT, probe).await.map_err(|_| {
        AppError::Connection(format!("no answer after {}s — check the host and port", connections::CONNECT_TIMEOUT.as_secs()))
    })?
}

#[tauri::command]
pub async fn gdrive_sign_in<R: Runtime>(app: AppHandle<R>, state: S<'_>, id: String) -> AppResult<()> {
    let conn = state.conns.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
    if conn.kind != Kind::Gdrive {
        return Err(AppError::Invalid("not a Google Drive connection".into()));
    }
    let client_id = conn.options.get("clientId").cloned().unwrap_or_default();
    let client_secret = state.conns.secrets_for(&id).get("clientSecret").cloned().unwrap_or_default();
    if client_id.is_empty() || client_secret.is_empty() {
        return Err(AppError::Invalid("Client ID and Client Secret are required".into()));
    }
    let token =
        crate::oauth::google_sign_in(&client_id, &client_secret, |url| app.opener().open_url(url, None::<&str>).map_err(AppError::other))
            .await?;
    state.conns.set_secret(&id, "refreshToken", token)?;
    state.conns.disconnect(&id);
    Ok(())
}

// ----------------------------- misc -----------------------------

#[tauri::command]
pub fn watch_set(state: S<'_>, paths: Vec<String>) {
    state.watcher.set(paths)
}

#[tauri::command]
pub async fn search_start(
    state: S<'_>,
    fs: String,
    root: String,
    query: String,
    include_hidden: bool,
    on_event: Channel<SearchEvent>,
) -> AppResult<String> {
    let p = state.provider(&fs).await?;
    let id = uuid::Uuid::new_v4().to_string();
    let token = CancellationToken::new();
    state.searches.lock().insert(id.clone(), token.clone());
    let searches = state.searches.clone();
    let sid = id.clone();
    tauri::async_runtime::spawn(async move {
        search::run(p, root, query, include_hidden, token, move |e| {
            let _ = on_event.send(e);
        })
        .await;
        searches.lock().remove(&sid);
    });
    Ok(id)
}

#[tauri::command]
pub fn search_cancel(state: S<'_>, id: String) {
    if let Some(t) = state.searches.lock().remove(&id) {
        t.cancel();
    }
}

/// Opens with the default app. Remote files are downloaded (through the queue, so
/// progress is visible) to a temp folder first; returns the job id in that case.
#[tauri::command]
pub async fn open_item<R: Runtime>(app: AppHandle<R>, state: S<'_>, fs: String, path: String) -> AppResult<Option<String>> {
    if fs == "local" {
        app.opener().open_path(&path, None::<&str>).map_err(AppError::other)?;
        return Ok(None);
    }
    let dir = state.temp_dir.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir)?;
    let id = state.transfers.enqueue(JobRequest {
        kind: crate::transfer::JobKind::Copy,
        sources: vec![Loc { fs, path }],
        dest: Some(Loc { fs: "local".into(), path: dir.to_string_lossy().into_owned() }),
        conflict: crate::transfer::Conflict::Overwrite,
        open_after: true,
    })?;
    Ok(Some(id))
}

#[tauri::command]
pub fn reveal_item<R: Runtime>(app: AppHandle<R>, path: String) -> AppResult<()> {
    app.opener().reveal_item_in_dir(&path).map_err(AppError::other)
}

/// Frontend errors land in the app's stderr/log so they're diagnosable in release builds.
#[tauri::command]
pub fn frontend_log(level: String, message: String) {
    eprintln!("[webview:{level}] {message}");
}

#[derive(Serialize)]
pub struct E2e {
    dir: String,
    /// Optional real folder to time a size measurement on (`FIVEFILES_E2E_BENCH`).
    bench: Option<String>,
}

/// Settings for the built-in end-to-end smoke run (`FIVEFILES_E2E=<dir>`), if enabled.
#[tauri::command]
pub fn e2e_dir() -> Option<E2e> {
    let dir = std::env::var("FIVEFILES_E2E").ok().filter(|d| !d.is_empty())?;
    Some(E2e { dir, bench: std::env::var("FIVEFILES_E2E_BENCH").ok().filter(|d| !d.is_empty()) })
}

/// Starts an upload into `dir`; the name is validated and made unique. Returns an id.
#[tauri::command]
pub async fn upload_begin(state: S<'_>, fs: String, dir: String, name: String) -> AppResult<String> {
    let p = state.provider(&fs).await?;
    p.check_name(&name)?;
    let target = vfs::unique_name(&*p, &dir, &name, false).await?;
    let writer = p.writer(&target).await?;
    let id = uuid::Uuid::new_v4().to_string();
    state.uploads.lock().await.insert(id.clone(), (target, p, writer));
    Ok(id)
}

/// Appends a raw chunk (request body) to the upload named by the `x-upload-id` header.
#[tauri::command]
pub async fn upload_chunk(state: S<'_>, request: tauri::ipc::Request<'_>) -> AppResult<()> {
    use tokio::io::AsyncWriteExt;
    let id =
        request.headers().get("x-upload-id").and_then(|v| v.to_str().ok()).ok_or_else(|| AppError::Invalid("missing upload id".into()))?;
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(AppError::Invalid("expected a binary body".into()));
    };
    let mut uploads = state.uploads.lock().await;
    let (_, _, w) = uploads.get_mut(id).ok_or_else(|| AppError::NotFound(format!("upload {id}")))?;
    w.write_all(bytes).await?;
    Ok(())
}

/// Finishes (or with `cancel`, aborts and removes) an upload. Returns the created path.
#[tauri::command]
pub async fn upload_end(state: S<'_>, id: String, cancel: bool) -> AppResult<String> {
    use tokio::io::AsyncWriteExt;
    let (path, p, mut w) = state.uploads.lock().await.remove(&id).ok_or_else(|| AppError::NotFound(format!("upload {id}")))?;
    if cancel {
        drop(w);
        let _ = p.remove(&path).await;
        return Err(AppError::Cancelled);
    }
    if let Err(e) = w.shutdown().await {
        drop(w);
        let _ = p.remove(&path).await;
        return Err(e.into());
    }
    Ok(path)
}

/// Measures folders in the background, streaming progress through `on_event`.
/// Returns an id that `search_cancel` stops (both are background tasks).
#[tauri::command]
pub async fn folder_sizes(
    state: S<'_>,
    fs: String,
    paths: Vec<String>,
    force: bool,
    on_event: Channel<crate::sizes::SizeEvent>,
) -> AppResult<String> {
    let p = state.provider(&fs).await?;
    if force {
        for path in &paths {
            state.sizes.invalidate(&fs, path);
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let token = CancellationToken::new();
    state.searches.lock().insert(id.clone(), token.clone());
    let (tasks, cache, tid) = (state.searches.clone(), state.sizes.clone(), id.clone());
    tauri::async_runtime::spawn(async move {
        crate::sizes::run(p, fs, paths, cache, token, move |e| {
            let _ = on_event.send(e);
        })
        .await;
        tasks.lock().remove(&tid);
    });
    Ok(id)
}

// ----------------------------- folder sync -----------------------------

#[tauri::command]
pub fn sync_list(state: S<'_>) -> Vec<crate::sync::SyncView> {
    state.syncs.views()
}

/// Dry run: what a sync with these settings would do right now.
#[tauri::command]
pub async fn sync_preview(state: S<'_>, input: crate::sync::SyncInput) -> AppResult<crate::sync::Preview> {
    state.syncs.preview(&input).await
}

#[tauri::command]
pub async fn sync_save(state: S<'_>, input: crate::sync::SyncInput) -> AppResult<crate::sync::SyncPair> {
    state.syncs.save(input).await
}

#[tauri::command]
pub fn sync_delete(state: S<'_>, id: String) -> AppResult<()> {
    state.syncs.delete(&id)
}

/// Runs now; `allow_deletes` confirms a run that was held because it deletes a lot.
#[tauri::command]
pub fn sync_run(state: S<'_>, id: String, allow_deletes: bool) {
    state.syncs.run_now(&id, allow_deletes)
}

#[tauri::command]
pub fn sync_pause(state: S<'_>, id: String, paused: bool) -> AppResult<()> {
    state.syncs.set_paused(&id, paused)
}

/// Ejects a removable volume. Refuses while a transfer or sync is using it.
#[tauri::command]
pub async fn eject_volume(state: S<'_>, mount: String) -> AppResult<()> {
    let vol = volumes::list().into_iter().find(|v| v.mount == mount).ok_or_else(|| AppError::NotFound(format!("no drive at {mount}")))?;
    if !vol.ejectable {
        return Err(AppError::Invalid(format!("{} can't be ejected", vol.name)));
    }
    let on_it = |p: &str| {
        let (p, m) = (p.to_lowercase(), mount.trim_end_matches(['/', '\\']).to_lowercase());
        p == m || p.starts_with(&format!("{m}/")) || p.starts_with(&format!("{m}\\"))
    };
    let busy_transfer = state.transfers.snapshots().iter().any(|j| {
        !j.status.is_final()
            && (j.sources.iter().any(|s| s.fs == "local" && on_it(&s.path))
                || j.dest.as_ref().is_some_and(|d| d.fs == "local" && on_it(&d.path)))
    });
    if busy_transfer {
        return Err(AppError::Invalid(format!("A transfer is using {}. Wait for it to finish or cancel it first.", vol.name)));
    }
    if state.syncs.busy_on("local", &on_it) {
        return Err(AppError::Invalid(format!("A sync is running on {}. Pause it first.", vol.name)));
    }
    state.watcher.release_under(&mount);
    volumes::eject(&vol.mount, &vol.device).await.map_err(|e| AppError::Other(format!("Couldn't eject {}: {e}", vol.name)))
}

/// Whether closing the window keeps 5files running in the tray.
#[tauri::command]
pub fn set_background_mode(enabled: bool) {
    crate::tray::KEEP_RUNNING.store(enabled, std::sync::atomic::Ordering::Relaxed);
}
