//! Virtual file system: one trait, many backends (local disk, S3, Google Drive,
//! Nextcloud/WebDAV, SFTP, FTP…). Every location in the UI is a `(fs, path)` pair
//! where `fs` is `"local"` or a connection id.

pub mod dal;
pub mod ftp_empty;
pub mod local;
pub mod sftp;
pub mod spool;

use crate::error::{AppError, AppResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};

pub type BoxReader = Box<dyn AsyncRead + Send + Unpin>;
pub type BoxWriter = Box<dyn AsyncWrite + Send + Unpin>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    /// Unix epoch milliseconds.
    pub modified: Option<i64>,
    pub created: Option<i64>,
    pub hidden: bool,
    pub readonly: bool,
    pub ext: Option<String>,
}

impl Entry {
    pub fn new(name: impl Into<String>, path: impl Into<String>, is_dir: bool) -> Self {
        let name = name.into();
        let ext = if is_dir { None } else { ext_of(&name) };
        Entry {
            hidden: name.starts_with('.'),
            name,
            path: path.into(),
            is_dir,
            is_symlink: false,
            size: 0,
            modified: None,
            created: None,
            readonly: false,
            ext,
        }
    }
}

pub fn ext_of(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() || ext.len() > 12 {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Crumb {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Caps {
    pub rename: bool,
    pub trash: bool,
    pub thumbnails: bool,
    pub server_copy: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub total: u64,
    pub free: u64,
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// Whether I/O goes over the network (drives which metrics counter is charged).
    fn is_remote(&self) -> bool;
    fn caps(&self) -> Caps;
    /// Path that represents the top of this file system.
    fn root(&self) -> String {
        "/".into()
    }

    async fn list(&self, path: &str) -> AppResult<Vec<Entry>>;
    async fn stat(&self, path: &str) -> AppResult<Entry>;
    async fn exists(&self, path: &str) -> AppResult<bool> {
        match self.stat(path).await {
            Ok(_) => Ok(true),
            Err(AppError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }
    async fn mkdir(&self, path: &str) -> AppResult<()>;
    /// Removes a file, or a directory with all of its contents.
    async fn remove(&self, path: &str) -> AppResult<()>;
    async fn rename(&self, from: &str, to: &str) -> AppResult<()>;
    /// Server-side copy of a single file. Returns `Ok(false)` when unsupported so the
    /// caller falls back to streaming.
    async fn copy_file(&self, _from: &str, _to: &str) -> AppResult<bool> {
        Ok(false)
    }
    async fn reader(&self, path: &str) -> AppResult<BoxReader>;
    /// Bytes written are only guaranteed persisted after `shutdown()` succeeds.
    async fn writer(&self, path: &str) -> AppResult<BoxWriter>;
    async fn usage(&self) -> AppResult<Option<Usage>> {
        Ok(None)
    }

    // --- identity & naming (safety checks in the transfer engine) ---

    /// Rejects names that can't be created here (or would escape the parent folder).
    fn check_name(&self, name: &str) -> AppResult<()> {
        check_name(name, false)
    }
    /// Whether two paths are the same entry (not just the same string).
    async fn same_entry(&self, a: &str, b: &str) -> bool {
        posix::normalize(a) == posix::normalize(b)
    }
    /// Whether `path` is `ancestor` or lies inside it.
    async fn contains(&self, ancestor: &str, path: &str) -> bool {
        let (a, p) = (posix::normalize(ancestor), posix::normalize(path));
        a == p || a == "/" || p.starts_with(&format!("{a}/"))
    }
    /// Names differing only in case collide (macOS / Windows local disks).
    fn case_insensitive(&self) -> bool {
        false
    }

    // --- path helpers (POSIX-style by default; local overrides with OS semantics) ---

    fn join(&self, parent: &str, name: &str) -> String {
        posix::join(parent, name)
    }
    fn parent(&self, path: &str) -> Option<String> {
        posix::parent(path)
    }
    fn file_name(&self, path: &str) -> String {
        posix::file_name(path)
    }
    fn crumbs(&self, path: &str, root_label: &str) -> Vec<Crumb> {
        posix::crumbs(path, root_label)
    }
}

pub type DynProvider = Arc<dyn Provider>;

pub mod posix {
    use super::Crumb;

    pub fn normalize(path: &str) -> String {
        let mut out = Vec::new();
        for seg in path.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    out.pop();
                }
                s => out.push(s),
            }
        }
        format!("/{}", out.join("/"))
    }

    pub fn join(parent: &str, name: &str) -> String {
        let p = normalize(parent);
        if p == "/" { format!("/{name}") } else { format!("{p}/{name}") }
    }

    pub fn parent(path: &str) -> Option<String> {
        let p = normalize(path);
        if p == "/" {
            return None;
        }
        let idx = p.rfind('/').unwrap_or(0);
        Some(if idx == 0 { "/".into() } else { p[..idx].to_string() })
    }

    pub fn file_name(path: &str) -> String {
        normalize(path).rsplit('/').next().unwrap_or("").to_string()
    }

    pub fn crumbs(path: &str, root_label: &str) -> Vec<Crumb> {
        let p = normalize(path);
        let mut out = vec![Crumb { name: root_label.into(), path: "/".into() }];
        let mut acc = String::new();
        for seg in p.split('/').filter(|s| !s.is_empty()) {
            acc.push('/');
            acc.push_str(seg);
            out.push(Crumb { name: seg.into(), path: acc.clone() });
        }
        out
    }

    /// Path relative to a root, for use when mapping a source tree onto a destination.
    pub fn relative(root: &str, path: &str) -> Option<String> {
        let r = normalize(root);
        let p = normalize(path);
        if r == "/" {
            return Some(p.trim_start_matches('/').to_string());
        }
        p.strip_prefix(&r).map(|rest| rest.trim_start_matches('/').to_string())
    }
}

/// Produces "name (2).ext", "name (3).ext"… for "keep both" conflict resolution.
pub fn numbered_name(name: &str, n: u32, is_dir: bool) -> String {
    match (is_dir, name.rsplit_once('.')) {
        (false, Some((stem, ext))) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    }
}

/// Finds the first free name in `parent` (checking the provider).
pub async fn unique_name(p: &dyn Provider, parent: &str, name: &str, is_dir: bool) -> AppResult<String> {
    let first = p.join(parent, name);
    if !p.exists(&first).await? {
        return Ok(first);
    }
    for n in 2..10_000 {
        let cand = p.join(parent, &numbered_name(name, n, is_dir));
        if !p.exists(&cand).await? {
            return Ok(cand);
        }
    }
    Err(AppError::AlreadyExists(name.into()))
}

/// Validates a name for the current OS's local file system.
pub fn validate_name(name: &str) -> AppResult<()> {
    check_name(name, cfg!(windows))
}

const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4",
    "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Name rules. Always: non-empty, not `.`/`..`, no path separators (`/` or `\`) and no
/// NUL — so a name from a remote listing can never escape its parent folder. With
/// `windows`: also no `<>:"|?*` (`:` would create an NTFS alternate data stream),
/// control characters, trailing dot/space, or reserved device names (`CON`, `aux.c`…).
pub fn check_name(name: &str, windows: bool) -> AppResult<()> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.trim().is_empty();
    if bad {
        return Err(AppError::Invalid(format!("invalid file name: {name:?}")));
    }
    if windows {
        const BAD: &[char] = &['<', '>', ':', '"', '|', '?', '*'];
        let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
        if name.chars().any(|c| BAD.contains(&c) || (c as u32) < 32)
            || name.ends_with('.')
            || name.ends_with(' ')
            || WINDOWS_RESERVED.contains(&stem.as_str())
        {
            return Err(AppError::Invalid(format!("name not allowed on Windows: {name:?}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_helpers() {
        assert_eq!(posix::normalize("//a/./b/../c/"), "/a/c");
        assert_eq!(posix::join("/", "x"), "/x");
        assert_eq!(posix::join("/a/b/", "x"), "/a/b/x");
        assert_eq!(posix::parent("/a/b"), Some("/a".into()));
        assert_eq!(posix::parent("/a"), Some("/".into()));
        assert_eq!(posix::parent("/"), None);
        assert_eq!(posix::file_name("/a/b.txt"), "b.txt");
        let c = posix::crumbs("/a/b", "bucket");
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].name, "bucket");
        assert_eq!(c[2].path, "/a/b");
        assert_eq!(posix::relative("/a", "/a/b/c"), Some("b/c".into()));
        assert_eq!(posix::relative("/", "/a/b"), Some("a/b".into()));
        assert_eq!(posix::relative("/x", "/a/b"), None);
    }

    #[test]
    fn numbered_names() {
        assert_eq!(numbered_name("photo.jpg", 2, false), "photo (2).jpg");
        assert_eq!(numbered_name("Makefile", 3, false), "Makefile (3)");
        assert_eq!(numbered_name(".env", 2, false), ".env (2)");
        assert_eq!(numbered_name("my.folder", 2, true), "my.folder (2)");
    }

    #[test]
    fn ext_detection() {
        assert_eq!(ext_of("a.TXT"), Some("txt".into()));
        assert_eq!(ext_of(".bashrc"), None);
        assert_eq!(ext_of("noext"), None);
        assert_eq!(ext_of("archive.tar.gz"), Some("gz".into()));
    }

    #[test]
    fn hostile_names_are_rejected() {
        for n in ["../x", "..", "a/b", "..\\..\\evil.bat", "a\0b"] {
            assert!(check_name(n, false).is_err(), "{n:?}");
        }
        for n in ["foo:bar", "aux.c", "CON", "nul.txt", "trailing.", "tab\tname", "a?b"] {
            assert!(check_name(n, true).is_err(), "{n:?}");
            assert!(check_name(n, false).is_ok() || n.contains('\t'), "{n:?} should be fine off Windows");
        }
        assert!(check_name("auxiliary.txt", true).is_ok());
        assert!(check_name("report (2).pdf", true).is_ok());
    }

    #[tokio::test]
    async fn default_identity_helpers_are_boundary_aware() {
        let p = dal::DalFs::new(opendal::Operator::new(opendal::services::Memory::default()).unwrap().finish(), true);
        assert!(p.contains("/a", "/a/b").await);
        assert!(p.contains("/a/", "/a").await);
        assert!(!p.contains("/a", "/ab").await);
        assert!(p.same_entry("/a/b/", "/a/b").await);
    }

    #[test]
    fn name_validation() {
        assert!(validate_name("ok.txt").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("   ").is_err());
    }
}
