//! The share web server: one HTML page plus a small JSON/streaming API, all under
//! `/s/<token>/`. Paths inside a share are always relative to its folder; every
//! segment is validated, so a visitor can never reach anything outside it (symlinks
//! leading out of a local folder are refused too).

use super::{Share, Shares, zip};
use crate::error::{AppError, AppResult};
use crate::metrics::{self, Channel};
use crate::vfs::{Entry, check_name};
use bytes::Bytes;
use futures::StreamExt;
use http_body_util::{BodyExt, Empty, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::body::{Frame, Incoming};
use hyper::header::{self, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use std::convert::Infallible;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

type Body = UnsyncBoxBody<Bytes, std::io::Error>;

const PAGE: &str = include_str!("page.html");
const FONT_SANS: &[u8] = include_bytes!("assets/geist.woff2");
const FONT_MONO: &[u8] = include_bytes!("assets/geist-mono.woff2");
const ICON: &str = include_str!("../../../assets/favicon.svg");
/// Name suffix of files being received; hidden from listings, renamed when complete.
const PART_SUFFIX: &str = ".5files-part";
const THUMB_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff", "ico"];

pub async fn serve(listener: tokio::net::TcpListener, shares: Arc<Shares>, cancel: CancellationToken) {
    loop {
        let (stream, peer) = tokio::select! {
            r = listener.accept() => match r {
                Ok(x) => x,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            },
            _ = cancel.cancelled() => break,
        };
        let _ = stream.set_nodelay(true);
        let (shares, cancel) = (shares.clone(), cancel.clone());
        tokio::spawn(async move {
            let ip = peer.ip().to_canonical();
            let svc = service_fn(move |req| {
                let shares = shares.clone();
                async move { Ok::<_, Infallible>(handle(shares, ip, req).await) }
            });
            let conn = http1::Builder::new().keep_alive(true).serve_connection(TokioIo::new(stream), svc);
            tokio::select! {
                _ = conn => {}
                _ = cancel.cancelled() => {}
            }
        });
    }
}

// ---------------------------------------------------------------- responses

fn full(b: impl Into<Bytes>) -> Body {
    Full::new(b.into()).map_err(|n: Infallible| match n {}).boxed_unsync()
}

fn empty() -> Body {
    Empty::new().map_err(|n: Infallible| match n {}).boxed_unsync()
}

fn with_common(mut r: Response<Body>) -> Response<Body> {
    let h = r.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert("x-frame-options", HeaderValue::from_static("SAMEORIGIN"));
    r
}

fn status(code: StatusCode, msg: &str) -> Response<Body> {
    with_common(
        Response::builder()
            .status(code)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-store")
            .body(full(msg.to_string()))
            .unwrap(),
    )
}

fn json<T: Serialize>(v: &T) -> Response<Body> {
    with_common(
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CACHE_CONTROL, "no-store")
            .body(full(serde_json::to_vec(v).unwrap_or_default()))
            .unwrap(),
    )
}

fn error_response(e: &AppError) -> Response<Body> {
    match e {
        AppError::NotFound(_) => status(StatusCode::NOT_FOUND, "Not found"),
        AppError::PermissionDenied(_) => status(StatusCode::FORBIDDEN, "Not allowed"),
        AppError::Invalid(m) => status(StatusCode::BAD_REQUEST, m),
        AppError::AlreadyExists(m) => status(StatusCode::CONFLICT, m),
        _ => status(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

fn page(code: StatusCode) -> Response<Body> {
    // Debug builds can serve the page from disk while it's being worked on.
    #[cfg(debug_assertions)]
    let html: Bytes = match std::env::var("FIVEFILES_SHARE_PAGE") {
        Ok(p) => std::fs::read(p).map(Bytes::from).unwrap_or_else(|_| Bytes::from_static(PAGE.as_bytes())),
        Err(_) => Bytes::from_static(PAGE.as_bytes()),
    };
    #[cfg(not(debug_assertions))]
    let html = Bytes::from_static(PAGE.as_bytes());
    with_common(
        Response::builder()
            .status(code)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-store")
            .header(
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; font-src 'self'; \
                 img-src 'self' data: blob:; media-src 'self'; connect-src 'self'; frame-src 'self'; \
                 base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
            )
            .body(full(html))
            .unwrap(),
    )
}

fn asset(bytes: &'static [u8], mime: &'static str) -> Response<Body> {
    with_common(
        Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "public, max-age=604800, immutable")
            .body(full(Bytes::from_static(bytes)))
            .unwrap(),
    )
}

/// `attachment; filename="ascii"; filename*=UTF-8''pct` — works in every browser.
fn disposition(kind: &str, name: &str) -> String {
    let ascii: String = name.chars().map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' }).collect();
    format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{}", urlencoding::encode(name))
}

// ---------------------------------------------------------------- routing

fn query(req: &Request<Incoming>, key: &str) -> Option<String> {
    req.uri().query()?.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == key).then(|| urlencoding::decode(&v.replace('+', " ")).map(|s| s.into_owned()).unwrap_or_default())
    })
}

