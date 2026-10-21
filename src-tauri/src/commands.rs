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
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

pub struct AppState {
    pub local: DynProvider,
    pub conns: Arc<Connections>,
    pub transfers: Arc<Manager>,
    pub watcher: Watcher,
    pub searches: Mutex<HashMap<String, CancellationToken>>,
    pub temp_dir: std::path::PathBuf,
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
    let path = path.filter(|p| !p.is_empty()).unwrap_or_else(|| p.root());
    let entries = p.list(&path).await?;
    Ok(Listing {
        crumbs: p.crumbs(&path, &root_label(&state, &fs)),
        parent: p.parent(&path),
        caps: p.caps(),
        fs,
        path,
        entries,
    })
}

#[tauri::command]
pub async fn stat(state: S<'_>, fs: String, path: String) -> AppResult<Entry> {
    state.provider(&fs).await?.stat(&path).await
}

#[tauri::command]
pub async fn create_dir(state: S<'_>, fs: String, parent: String, name: String) -> AppResult<Entry> {
    vfs::validate_name(&name)?;
    let p = state.provider(&fs).await?;
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
    vfs::validate_name(&name)?;
    let p = state.provider(&fs).await?;
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
    vfs::validate_name(&new_name)?;
    let p = state.provider(&fs).await?;
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
        if s.fs == dest.fs && target == s.path {
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
    let binary = sample.contains(&0) || std::str::from_utf8(sample).is_err() && String::from_utf8_lossy(sample).matches('\u{FFFD}').count() > 8;
    Ok(TextPreview { text: if binary { String::new() } else { String::from_utf8_lossy(&buf).into_owned() }, truncated: size > buf.len() as u64, binary, size })
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
            if e.is_dir {
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
    Platform {
        os: std::env::consts::OS,
        sep: std::path::MAIN_SEPARATOR,
        home: dirs::home_dir().map(|h| h.to_string_lossy().into_owned()),
    }
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
    let built = tokio::time::timeout(std::time::Duration::from_secs(25), connections::build(&conn, &secrets))
        .await
        .map_err(|_| AppError::Connection("timed out".into()))??;
    let entries = built.provider.list(&built.provider.root()).await?;
    Ok(entries.len())
}

#[tauri::command]
pub async fn gdrive_sign_in(app: AppHandle, state: S<'_>, id: String) -> AppResult<()> {
    let conn = state.conns.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
    if conn.kind != Kind::Gdrive {
        return Err(AppError::Invalid("not a Google Drive connection".into()));
    }
    let client_id = conn.options.get("clientId").cloned().unwrap_or_default();
    let client_secret = state.conns.secrets_for(&id).get("clientSecret").cloned().unwrap_or_default();
    if client_id.is_empty() || client_secret.is_empty() {
        return Err(AppError::Invalid("Client ID and Client Secret are required".into()));
    }
    let token = crate::oauth::google_sign_in(&client_id, &client_secret, |url| {
        app.opener().open_url(url, None::<&str>).map_err(AppError::other)
    })
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
    tokio::spawn(async move {
        search::run(p, root, query, include_hidden, token, move |e| {
            let _ = on_event.send(e);
        })
        .await;
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
pub async fn open_item(app: AppHandle, state: S<'_>, fs: String, path: String) -> AppResult<Option<String>> {
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
pub fn reveal_item(app: AppHandle, path: String) -> AppResult<()> {
    app.opener().reveal_item_in_dir(&path).map_err(AppError::other)
}
