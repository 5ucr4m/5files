//! Folder sizes for the Size column: computed in the background with bounded
//! concurrency, streamed while counting (so big folders show a growing number), cached,
//! and invalidated when something below a folder changes.

use crate::vfs::DynProvider;
use futures::StreamExt;
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Folders measured at the same time (each walk is one blocking thread locally).
const CONCURRENCY: usize = 3;
const PROGRESS_EVERY: Duration = Duration::from_millis(200);
/// Cached results are reused for this long unless invalidated earlier.
const TTL: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum SizeEvent {
    /// Still counting: `bytes`/`files` so far.
    Progress {
        path: String,
        bytes: u64,
        files: u64,
    },
    Done {
        path: String,
        bytes: u64,
        files: u64,
    },
    Error {
        path: String,
        message: String,
    },
}

#[derive(Default)]
pub struct SizeCache {
    map: Mutex<HashMap<(String, String), (u64, u64, Instant)>>,
}

fn sep_boundary(ancestor: &str, path: &str) -> bool {
    let a = ancestor.trim_end_matches(['/', '\\']);
    path == a || path.starts_with(&format!("{a}/")) || path.starts_with(&format!("{a}\\")) || a.is_empty()
}

impl SizeCache {
    fn get(&self, fs: &str, path: &str) -> Option<(u64, u64)> {
        let map = self.map.lock();
        let (b, f, at) = map.get(&(fs.to_string(), path.to_string()))?;
        (at.elapsed() < TTL).then_some((*b, *f))
    }

    fn put(&self, fs: &str, path: &str, bytes: u64, files: u64) {
        self.map.lock().insert((fs.to_string(), path.to_string()), (bytes, files, Instant::now()));
    }

    /// Something changed at `path`: every cached folder containing it is stale.
    pub fn invalidate(&self, fs: &str, path: &str) {
        self.map.lock().retain(|(f, p), _| !(f == fs && sep_boundary(p, path)));
    }
}

/// Measures `paths` (folders of `fs`), emitting progress and one final event per path.
pub async fn run(
    provider: DynProvider,
    fs: String,
    paths: Vec<String>,
    cache: Arc<SizeCache>,
    cancel: CancellationToken,
    emit: impl Fn(SizeEvent) + Send + Sync + 'static,
) {
    let emit = Arc::new(emit);
    futures::stream::iter(paths)
        .map(|path| {
            let (provider, fs, cache, cancel, emit) = (provider.clone(), fs.clone(), cache.clone(), cancel.clone(), emit.clone());
            async move {
                if cancel.is_cancelled() {
                    return;
                }
                if let Some((bytes, files)) = cache.get(&fs, &path) {
                    emit(SizeEvent::Done { path, bytes, files });
                    return;
                }
                let result = if provider.is_remote() {
                    measure_remote(&provider, &path, &cancel, &*emit).await
                } else {
                    let (p, c, e) = (path.clone(), cancel.clone(), emit.clone());
                    tokio::task::spawn_blocking(move || measure_local(&p, &c, &*e)).await.unwrap_or(None)
                };
                match result {
                    Some((bytes, files)) => {
                        cache.put(&fs, &path, bytes, files);
                        emit(SizeEvent::Done { path, bytes, files });
                    }
                    None if cancel.is_cancelled() => {}
                    None => emit(SizeEvent::Error { path, message: "couldn't read this folder".into() }),
                }
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect::<Vec<()>>()
        .await;
}

/// Apparent size of every file below `root` (links are not followed). `None` if
/// cancelled or the folder itself can't be read; unreadable children are skipped.
///
/// The tree is walked by a small pool of threads sharing a queue of directories: SSDs
/// answer many metadata requests at once, so this is several times faster than a
/// single-threaded walk (or `du`) on big trees.
fn measure_local(root: &str, cancel: &CancellationToken, emit: &dyn Fn(SizeEvent)) -> Option<(u64, u64)> {
    use std::sync::Condvar;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

    std::fs::read_dir(root).ok()?;
    struct Shared {
        queue: std::sync::Mutex<Vec<std::path::PathBuf>>,
        ready: Condvar,
        /// Directories queued or being read; the walk is over when it reaches 0.
        pending: AtomicUsize,
        bytes: AtomicU64,
        files: AtomicU64,
    }
    let shared = Shared {
        queue: std::sync::Mutex::new(vec![std::path::PathBuf::from(root)]),
        ready: Condvar::new(),
        pending: AtomicUsize::new(1),
        bytes: AtomicU64::new(0),
        files: AtomicU64::new(0),
    };
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let dir = {
                        let mut q = shared.queue.lock().unwrap();
                        loop {
                            if let Some(d) = q.pop() {
                                break Some(d);
                            }
                            if shared.pending.load(Relaxed) == 0 || cancel.is_cancelled() {
                                break None;
                            }
                            q = shared.ready.wait_timeout(q, Duration::from_millis(50)).unwrap().0;
                        }
                    };
                    let Some(dir) = dir else { return };
                    let (mut bytes, mut files, mut subdirs) = (0u64, 0u64, Vec::new());
                    if let Ok(rd) = std::fs::read_dir(&dir) {
                        for de in rd.flatten() {
                            // file_type() comes from the directory listing (no extra stat).
                            let Ok(ft) = de.file_type() else { continue };
                            if ft.is_dir() {
                                subdirs.push(de.path());
                            } else if ft.is_file() {
                                if let Ok(m) = de.metadata() {
                                    bytes += m.len();
                                    files += 1;
                                }
                            }
                        }
                    }
                    shared.bytes.fetch_add(bytes, Relaxed);
                    shared.files.fetch_add(files, Relaxed);
                    if !subdirs.is_empty() && !cancel.is_cancelled() {
                        shared.pending.fetch_add(subdirs.len(), Relaxed);
                        shared.queue.lock().unwrap().extend(subdirs);
                        shared.ready.notify_all();
                    }
                    if shared.pending.fetch_sub(1, Relaxed) == 1 {
                        shared.ready.notify_all();
                    }
                }
            });
        }
        // This thread reports progress until the workers drain the tree.
        while shared.pending.load(Relaxed) > 0 && !cancel.is_cancelled() {
            std::thread::sleep(PROGRESS_EVERY);
            emit(SizeEvent::Progress { path: root.to_string(), bytes: shared.bytes.load(Relaxed), files: shared.files.load(Relaxed) });
        }
        shared.ready.notify_all();
    });
    if cancel.is_cancelled() {
        return None;
    }
    Some((shared.bytes.load(Relaxed), shared.files.load(Relaxed)))
}