async fn handle(shares: Arc<Shares>, ip: IpAddr, req: Request<Incoming>) -> Response<Body> {
    let path = req.uri().path().to_string();
    match path.as_str() {
        "/_/geist.woff2" => return asset(FONT_SANS, "font/woff2"),
        "/_/geist-mono.woff2" => return asset(FONT_MONO, "font/woff2"),
        "/favicon.ico" | "/_/icon.svg" => return asset(ICON.as_bytes(), "image/svg+xml"),
        _ => {}
    }
    let Some(rest) = path.strip_prefix("/s/") else {
        return page(StatusCode::NOT_FOUND);
    };
    let (token, sub) = rest.split_once('/').unwrap_or((rest, ""));
    let Some(share) = shares.get(token) else {
        // Same page: it explains that the link no longer works.
        return page(StatusCode::NOT_FOUND);
    };
    if !rest.contains('/') {
        return with_common(
            Response::builder()
                .status(StatusCode::PERMANENT_REDIRECT)
                .header(header::LOCATION, format!("/s/{token}/"))
                .body(empty())
                .unwrap(),
        );
    }
    share.touch(ip);
    let (route, rel) = sub.split_once('/').unwrap_or((sub, ""));
    let method = req.method().clone();
    let res = match (route, &method) {
        ("", &Method::GET) => {
            share.log(ip, "opened", "");
            Ok(page(StatusCode::OK))
        }
        ("api", &Method::GET) if rel == "list" => list(&share, &query(&req, "p").unwrap_or_default()).await,
        ("f", &Method::GET | &Method::HEAD) => file(&share, ip, rel, &req).await,
        ("t", &Method::GET) => thumb(&shares, &share, rel, query(&req, "s").and_then(|s| s.parse().ok()).unwrap_or(320)).await,
        ("zip", &Method::GET) => zip_folder(&share, ip, rel).await,
        ("up", &Method::PUT | &Method::POST) => {
            let name = query(&req, "name").unwrap_or_default();
            upload(&share, ip, rel, name, req).await
        }
        _ => Err(AppError::NotFound(path.clone())),
    };
    res.unwrap_or_else(|e| error_response(&e))
}

// ---------------------------------------------------------------- paths

/// Percent-encoded, `/`-separated path relative to the share -> provider path.
async fn resolve(share: &Share, rel: &str) -> AppResult<(String, Vec<String>)> {
    let p = &share.provider;
    let hidden_ok = share.options().show_hidden;
    let mut path = share.root.clone();
    let mut segs = Vec::new();
    for raw in rel.split('/').filter(|s| !s.is_empty()) {
        let seg = urlencoding::decode(raw).map_err(|_| AppError::Invalid("bad path".into()))?.into_owned();
        check_name(&seg, false).map_err(|_| AppError::NotFound(seg.clone()))?;
        p.check_name(&seg).map_err(|_| AppError::NotFound(seg.clone()))?;
        if (!hidden_ok && seg.starts_with('.')) || seg.ends_with(PART_SUFFIX) {
            return Err(AppError::NotFound(seg));
        }
        path = p.join(&path, &seg);
        segs.push(seg);
    }
    // Symlinks may point anywhere; the share only covers its own folder.
    if !p.is_remote() && !segs.is_empty() && !p.contains(&share.root, &path).await {
        return Err(AppError::NotFound(rel.into()));
    }
    Ok((path, segs))
}

