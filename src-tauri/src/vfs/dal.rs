//! Provider backed by Apache OpenDAL — covers S3 (and S3-compatibles), Google Drive,
//! Nextcloud / WebDAV and FTP(S) with one implementation.
//!
//! Our paths are absolute POSIX (`/a/b`); OpenDAL wants relative paths and a trailing
//! slash for directories (`a/b/`).

use super::{BoxReader, BoxWriter, Caps, Entry, Provider, posix};
use crate::error::{AppError, AppResult};
use crate::metrics::{self, Channel, CountingReader, CountingWriter};
use async_trait::async_trait;
use futures::TryStreamExt;
use opendal::{EntryMode, Metadata, Operator};
use tokio_util::compat::{FuturesAsyncReadCompatExt, FuturesAsyncWriteCompatExt};

const CHUNK: usize = 8 * 1024 * 1024;
const CONCURRENCY: usize = 4;
/// Rough wire cost of one listed entry, used to account metadata traffic in metrics.
const LIST_ENTRY_WIRE_BYTES: u64 = 256;

pub struct DalFs {
    op: Operator,
    remote: bool,
}

impl DalFs {
    pub fn new(op: Operator, remote: bool) -> Self {
        Self { op, remote }
    }

    fn ch(&self) -> Channel {
        Channel::for_remote(self.remote)
    }
}

fn rel_file(path: &str) -> String {
    posix::normalize(path).trim_start_matches('/').to_string()
}

fn rel_dir(path: &str) -> String {
    let p = rel_file(path);
    if p.is_empty() { "/".into() } else { format!("{p}/") }
}

fn to_entry(path: &str, meta: &Metadata) -> Entry {
    let is_dir = meta.mode() == EntryMode::DIR;
    let mut e = Entry::new(posix::file_name(path), posix::normalize(path), is_dir);
    e.size = if is_dir { 0 } else { meta.content_length() };
    e.modified = meta.last_modified().map(|t| t.timestamp_millis());
    e
}

#[async_trait]
impl Provider for DalFs {
    fn is_remote(&self) -> bool {
        self.remote
    }

    fn caps(&self) -> Caps {
        let c = self.op.info().full_capability();
        Caps { rename: c.rename, trash: false, thumbnails: false, server_copy: c.copy }
    }

    async fn list(&self, path: &str) -> AppResult<Vec<Entry>> {
        let dir = rel_dir(path);
        let base = posix::normalize(path);
        let lister = self.op.lister_with(&dir).await?;
        let entries: Vec<opendal::Entry> = lister.try_collect().await?;
        metrics::add_read(self.ch(), entries.len() as u64 * LIST_ENTRY_WIRE_BYTES);
        let mut out = Vec::with_capacity(entries.len());
        for de in entries {
            // Some services include the directory itself in the listing.
            let name = de.name().trim_end_matches('/');
            if name.is_empty() || de.path() == dir || de.path().trim_end_matches('/') == dir.trim_end_matches('/') {
                continue;
            }
            let full = posix::join(&base, name);
            out.push(to_entry(&full, de.metadata()));
        }
        // Some services don't return size/mtime in listings; fill the gaps with
        // bounded-concurrency stats (skipped for huge folders to keep listing fast).
        let missing: Vec<usize> =
            out.iter().enumerate().filter(|(_, e)| !e.is_dir && e.size == 0 && e.modified.is_none()).map(|(i, _)| i).collect();
        if !missing.is_empty() && missing.len() <= 2000 {
            use futures::StreamExt;
            let stats: Vec<(usize, Option<Metadata>)> = futures::stream::iter(missing)
                .map(|i| {
                    let p = rel_file(&out[i].path);
                    let op = self.op.clone();
                    async move { (i, op.stat(&p).await.ok()) }
                })
                .buffer_unordered(16)
                .collect()
                .await;
            for (i, m) in stats {
                if let Some(m) = m {
                    out[i].size = m.content_length();
                    out[i].modified = m.last_modified().map(|t| t.timestamp_millis());
                }
            }
        }
        Ok(out)
    }

