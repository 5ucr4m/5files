use super::{BoxReader, BoxWriter, Caps, Crumb, Entry, Provider};
use crate::error::{AppError, AppResult};
use crate::metrics::{self, Channel, CountingReader, CountingWriter};
use async_trait::async_trait;
use std::fs::Metadata;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

pub struct LocalFs;

fn to_ms(t: std::io::Result<std::time::SystemTime>) -> Option<i64> {
    t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64)
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
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned());
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
        if meta.file_type().is_symlink() {
            // Remove the link itself, never its target. Windows directory symlinks and
            // junctions must be removed with remove_dir.
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
                if meta.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0 {
                    return Ok(tokio::fs::remove_dir(path).await?);
                }
            }
            tokio::fs::remove_file(path).await?;
        } else if meta.is_dir() {
            tokio::fs::remove_dir_all(path).await?;
        } else {
            tokio::fs::remove_file(path).await?;
        }
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str) -> AppResult<()> {
        // The target may "exist" only because it is the same file under another case
        // (a.txt -> A.txt on a case-insensitive disk). Anything else is a real clash.
        if tokio::fs::symlink_metadata(to).await.is_ok() && !self.same_entry(from, to).await {
            return Err(AppError::AlreadyExists(to.into()));
        }
        Ok(tokio::fs::rename(from, to).await?)
    }

    fn check_name(&self, name: &str) -> AppResult<()> {
        super::validate_name(name)
    }

    async fn same_entry(&self, a: &str, b: &str) -> bool {
        if a == b {
            return true;
        }
        let (a, b) = (a.to_string(), b.to_string());
        tokio::task::spawn_blocking(move || same_file::is_same_file(&a, &b).unwrap_or(false)).await.unwrap_or(false)
    }

    async fn contains(&self, ancestor: &str, path: &str) -> bool {
        let (a, p) = (ancestor.to_string(), path.to_string());
        tokio::task::spawn_blocking(move || path_contains(&canonical(&a), &canonical(&p), CASE_INSENSITIVE)).await.unwrap_or(false)
    }

    fn case_insensitive(&self) -> bool {
        CASE_INSENSITIVE
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
        Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
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

/// Default volumes on macOS (APFS/HFS+) and Windows (NTFS) ignore case.
const CASE_INSENSITIVE: bool = cfg!(any(target_os = "macos", windows));

/// Resolves symlinks/`..`/case where possible; for paths that don't exist yet, resolves
/// the nearest existing ancestor and re-appends the rest.
fn canonical(p: &str) -> PathBuf {
    let path = Path::new(p);
    let mut tail = Vec::new();
    let mut cur = path;
    loop {
        if let Ok(c) = dunce_canonicalize(cur) {
            let mut out = c;
            for t in tail.iter().rev() {
                out.push(t);
            }
            return out;
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                cur = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// `std::fs::canonicalize` without Windows' `\\?\` prefix (keeps comparisons simple).
fn dunce_canonicalize(p: &Path) -> std::io::Result<PathBuf> {
    let c = std::fs::canonicalize(p)?;
    #[cfg(windows)]
    {
        let s = c.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            if !rest.starts_with("UNC\\") {
                return Ok(PathBuf::from(rest));
            }
        }
    }
    Ok(c)
}

/// Component-wise ancestor test (so `/a` doesn't contain `/ab`).
pub fn path_contains(ancestor: &Path, path: &Path, case_insensitive: bool) -> bool {
    let eq = |x: &std::ffi::OsStr, y: &std::ffi::OsStr| {
        if case_insensitive { x.to_string_lossy().to_lowercase() == y.to_string_lossy().to_lowercase() } else { x == y }
    };
    let a: Vec<_> = ancestor.components().map(|c| c.as_os_str().to_os_string()).collect();
    let p: Vec<_> = path.components().map(|c| c.as_os_str().to_os_string()).collect();
    a.len() <= p.len() && a.iter().zip(&p).all(|(x, y)| eq(x, y))
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

    #[tokio::test]
    async fn identity_handles_trailing_separators_case_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("Docs");
        std::fs::create_dir(&a).unwrap();
        let fs = LocalFs;
        let a_s = a.to_string_lossy().into_owned();
        assert!(fs.contains(&a_s, &format!("{a_s}{}", std::path::MAIN_SEPARATOR)).await);
        assert!(fs.contains(&a_s, &a.join("x").join("y").to_string_lossy()).await, "not-yet-existing child");
        assert!(!fs.contains(&a_s, &dir.path().join("Docs2").to_string_lossy()).await);
        if CASE_INSENSITIVE {
            assert!(fs.contains(&dir.path().join("docs").to_string_lossy(), &a.join("z").to_string_lossy()).await);
        }
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&a, &link).unwrap();
            assert!(fs.contains(&a_s, &link.join("inner").to_string_lossy()).await, "through a symlink");
            assert!(fs.same_entry(&a_s, &link.to_string_lossy()).await);
            // Removing the link must not touch the target.
            std::fs::write(a.join("keep.txt"), b"x").unwrap();
            fs.remove(&link.to_string_lossy()).await.unwrap();
            assert!(a.join("keep.txt").exists());
        }
    }

    #[tokio::test]
    async fn case_only_rename_never_clobbers_another_file() {
        let dir = tempfile::tempdir().unwrap();
        let lower = dir.path().join("a.txt");
        std::fs::write(&lower, b"lower").unwrap();
        let upper = dir.path().join("A.txt");
        let fs = LocalFs;
        if CASE_INSENSITIVE {
            // Same file under another case: allowed.
            fs.rename(&lower.to_string_lossy(), &upper.to_string_lossy()).await.unwrap();
            assert_eq!(std::fs::read(&upper).unwrap(), b"lower");
        } else {
            std::fs::write(&upper, b"upper").unwrap();
            let err = fs.rename(&lower.to_string_lossy(), &upper.to_string_lossy()).await.unwrap_err();
            assert!(matches!(err, AppError::AlreadyExists(_)));
            assert_eq!(std::fs::read(&upper).unwrap(), b"upper");
        }
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