async fn visible(share: &Share, e: &Entry) -> bool {
    if e.name.ends_with(PART_SUFFIX) || check_name(&e.name, false).is_err() {
        return false;
    }
    if e.hidden && !share.options().show_hidden {
        return false;
    }
    if e.is_symlink && !share.provider.is_remote() {
        return share.provider.contains(&share.root, &e.path).await;
    }
    true
}

fn rel_of(segs: &[String]) -> String {
    segs.iter().map(|s| urlencoding::encode(s).into_owned()).collect::<Vec<_>>().join("/")
}

// ---------------------------------------------------------------- handlers

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    name: String,
    dir: bool,
    size: u64,
    modified: Option<i64>,
    ext: Option<String>,
    thumb: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Crumb {
    name: String,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Listing {
    share: String,
    host: String,
    path: String,
    crumbs: Vec<Crumb>,
    items: Vec<Item>,
    upload: bool,
}

async fn list(share: &Arc<Share>, rel: &str) -> AppResult<Response<Body>> {
    let (path, segs) = resolve(share, rel).await?;
    let entries = share.provider.list(&path).await?;
    let local = share.fs == "local";
    let mut items = Vec::with_capacity(entries.len());
    for e in entries {
        if !visible(share, &e).await {
            continue;
        }
        let thumb = local && !e.is_dir && e.ext.as_deref().is_some_and(|x| THUMB_EXTS.contains(&x)) && e.size <= 200 * 1024 * 1024;
        items.push(Item { name: e.name, dir: e.is_dir, size: e.size, modified: e.modified, ext: e.ext, thumb });
    }
    let mut crumbs = vec![Crumb { name: share.name.clone(), path: String::new() }];
    for i in 0..segs.len() {
        crumbs.push(Crumb { name: segs[i].clone(), path: rel_of(&segs[..=i]) });
    }
    Ok(json(&Listing {
        share: share.name.clone(),
        host: super::computer_name().to_string(),
        path: rel_of(&segs),
        crumbs,
        items,
        upload: share.options().allow_upload,
    }))
}

/// `bytes=a-b`, `bytes=a-`, `bytes=-n` (one range). `None`: whole file.
fn parse_range(h: &str, size: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = h.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None; // multi-range: send everything
    }
    let (a, b) = spec.split_once('-')?;
    let r = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            if n == 0 || size == 0 {
                return Some(Err(()));
            }
            (size.saturating_sub(n), size - 1)
        }
        (a, "") => (a.parse().ok()?, size.saturating_sub(1)),
        (a, b) => (a.parse().ok()?, b.parse::<u64>().ok()?.min(size.saturating_sub(1))),
    };
    if r.0 > r.1 || r.0 >= size {
        return Some(Err(()));
    }
    Some(Ok(r))
}

/// What a browser may render in place. Everything else downloads; text-like files
/// (HTML, SVG, scripts…) are shown as plain text so nothing in a share can run code.
fn inline_type(mime: &str) -> Option<String> {
    let text =
        mime.starts_with("text/") || matches!(mime, "application/json" | "application/javascript" | "application/xml" | "image/svg+xml");
    if text {
        Some("text/plain; charset=utf-8".into())
    } else if mime.starts_with("image/") || mime.starts_with("video/") || mime.starts_with("audio/") || mime == "application/pdf" {
        Some(mime.into())
    } else {
        None
    }
}

