//! Upload writer for HTTP backends whose OpenDAL writer is one-shot (WebDAV/Nextcloud):
//! bytes are spooled to a local temp file, then sent with a single streaming `PUT`
//! carrying a real `Content-Length`. Works with every WebDAV server (including
//! Nextcloud behind nginx/php-fpm, which often rejects chunked transfer encoding),
//! never holds the whole file in memory, and charges network metrics as bytes leave.

use crate::metrics::{self, Channel};
use futures::{FutureExt, StreamExt};
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;

pub struct HttpPut {
    pub client: reqwest::Client,
    /// Endpoint + root, without a trailing slash, e.g. `https://h/remote.php/dav/files/ana`.
    pub base: String,
    pub user: Option<String>,
    pub pass: Option<String>,
}

impl HttpPut {
    /// Absolute POSIX path → URL with each segment percent-encoded.
    pub fn url(&self, path: &str) -> String {
        let enc: Vec<String> = path.split('/').filter(|s| !s.is_empty()).map(|s| urlencoding::encode(s).into_owned()).collect();
        format!("{}/{}", self.base, enc.join("/"))
    }
}

type Upload = Pin<Box<dyn Future<Output = io::Result<()>> + Send>>;

pub struct SpoolWriter {
    file: Option<tokio::fs::File>,
    tmp: PathBuf,
    url: String,
    put: Arc<HttpPut>,
    upload: Option<Upload>,
}

impl SpoolWriter {
    pub async fn new(put: Arc<HttpPut>, path: &str) -> io::Result<Self> {
        let tmp = std::env::temp_dir().join(format!("atlas-upload-{}", uuid::Uuid::new_v4().simple()));
        let file = tokio::fs::File::create(&tmp).await?;
        Ok(Self { file: Some(file), tmp, url: put.url(path), put, upload: None })
    }
}

fn other(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

async fn send(put: Arc<HttpPut>, url: String, tmp: PathBuf, mut file: tokio::fs::File) -> io::Result<()> {
    use tokio::io::AsyncWriteExt;
    file.flush().await?;
    drop(file);
    let len = tokio::fs::metadata(&tmp).await?.len();
    let reader = tokio::fs::File::open(&tmp).await?;
    let stream = tokio_util::io::ReaderStream::with_capacity(reader, 256 * 1024).inspect(|chunk| {
        if let Ok(b) = chunk {
            metrics::add_write(Channel::Net, b.len() as u64);
        }
    });
    let mut req = put.client.put(&url).header(reqwest::header::CONTENT_LENGTH, len).body(reqwest::Body::wrap_stream(stream));
    if let Some(u) = &put.user {
        req = req.basic_auth(u, put.pass.as_deref());
    }
    let resp = req.send().await.map_err(other)?;
    let status = resp.status();
    let _ = tokio::fs::remove_file(&tmp).await;
    if !status.is_success() {
        let kind = match status.as_u16() {
            401 | 403 => io::ErrorKind::PermissionDenied,
            404 | 409 => io::ErrorKind::NotFound,
            _ => io::ErrorKind::Other,
        };
        return Err(io::Error::new(kind, format!("upload failed: HTTP {status}")));
    }
    Ok(())
}

impl AsyncWrite for SpoolWriter {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.file.as_mut() {
            Some(f) => Pin::new(f).poll_write(cx, buf),
            None => Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "writer already closed"))),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.file.as_mut() {
            Some(f) => Pin::new(f).poll_flush(cx),
            None => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.upload.is_none() {
            let Some(file) = self.file.take() else { return Poll::Ready(Ok(())) };
            let fut = send(self.put.clone(), self.url.clone(), self.tmp.clone(), file);
            self.upload = Some(fut.boxed());
        }
        let res = futures::ready!(self.upload.as_mut().unwrap().as_mut().poll(cx));
        self.upload = None;
        Poll::Ready(res)
    }
}

impl Drop for SpoolWriter {
    fn drop(&mut self) {
        // Cancelled or failed before upload: don't leave spool files behind.
        let _ = std::fs::remove_file(&self.tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_encode_each_segment() {
        let p = HttpPut { client: reqwest::Client::new(), base: "https://h/remote.php/dav/files/ana".into(), user: None, pass: None };
        assert_eq!(p.url("/My Docs/a#1.txt"), "https://h/remote.php/dav/files/ana/My%20Docs/a%231.txt");
    }
}