async fn measure_remote(
    p: &DynProvider,
    root: &str,
    cancel: &CancellationToken,
    emit: &(dyn Fn(SizeEvent) + Send + Sync),
) -> Option<(u64, u64)> {
    let (mut bytes, mut files, mut last) = (0u64, 0u64, Instant::now());
    let mut stack = vec![root.to_string()];
    let mut first = true;
    while let Some(dir) = stack.pop() {
        if cancel.is_cancelled() {
            return None;
        }
        let entries = match p.list(&dir).await {
            Ok(e) => e,
            Err(_) if first => return None,
            Err(_) => continue,
        };
        first = false;
        for e in entries {
            // Folder links are not followed (a `self -> .` link would never end).
            if e.is_dir && !e.is_symlink {
                stack.push(e.path);
            } else if !e.is_dir {
                bytes += e.size;
                files += 1;
            }
        }
        if last.elapsed() >= PROGRESS_EVERY {
            emit(SizeEvent::Progress { path: root.to_string(), bytes, files });
            last = Instant::now();
        }
    }
    Some((bytes, files))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::local::LocalFs;

    fn collect() -> (Arc<Mutex<Vec<SizeEvent>>>, impl Fn(SizeEvent) + Send + Sync + 'static) {
        let ev = Arc::new(Mutex::new(Vec::new()));
        let e2 = ev.clone();
        (ev, move |e| e2.lock().push(e))
    }

    #[tokio::test]
    async fn measures_nested_folders_and_caches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b/c")).unwrap();
        std::fs::write(dir.path().join("a/one.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(dir.path().join("a/b/c/two.bin"), vec![0u8; 2345]).unwrap();
        std::fs::create_dir(dir.path().join("empty")).unwrap();
        let a = dir.path().join("a").to_string_lossy().into_owned();
        let empty = dir.path().join("empty").to_string_lossy().into_owned();
        let cache = Arc::new(SizeCache::default());
        let (ev, emit) = collect();
        run(Arc::new(LocalFs), "local".into(), vec![a.clone(), empty.clone()], cache.clone(), CancellationToken::new(), emit).await;
        let done: Vec<SizeEvent> = ev.lock().iter().filter(|e| matches!(e, SizeEvent::Done { .. })).cloned().collect();
        assert!(done.contains(&SizeEvent::Done { path: a.clone(), bytes: 3345, files: 2 }), "{done:?}");
        assert!(done.contains(&SizeEvent::Done { path: empty.clone(), bytes: 0, files: 0 }));

        // Cached until something inside changes.
        std::fs::write(dir.path().join("a/b/three.bin"), vec![0u8; 5]).unwrap();
        assert_eq!(cache.get("local", &a), Some((3345, 2)));
        cache.invalidate("local", &dir.path().join("a/b").to_string_lossy());
        assert_eq!(cache.get("local", &a), None, "ancestor of the change must be invalidated");
        assert_eq!(cache.get("local", &empty), Some((0, 0)), "unrelated folder stays cached");
    }

    #[tokio::test]
    async fn cancelled_runs_emit_nothing_final() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("x")).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (ev, emit) = collect();
        run(Arc::new(LocalFs), "local".into(), vec![dir.path().join("x").to_string_lossy().into_owned()], Arc::default(), cancel, emit)
            .await;
        assert!(ev.lock().is_empty());
    }

    /// `FIVEFILES_BENCH_DIR=~/Projects cargo test --release --lib bench_local -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_local() {
        let dir = std::env::var("FIVEFILES_BENCH_DIR").expect("set FIVEFILES_BENCH_DIR");
        let t = Instant::now();
        let (bytes, files) = measure_local(&dir, &CancellationToken::new(), &|_| {}).unwrap();
        eprintln!("{files} files, {:.2} GB in {:?}", bytes as f64 / 1e9, t.elapsed());
    }

    #[test]
    fn parallel_walk_matches_a_simple_count() {
        let dir = tempfile::tempdir().unwrap();
        let (mut want_bytes, mut want_files) = (0u64, 0u64);
        for i in 0..40 {
            let d = dir.path().join(format!("d{}", i % 7)).join(format!("s{}", i % 3));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(format!("f{i}")), vec![1u8; i * 13]).unwrap();
            want_bytes += (i * 13) as u64;
            want_files += 1;
        }
        let got = measure_local(&dir.path().to_string_lossy(), &CancellationToken::new(), &|_| {}).unwrap();
        assert_eq!(got, (want_bytes, want_files));
    }

    #[test]
    fn invalidation_respects_name_boundaries() {
        assert!(sep_boundary("/a", "/a/b"));
        assert!(sep_boundary("C:\\x", "C:\\x\\y"));
        assert!(!sep_boundary("/a", "/ab"));
    }
}