async fn file(share: &Arc<Share>, ip: IpAddr, rel: &str, req: &Request<Incoming>) -> AppResult<Response<Body>> {
    let (path, segs) = resolve(share, rel).await?;
    if segs.is_empty() {
        return Err(AppError::NotFound(rel.into()));
    }
    let e = share.provider.stat(&path).await?;
    if e.is_dir || !visible(share, &e).await {
        return Err(AppError::NotFound(rel.into()));
    }
    let size = e.size;
    let mime = mime_guess::from_path(&e.name).first_raw().unwrap_or("application/octet-stream");
    let download = query(req, "dl").is_some();
    let inline = if download { None } else { inline_type(mime) };
    let range = req.headers().get(header::RANGE).and_then(|h| h.to_str().ok()).and_then(|h| parse_range(h, size));
    let (start, end, partial) = match range {
        Some(Err(())) => {
            return Ok(with_common(
                Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                    .body(empty())
                    .unwrap(),
            ));
        }
        Some(Ok((a, b))) => (a, b, true),
        None => (0, size.saturating_sub(1), false),
    };
    let len = if size == 0 { 0 } else { end - start + 1 };
    let mut b = Response::builder()
        .status(if partial { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK })
        .header(header::CONTENT_TYPE, inline.as_deref().unwrap_or("application/octet-stream"))
        .header(header::CONTENT_LENGTH, len)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "private, no-cache")
        .header(header::CONTENT_DISPOSITION, disposition(if inline.is_some() { "inline" } else { "attachment" }, &e.name));
    if let Some(m) = e.modified {
        b = b.header(header::LAST_MODIFIED, httpdate(m));
    }
    if partial {
        b = b.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    if inline.as_deref() != Some("application/pdf") {
        // Files are data, never a document with powers on this origin.
        b = b.header(
            header::CONTENT_SECURITY_POLICY,
            "sandbox; default-src 'none'; img-src 'self'; media-src 'self'; style-src 'unsafe-inline'",
        );
    }
    if req.method() == Method::HEAD || len == 0 {
        return Ok(with_common(b.body(empty()).unwrap()));
    }
    // Whole downloads (not video seeking or the text preview) show up in the activity.
    if start == 0 && (download || !partial) {
        share.log(ip, "downloaded", segs.join("/"));
    }
    let reader = open_at(share, &path, start).await?;
    Ok(with_common(b.body(stream_body(share, reader.take(len))).unwrap()))
}

fn httpdate(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.format("%a, %d %b %Y %H:%M:%S GMT").to_string()).unwrap_or_default()
}

/// A reader positioned at `start`: seeks on local disks, skips bytes elsewhere.
async fn open_at(share: &Share, path: &str, start: u64) -> AppResult<crate::vfs::BoxReader> {
    if share.fs == "local" {
        let mut f = tokio::fs::File::open(path).await?;
        if start > 0 {
            f.seek(std::io::SeekFrom::Start(start)).await?;
        }
        return Ok(Box::new(metrics::CountingReader::new(f, Channel::Disk)));
    }
    let mut r = share.provider.reader(path).await?;
    if start > 0 {
        tokio::io::copy(&mut (&mut r).take(start), &mut tokio::io::sink()).await?;
    }
    Ok(r)
}

/// Streams a reader as the response body, charging the bytes to the network monitor
/// and this share, and ending it when the share stops.
fn stream_body<R: tokio::io::AsyncRead + Send + Unpin + 'static>(share: &Arc<Share>, reader: R) -> Body {
    let share2 = share.clone();
    let guard = share.busy();
    let s =
        tokio_util::io::ReaderStream::with_capacity(reader, 256 * 1024).take_until(share.stopped.clone().cancelled_owned()).map(move |r| {
            let _ = &guard;
            r.map(|b| {
                metrics::add_write(Channel::Net, b.len() as u64);
                share2.add_out(b.len() as u64);
                Frame::data(b)
            })
        });
    BodyExt::boxed_unsync(StreamBody::new(s))
}

