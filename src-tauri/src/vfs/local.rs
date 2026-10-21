use super::{BoxReader, BoxWriter, Caps, Crumb, Entry, Provider};
use crate::error::{AppError, AppResult};
use crate::metrics::{self, Channel, CountingReader, CountingWriter};
use async_trait::async_trait;
use std::fs::Metadata;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

pub struct LocalFs;

fn to_ms(t: std::io::Result<std::time::SystemTime>) -> Option<i64> {
    t.ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

fn is_hidden(name: &str, _meta: &Metadata) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if _meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0 {
            return true;
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        const UF_HIDDEN: u32 = 0x8000;
        if _meta.st_flags() & UF_HIDDEN != 0 {
            return true;
        }
    }
    false
}

pub fn entry_from_path(path: &Path) -> std::io::Result<Entry> {
    let lmeta = std::fs::symlink_metadata(path)?;
    let is_symlink = lmeta.file_type().is_symlink();
    // Follow symlinks for type/size, but keep the link itself if it is dangling.
    let meta = if is_symlink { std::fs::metadata(path).unwrap_or(lmeta) } else { lmeta };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let mut e = Entry::new(name.clone(), path.to_string_lossy().into_owned(), meta.is_dir());
    e.is_symlink = is_symlink;
    e.size = if meta.is_dir() { 0 } else { meta.len() };
    e.modified = to_ms(meta.modified());
    e.created = to_ms(meta.created());
    e.hidden = is_hidden(&name, &meta);
    e.readonly = meta.permissions().readonly();
    Ok(e)
}

pub fn list_sync(path: &Path) -> std::io::Result<Vec<Entry>> {
    let rd = std::fs::read_dir(path)?;
    let mut out = Vec::with_capacity(64);
    for de in rd.flatten() {
        // Entries can vanish or be unreadable mid-listing; skip them rather than failing.
        if let Ok(e) = entry_from_path(&de.path()) {
            out.push(e);
        }
    }
    Ok(out)
}

/// Files below this size are copied with the OS primitive (clonefile on APFS,
/// CopyFileEx on Windows, copy_file_range on Linux) which is faster than streaming;
/// progress for them is reported per file.
pub const FAST_COPY_MAX: u64 = 16 * 1024 * 1024;

#[async_trait]
impl Provider for LocalFs {
    fn is_remote(&self) -> bool {
        false
    }

    fn caps(&self) -> Caps {
        Caps { rename: true, trash: true, thumbnails: true, server_copy: true }
    }

    fn root(&self) -> String {
        #[cfg(windows)]
        {
            std::env::var("SystemDrive").map(|d| format!("{d}\\")).unwrap_or_else(|_| "C:\\".into())
        }
        #[cfg(not(windows))]
        {
            "/".into()
        }
    }

    async fn list(&self, path: &str) -> AppResult<Vec<Entry>> {
        let p = PathBuf::from(path);
        Ok(tokio::task::spawn_blocking(move || list_sync(&p)).await??)
    }

    async fn stat(&self, path: &str) -> AppResult<Entry> {
        let p = PathBuf::from(path);
        Ok(tokio::task::spawn_blocking(move || entry_from_path(&p)).await??)
    }

    async fn exists(&self, path: &str) -> AppResult<bool> {
        Ok(tokio::fs::symlink_metadata(path).await.is_ok())
    }

    async fn mkdir(&self, path: &str) -> AppResult<()> {
        Ok(tokio::fs::create_dir_all(path).await?)
    }

    async fn remove(&self, path: &str) -> AppResult<()> {
        let meta = tokio::fs::symlink_metadata(path).await?;
        if meta.is_dir() {
            tokio::fs::remove_dir_all(path).await?;
        } else {
            tokio::fs::remove_file(path).await?;
        }
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str) -> AppResult<()> {
        if tokio::fs::symlink_metadata(to).await.is_ok() && !same_file_case_insensitive(from, to) {
            return Err(AppError::AlreadyExists(to.into()));
        }
        Ok(tokio::fs::rename(from, to).await?)
    }

    async fn copy_file(&self, from: &str, to: &str) -> AppResult<bool> {
        let n = tokio::fs::copy(from, to).await?;
        metrics::add_read(Channel::Disk, n);
        metrics::add_write(Channel::Disk, n);
        Ok(true)
    }

    async fn reader(&self, path: &str) -> AppResult<BoxReader> {
        let f = tokio::fs::File::open(path).await?;
        Ok(Box::new(CountingReader::new(f, Channel::Disk)))
    }

