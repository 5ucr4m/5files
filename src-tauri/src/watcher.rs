//! Watches the local folders currently open in tabs and tells the UI to refresh.

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub struct Watcher {
    inner: Mutex<Option<Debouncer<RecommendedWatcher, RecommendedCache>>>,
    /// Watched dir as the UI knows it → its canonical form (events use the latter on
    /// macOS when the path goes through a symlink, e.g. /tmp → /private/tmp).
    watched: Arc<Mutex<HashMap<PathBuf, PathBuf>>>,
}

impl Watcher {
    /// `on_change` receives the watched directories (as the UI spelled them) that changed.
    pub fn new(on_change: impl Fn(Vec<String>) + Send + 'static) -> Self {
        let watched: Arc<Mutex<HashMap<PathBuf, PathBuf>>> = Arc::default();
        let map = watched.clone();
        let debouncer = new_debouncer(Duration::from_millis(250), None, move |res: DebounceEventResult| {
            let Ok(events) = res else { return };
            let map = map.lock();
            let mut dirs: HashSet<String> = HashSet::new();
            for ev in events {
                for p in &ev.paths {
                    // A change to a child, or to the watched dir itself.
                    for cand in [p.parent(), Some(p.as_path())].into_iter().flatten() {
                        for (orig, canon) in map.iter() {
                            if cand == orig || cand == canon {
                                dirs.insert(orig.to_string_lossy().into_owned());
                            }
                        }
                    }
                }
            }
            if !dirs.is_empty() {
                on_change(dirs.into_iter().collect());
            }
        })
        .ok();
        Self { inner: Mutex::new(debouncer), watched }
    }

    /// Replaces the watched set with `paths` (non-recursive).
    pub fn set(&self, paths: Vec<String>) {
        let next: HashSet<PathBuf> = paths.into_iter().map(PathBuf::from).filter(|p| p.is_dir()).collect();
        let mut guard = self.inner.lock();
        let Some(deb) = guard.as_mut() else { return };
        // Never hold the map lock while calling into notify: its event thread takes the
        // same lock inside the callback.
        let current: HashSet<PathBuf> = self.watched.lock().keys().cloned().collect();
        let removed: Vec<PathBuf> = current.difference(&next).cloned().collect();
        let mut added = Vec::new();
        for p in &removed {
            let _ = deb.unwatch(p);
        }
        for p in next.difference(&current) {
            if deb.watch(p, RecursiveMode::NonRecursive).is_ok() {
                added.push((p.clone(), std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())));
            }
        }
        let mut watched = self.watched.lock();
        for p in removed {
            watched.remove(&p);
        }
        watched.extend(added);
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
        // Deliberately not canonicalized: on macOS the temp dir is behind the
        // /var -> /private/var symlink and events must still map back to this spelling.
        let root = dir.path().to_path_buf();
        w.set(vec![root.to_string_lossy().into_owned()]);
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(root.join("new.txt"), b"x").unwrap();
        let dirs = rx.recv_timeout(Duration::from_secs(5)).expect("no change event");
        assert!(dirs.iter().any(|d| std::path::Path::new(d) == root), "{dirs:?}");
    }
}
