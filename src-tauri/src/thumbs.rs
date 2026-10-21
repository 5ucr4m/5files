//! `thumb://` protocol: decodes local images off the UI thread, downsizes them and
//! caches the result on disk so big folders of photos scroll smoothly.
//!
//! URL shape (built by the frontend with `convertFileSrc(path, "thumb")`):
//! `thumb://localhost/<percent-encoded absolute path>?s=<px>`
//! (`http://thumb.localhost/...` on Windows).

use crate::metrics::{self, Channel};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tauri::http::{Response, StatusCode, header};

const DEFAULT_SIZE: u32 = 256;
const MAX_SIZE: u32 = 1024;
/// Images above this are not thumbnailed (decode cost/memory).
const MAX_SOURCE_BYTES: u64 = 200 * 1024 * 1024;

fn decode_slots() -> &'static tokio::sync::Semaphore {
    static S: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    S.get_or_init(|| tokio::sync::Semaphore::new(std::thread::available_parallelism().map(|n| n.get().clamp(2, 6)).unwrap_or(4)))
}

pub fn parse_request(uri: &str) -> Option<(PathBuf, u32)> {
    // Strip scheme + host; keep path and query.
    let rest = uri.split_once("://").map(|(_, r)| r).unwrap_or(uri);
    let rest = rest.split_once('/').map(|(_, r)| r).unwrap_or("");
    let (raw_path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let path = urlencoding::decode(raw_path).ok()?.into_owned();
    if path.is_empty() {
        return None;
    }
    let size = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("s="))
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(DEFAULT_SIZE)
        .clamp(16, MAX_SIZE);
    Some((PathBuf::from(path), size))
}

fn cache_key(path: &Path, meta: &std::fs::Metadata, size: u32) -> String {
    let mut h = Sha256::new();
    h.update(path.to_string_lossy().as_bytes());
    h.update(meta.len().to_le_bytes());
    if let Ok(m) = meta.modified() {
        if let Ok(d) = m.duration_since(std::time::UNIX_EPOCH) {
            h.update(d.as_nanos().to_le_bytes());
        }
    }
    h.update(size.to_le_bytes());
    hex::encode(&h.finalize()[..16])
}

pub fn render(path: &Path, size: u32, cache_dir: &Path) -> Result<(Vec<u8>, &'static str), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_SOURCE_BYTES {
        return Err("not thumbnailable".into());
    }
    let key = cache_key(path, &meta, size);
    for (ext, mime) in [("jpg", "image/jpeg"), ("png", "image/png")] {
        let cached = cache_dir.join(format!("{key}.{ext}"));
        if let Ok(bytes) = std::fs::read(&cached) {
            metrics::add_read(Channel::Disk, bytes.len() as u64);
            return Ok((bytes, mime));
        }
    }
    metrics::add_read(Channel::Disk, meta.len());
    let img = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())?;
    let thumb = img.thumbnail(size, size);
    let has_alpha = thumb.color().has_alpha();
    let mut out = std::io::Cursor::new(Vec::new());
    let (ext, mime) = if has_alpha {
        thumb.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
        ("png", "image/png")
    } else {
        let rgb = thumb.to_rgb8();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82);
        enc.encode_image(&rgb).map_err(|e| e.to_string())?;
        ("jpg", "image/jpeg")
    };
    let bytes = out.into_inner();
    let _ = std::fs::create_dir_all(cache_dir);
    if std::fs::write(cache_dir.join(format!("{key}.{ext}")), &bytes).is_ok() {
        metrics::add_write(Channel::Disk, bytes.len() as u64);
    }
    Ok((bytes, mime))
}

pub async fn respond(uri: String, cache_dir: PathBuf) -> Response<Vec<u8>> {
    let Some((path, size)) = parse_request(&uri) else {
        return status(StatusCode::BAD_REQUEST);
    };
    let _permit = decode_slots().acquire().await;
    match tokio::task::spawn_blocking(move || render(&path, size, &cache_dir)).await {
        Ok(Ok((bytes, mime))) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "max-age=31536000, immutable")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(bytes)
            .unwrap(),
        _ => status(StatusCode::NOT_FOUND),
    }
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Vec::new())
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_thumb_urls_on_all_platform_shapes() {
        let (p, s) = parse_request("thumb://localhost/%2FUsers%2Fana%2Fa%20b.png?s=128").unwrap();
        assert_eq!(p, PathBuf::from("/Users/ana/a b.png"));
        assert_eq!(s, 128);
        let (p, s) = parse_request("http://thumb.localhost/C%3A%5CPics%5Cx.jpg").unwrap();
        assert_eq!(p, PathBuf::from("C:\\Pics\\x.jpg"));
        assert_eq!(s, DEFAULT_SIZE);
        assert_eq!(parse_request("thumb://localhost/x?s=99999").unwrap().1, MAX_SIZE);
        assert!(parse_request("thumb://localhost/").is_none());
    }

    #[test]
    fn renders_and_caches() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("img.png");
        image::RgbImage::from_fn(800, 400, |x, _| image::Rgb([(x % 255) as u8, 80, 160])).save(&src).unwrap();
        let cache = dir.path().join("cache");
        let (bytes, mime) = render(&src, 200, &cache).unwrap();
        assert_eq!(mime, "image/jpeg");
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (200, 100));
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
        // Second call hits the cache.
        let (bytes2, _) = render(&src, 200, &cache).unwrap();
        assert_eq!(bytes, bytes2);
    }
}