    async fn writer(&self, path: &str) -> AppResult<BoxWriter> {
        let f = tokio::fs::File::create(path).await?;
        Ok(Box::new(CountingWriter::new(f, Channel::Disk)))
    }

    fn join(&self, parent: &str, name: &str) -> String {
        Path::new(parent).join(name).to_string_lossy().into_owned()
    }

    fn parent(&self, path: &str) -> Option<String> {
        Path::new(path).parent().map(|p| p.to_string_lossy().into_owned())
    }

    fn file_name(&self, path: &str) -> String {
        Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string())
    }

    fn crumbs(&self, path: &str, _root_label: &str) -> Vec<Crumb> {
        let mut out = Vec::new();
        let mut acc = PathBuf::new();
        for c in Path::new(path).components() {
            acc.push(c.as_os_str());
            let name = match c {
                Component::Prefix(p) => p.as_os_str().to_string_lossy().into_owned(),
                Component::RootDir => {
                    // On Windows the prefix crumb ("C:") already represents the drive root.
                    if !out.is_empty() {
                        if let Some(last) = out.last_mut() {
                            let last: &mut Crumb = last;
                            last.path = acc.to_string_lossy().into_owned();
                        }
                        continue;
                    }
                    std::path::MAIN_SEPARATOR.to_string()
                }
                Component::Normal(n) => n.to_string_lossy().into_owned(),
                Component::CurDir | Component::ParentDir => continue,
            };
            out.push(Crumb { name, path: acc.to_string_lossy().into_owned() });
        }
        out
    }
}

/// Renaming "a.txt" -> "A.txt" on a case-insensitive FS hits "exists" for itself.
fn same_file_case_insensitive(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b) && a != b
}

/// Total size and file count of a tree (for the properties panel).
pub fn tree_size(path: &Path) -> (u64, u64, u64) {
    let (mut bytes, mut files, mut dirs) = (0u64, 0u64, 0u64);
    for e in walkdir::WalkDir::new(path).follow_links(false).into_iter().flatten() {
        if e.file_type().is_dir() {
            dirs += 1;
        } else if let Ok(m) = e.metadata() {
            files += 1;
            bytes += m.len();
        }
    }
    (bytes, files, dirs.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn local_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        let fs = LocalFs;
        let sub = fs.join(&root, "sub");
        fs.mkdir(&sub).await.unwrap();
        let file = fs.join(&sub, "hello.txt");
        let mut w = fs.writer(&file).await.unwrap();
        w.write_all(b"hello world").await.unwrap();
        w.shutdown().await.unwrap();
        drop(w);

        let list = fs.list(&sub).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "hello.txt");
        assert_eq!(list[0].size, 11);
        assert_eq!(list[0].ext.as_deref(), Some("txt"));
        assert!(list[0].modified.is_some());

        let mut r = fs.reader(&file).await.unwrap();
        let mut s = String::new();
        r.read_to_string(&mut s).await.unwrap();
        assert_eq!(s, "hello world");

        let renamed = fs.join(&sub, "bye.txt");
        fs.rename(&file, &renamed).await.unwrap();
        assert!(!fs.exists(&file).await.unwrap());
        assert!(fs.exists(&renamed).await.unwrap());

        let dot = fs.join(&sub, ".secret");
        std::fs::write(&dot, b"x").unwrap();
        let list = fs.list(&sub).await.unwrap();
        assert!(list.iter().find(|e| e.name == ".secret").unwrap().hidden);

        let (bytes, files, dirs) = tree_size(dir.path());
        assert_eq!((bytes, files, dirs), (12, 2, 1));

        fs.remove(&sub).await.unwrap();
        assert!(!fs.exists(&sub).await.unwrap());
    }

    #[tokio::test]
    async fn rename_refuses_to_clobber() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let err = LocalFs.rename(&a.to_string_lossy(), &b.to_string_lossy()).await.unwrap_err();
        assert!(matches!(err, AppError::AlreadyExists(_)));
    }

    #[test]
    fn crumbs_cover_every_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x").join("y");
        let crumbs = LocalFs.crumbs(&p.to_string_lossy(), "");
        assert!(crumbs.len() >= 3);
        assert_eq!(crumbs.last().unwrap().name, "y");
        assert_eq!(crumbs.last().unwrap().path, p.to_string_lossy());
        assert_eq!(Path::new(&crumbs[0].path).parent(), None);
    }
}
