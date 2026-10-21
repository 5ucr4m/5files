//! Watches the local folders currently open in tabs and tells the UI to refresh.

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Watcher {
    inner: Mutex<Option<Debouncer<RecommendedWatcher, RecommendedCache>>>,
    watched: Mutex<HashSet<PathBuf>>,
}

impl Watcher {
    /// `on_change` receives the de-duplicated set of watched directories that changed.
    pub fn new(on_change: impl Fn(Vec<String>) + Send + 'static) -> Self {
        let debouncer = new_debouncer(Duration::from_millis(250), None, move |res: DebounceEventResult| {
            let Ok(events) = res else { return };
            let mut dirs: HashSet<String> = HashSet::new();
            for ev in events {
                for p in &ev.paths {
                    if let Some(parent) = p.parent() {
                        dirs.insert(parent.to_string_lossy().into_owned());
                    }
                    // The watched dir itself may be the event subject (e.g. deleted).
                    dirs.insert(p.to_string_lossy().into_owned());
                }
            }
            if !dirs.is_empty() {
                on_change(dirs.into_iter().collect());
            }
        })
        .ok();
        Self { inner: Mutex::new(debouncer), watched: Mutex::new(HashSet::new()) }
    }

    /// Replaces the watched set with `paths` (non-recursive).
    pub fn set(&self, paths: Vec<String>) {
        let next: HashSet<PathBuf> = paths.into_iter().map(PathBuf::from).filter(|p| p.is_dir()).collect();
        let mut watched = self.watched.lock();
        let mut guard = self.inner.lock();
        let Some(deb) = guard.as_mut() else { return };
        for old in watched.difference(&next) {
            let _ = deb.unwatch(old);
        }
        for new in next.difference(&watched) {
            let _ = deb.watch(Path::new(new), RecursiveMode::NonRecursive);
        }
        *watched = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn reports_changes_in_watched_dir() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let w = Watcher::new(move |dirs| {
            let _ = tx.send(dirs);
        });
        // Canonicalize: on macOS the temp dir lives behind the /var -> /private/var symlink.
        let root = dir.path().canonicalize().unwrap();
        w.set(vec![root.to_string_lossy().into_owned()]);
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(root.join("new.txt"), b"x").unwrap();
        let dirs = rx.recv_timeout(Duration::from_secs(5)).expect("no change event");
        assert!(dirs.iter().any(|d| Path::new(d) == root), "{dirs:?}");
    }
}