async fn thumb(shares: &Shares, share: &Arc<Share>, rel: &str, size: u32) -> AppResult<Response<Body>> {
    if share.fs != "local" {
        return Err(AppError::NotFound(rel.into()));
    }
    let (path, segs) = resolve(share, rel).await?;
    if segs.is_empty() {
        return Err(AppError::NotFound(rel.into()));
    }
    let (bytes, mime) =
        crate::thumbs::render_async(path.into(), size.clamp(64, 1024), shares.thumbs_dir.clone()).await.map_err(AppError::NotFound)?;
    metrics::add_write(Channel::Net, bytes.len() as u64);
    share.add_out(bytes.len() as u64);
    Ok(with_common(
        Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "private, max-age=3600")
            .body(full(bytes))
            .unwrap(),
    ))
}

/// Every file and folder under `path` as ZIP entries named `prefix/...`.
async fn walk(share: &Share, path: &str, prefix: &str, out: &mut Vec<zip::Item>) {
    let mut stack = vec![(path.to_string(), prefix.to_string())];
    while let Some((dir, name)) = stack.pop() {
        out.push(zip::Item { name: format!("{name}/"), source: None, size: 0, modified: None });
        let Ok(entries) = share.provider.list(&dir).await else { continue };
        for e in entries {
            if !visible(share, &e).await {
                continue;
            }
            let child = format!("{name}/{}", e.name);
            if e.is_dir {
                // Don't follow folder links: they can loop.
                if !e.is_symlink {
                    stack.push((e.path.clone(), child));
                }
            } else {
                out.push(zip::Item { name: child, source: Some(e.path.clone()), size: e.size, modified: e.modified });
            }
        }
    }
}

async fn zip_folder(share: &Arc<Share>, ip: IpAddr, rel: &str) -> AppResult<Response<Body>> {
    let rel = rel.trim_end_matches(".zip");
    let (path, segs) = resolve(share, rel).await?;
    let st = share.provider.stat(&path).await?;
    if !st.is_dir {
        return Err(AppError::NotFound(rel.into()));
    }
    let folder = segs.last().cloned().unwrap_or_else(|| share.name.clone());
    let mut items = Vec::new();
    walk(share, &path, &folder, &mut items).await;
    let plan = zip::plan(items);
    let total = plan.total;
    share.log(ip, "zip", if segs.is_empty() { share.name.clone() } else { segs.join("/") });
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(4);
    let provider = share.provider.clone();
    tokio::spawn(async move {
        // On error the channel closes early; the response is then shorter than its
        // Content-Length and the browser marks the download failed.
        let _ = zip::write(plan, provider, tx).await;
    });
    let reader = tokio_util::io::StreamReader::new(tokio_stream_of(rx));
    Ok(with_common(
        Response::builder()
            .header(header::CONTENT_TYPE, "application/zip")
            .header(header::CONTENT_LENGTH, total)
            .header(header::CONTENT_DISPOSITION, disposition("attachment", &format!("{folder}.zip")))
            .header(header::CACHE_CONTROL, "no-store")
            .body(stream_body(share, reader))
            .unwrap(),
    ))
}

fn tokio_stream_of(rx: tokio::sync::mpsc::Receiver<Bytes>) -> impl futures::Stream<Item = std::io::Result<Bytes>> + Send + Unpin {
    Box::pin(futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|b| (Ok(b), rx)) }))
}