    async fn stat(&self, path: &str) -> AppResult<Entry> {
        let norm = posix::normalize(path);
        if norm == "/" {
            return Ok(Entry::new("/", "/", true));
        }
        metrics::add_read(self.ch(), LIST_ENTRY_WIRE_BYTES);
        match self.op.stat(&rel_file(&norm)).await {
            Ok(m) => Ok(to_entry(&norm, &m)),
            Err(e) if e.kind() == opendal::ErrorKind::NotFound => {
                let m = self.op.stat(&rel_dir(&norm)).await?;
                Ok(to_entry(&norm, &m))
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn mkdir(&self, path: &str) -> AppResult<()> {
        Ok(self.op.create_dir(&rel_dir(path)).await?)
    }

    async fn remove(&self, path: &str) -> AppResult<()> {
        let e = self.stat(path).await?;
        if e.is_dir {
            self.op.remove_all(&rel_dir(path)).await?;
        } else {
            self.op.delete(&rel_file(path)).await?;
        }
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str) -> AppResult<()> {
        if self.exists(to).await? {
            return Err(AppError::AlreadyExists(to.into()));
        }
        let src = self.stat(from).await?;
        if !self.op.info().full_capability().rename || src.is_dir {
            // Object stores have no directory rename; the transfer engine falls back to
            // copy + delete for those.
            return Err(AppError::Unsupported("rename".into()));
        }
        Ok(self.op.rename(&rel_file(from), &rel_file(to)).await?)
    }

    async fn copy_file(&self, from: &str, to: &str) -> AppResult<bool> {
        if !self.op.info().full_capability().copy {
            return Ok(false);
        }
        self.op.copy(&rel_file(from), &rel_file(to)).await?;
        Ok(true)
    }

    async fn reader(&self, path: &str) -> AppResult<BoxReader> {
        let p = rel_file(path);
        let size = self.op.stat(&p).await?.content_length();
        let r = self.op.reader_with(&p).chunk(CHUNK).concurrent(CONCURRENCY).await?;
        let fut = r.into_futures_async_read(0..size).await?;
        Ok(Box::new(CountingReader::new(fut.compat(), self.ch())))
    }

    async fn writer(&self, path: &str) -> AppResult<BoxWriter> {
        let p = rel_file(path);
        let cap = self.op.info().full_capability();
        let w = if cap.write_can_multi {
            self.op.writer_with(&p).chunk(CHUNK).concurrent(CONCURRENCY).await?
        } else {
            self.op.writer(&p).await?
        };
        Ok(Box::new(CountingWriter::new(w.into_futures_async_write().compat_write(), self.ch())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn mem() -> DalFs {
        let op = Operator::new(opendal::services::Memory::default()).unwrap().finish();
        DalFs::new(op, true)
    }

    #[tokio::test]
    async fn dal_roundtrip_on_memory_backend() {
        let fs = mem();
        fs.mkdir("/docs").await.unwrap();
        let mut w = fs.writer("/docs/a.txt").await.unwrap();
        w.write_all(b"opendal!").await.unwrap();
        w.shutdown().await.unwrap();

        let root = fs.list("/").await.unwrap();
        assert_eq!(root.len(), 1);
        assert!(root[0].is_dir);
        assert_eq!(root[0].path, "/docs");

        let docs = fs.list("/docs").await.unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].name, "a.txt");
        assert_eq!(docs[0].size, 8);

        let st = fs.stat("/docs").await.unwrap();
        assert!(st.is_dir);

        let mut r = fs.reader("/docs/a.txt").await.unwrap();
        let mut s = String::new();
        r.read_to_string(&mut s).await.unwrap();
        assert_eq!(s, "opendal!");

        fs.remove("/docs").await.unwrap();
        assert!(!fs.exists("/docs/a.txt").await.unwrap());
    }
}