/// "photo.jpg" -> "photo 2.jpg", "photo 3.jpg"… until the name is free.
async fn free_name(share: &Share, dir: &str, name: &str) -> AppResult<String> {
    let p = &share.provider;
    if !p.exists(&p.join(dir, name)).await? {
        return Ok(name.to_string());
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    for i in 2..10_000 {
        let candidate = format!("{stem} {i}{ext}");
        if !p.exists(&p.join(dir, &candidate)).await? {
            return Ok(candidate);
        }
    }
    Err(AppError::AlreadyExists(name.into()))
}

#[derive(Serialize)]
struct Uploaded {
    name: String,
}

async fn upload(share: &Arc<Share>, ip: IpAddr, rel: &str, name: String, req: Request<Incoming>) -> AppResult<Response<Body>> {
    if !share.options().allow_upload {
        return Err(AppError::PermissionDenied("uploads are off".into()));
    }
    let (dir, segs) = resolve(share, rel).await?;
    if !share.provider.stat(&dir).await?.is_dir {
        return Err(AppError::NotFound(rel.into()));
    }
    let p = share.provider.clone();
    check_name(&name, false).map_err(|_| AppError::Invalid("That file name can't be used here.".into()))?;
    p.check_name(&name).map_err(|_| AppError::Invalid("That file name can't be used here.".into()))?;
    if name.ends_with(PART_SUFFIX) || (name.starts_with('.') && !share.options().show_hidden) {
        return Err(AppError::Invalid("Hidden files can't be added to this folder.".into()));
    }
    let _busy = share.busy();
    // Received under a hidden temporary name, renamed once complete. Drives without a
    // rename (object stores) get the final name straight away.
    let staged = p.caps().rename;
    let early_name = if staged { None } else { Some(free_name(share, &dir, &name).await?) };
    let part = match &early_name {
        Some(n) => p.join(&dir, n),
        None => p.join(&dir, &format!(".{}.{}{PART_SUFFIX}", name, &super::new_token()[..6])),
    };
    let mut w = p.writer(&part).await?;
    let mut body = req.into_body();
    let res: AppResult<()> = async {
        loop {
            let frame = tokio::select! {
                f = body.frame() => f,
                _ = share.stopped.cancelled() => return Err(AppError::Cancelled),
            };
            let Some(frame) = frame else { break };
            let frame = frame.map_err(|e| AppError::Connection(e.to_string()))?;
            if let Ok(data) = frame.into_data() {
                metrics::add_read(Channel::Net, data.len() as u64);
                share.add_in(data.len() as u64);
                w.write_all(&data).await?;
            }
        }
        w.shutdown().await?;
        Ok(())
    }
    .await;
    if let Err(e) = res {
        drop(w);
        let _ = p.remove(&part).await;
        return Err(e);
    }
    let final_name = match early_name {
        Some(n) => n,
        None => {
            // Pick the final name only now, so two uploads of the same name don't collide.
            let n = free_name(share, &dir, &name).await?;
            if let Err(e) = p.rename(&part, &p.join(&dir, &n)).await {
                let _ = p.remove(&part).await;
                return Err(e);
            }
            n
        }
    };
    let shown = segs.iter().cloned().chain([final_name.clone()]).collect::<Vec<_>>().join("/");
    share.log(ip, "uploaded", shown);
    Ok(json(&Uploaded { name: final_name }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some(Ok((0, 99))));
        assert_eq!(parse_range("bytes=900-", 1000), Some(Ok((900, 999))));
        assert_eq!(parse_range("bytes=-100", 1000), Some(Ok((900, 999))));
        assert_eq!(parse_range("bytes=0-5000", 1000), Some(Ok((0, 999))));
        assert_eq!(parse_range("bytes=1000-", 1000), Some(Err(())));
        assert_eq!(parse_range("bytes=0-1,5-9", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
    }

    #[test]
    fn only_harmless_types_render_inline() {
        assert_eq!(inline_type("image/jpeg").as_deref(), Some("image/jpeg"));
        assert_eq!(inline_type("video/mp4").as_deref(), Some("video/mp4"));
        assert_eq!(inline_type("text/html").as_deref(), Some("text/plain; charset=utf-8"));
        assert_eq!(inline_type("image/svg+xml").as_deref(), Some("text/plain; charset=utf-8"));
        assert_eq!(inline_type("application/zip"), None);
    }

    #[test]
    fn content_disposition_is_safe_and_unicode() {
        let d = disposition("attachment", "Fotos \"verão\".zip");
        assert!(d.starts_with("attachment; filename=\"Fotos _ver_o_.zip\""), "{d}");
        assert!(d.contains("filename*=UTF-8''Fotos%20%22ver%C3%A3o%22.zip"), "{d}");
    }
}
