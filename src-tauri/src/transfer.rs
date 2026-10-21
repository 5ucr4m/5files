//! Transfer queue: copy / move / delete jobs between any two providers, with
//! byte-accurate progress, speed, ETA, pause/resume, cancel and retry.

use crate::error::{AppError, AppResult};
use crate::vfs::{self, DynProvider, local::FAST_COPY_MAX};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Semaphore, watch};
use tokio_util::sync::CancellationToken;

/// Resolves a file-system id ("local" or a connection id) to a provider.
pub type Resolver = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = AppResult<DynProvider>> + Send>> + Send + Sync>;
/// Called whenever a job's state changes in a way the UI should see (throttled by caller).
pub type Notify = Arc<dyn Fn() + Send + Sync>;
/// Called once when a job finishes successfully (e.g. to open a downloaded file).
pub type OnDone = Arc<dyn Fn(&JobSnapshot) + Send + Sync>;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum JobKind {
    Copy,
    Move,
    Delete,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Conflict {
    Overwrite,
    Skip,
    #[default]
    KeepBoth,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Queued,
    Scanning,
    Running,
    Paused,
    Done,
    Failed,
    Cancelled,
}

impl Status {
    pub fn is_final(self) -> bool {
        matches!(self, Status::Done | Status::Failed | Status::Cancelled)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Loc {
    pub fs: String,
    pub path: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRequest {
    pub kind: JobKind,
    pub sources: Vec<Loc>,
    /// Destination directory (ignored for delete).
    pub dest: Option<Loc>,
    #[serde(default)]
    pub conflict: Conflict,
    /// Open the resulting file with the default app when done (remote "open").
    #[serde(default)]
    pub open_after: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub id: String,
    pub kind: JobKind,
    pub status: Status,
    pub title: String,
    pub src_fs: String,
    pub dest: Option<Loc>,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub total_files: u64,
    pub done_files: u64,
    pub skipped: u64,
    pub current: Option<String>,
    pub speed: f64,
    pub eta_secs: Option<f64>,
    pub error: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub open_after: bool,
    /// Final paths created in the destination (top-level items only).
    pub outputs: Vec<String>,
}

struct Job {
    req: JobRequest,
    snap: Mutex<JobSnapshot>,
    cancel: CancellationToken,
    pause: watch::Sender<bool>,
    speed: Mutex<SpeedMeter>,
}

/// Exponentially smoothed throughput.
struct SpeedMeter {
    last_bytes: u64,
    last_at: Instant,
    ema: f64,
}

impl SpeedMeter {
    fn new() -> Self {
        Self { last_bytes: 0, last_at: Instant::now(), ema: 0.0 }
    }
    fn update(&mut self, bytes: u64) -> f64 {
        let now = Instant::now();
        let dt = now.duration_since(self.last_at).as_secs_f64();
        if dt >= 0.25 {
            let inst = bytes.saturating_sub(self.last_bytes) as f64 / dt;
            self.ema = if self.ema == 0.0 { inst } else { self.ema * 0.7 + inst * 0.3 };
            self.last_bytes = bytes;
            self.last_at = now;
        }
        self.ema
    }
}

pub struct Manager {
    jobs: Mutex<VecDeque<Arc<Job>>>,
    slots: Arc<Semaphore>,
    resolve: Resolver,
    notify: Notify,
    on_done: OnDone,
}

const MAX_HISTORY: usize = 200;
const BUF: usize = 1024 * 1024;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Manager {
    pub fn new(concurrency: usize, resolve: Resolver, notify: Notify, on_done: OnDone) -> Arc<Self> {
        Arc::new(Self { jobs: Mutex::new(VecDeque::new()), slots: Arc::new(Semaphore::new(concurrency.max(1))), resolve, notify, on_done })
    }

    pub fn snapshots(&self) -> Vec<JobSnapshot> {
        self.jobs.lock().iter().map(|j| j.snap.lock().clone()).collect()
    }

    pub fn active_count(&self) -> usize {
        self.jobs.lock().iter().filter(|j| !j.snap.lock().status.is_final()).count()
    }

    fn find(&self, id: &str) -> AppResult<Arc<Job>> {
        self.jobs.lock().iter().find(|j| j.snap.lock().id == id).cloned().ok_or_else(|| AppError::NotFound(format!("job {id}")))
    }

    pub fn enqueue(self: &Arc<Self>, req: JobRequest) -> AppResult<String> {
        if req.sources.is_empty() {
            return Err(AppError::Invalid("nothing to transfer".into()));
        }
        if req.kind != JobKind::Delete && req.dest.is_none() {
            return Err(AppError::Invalid("destination required".into()));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let names: Vec<String> = req.sources.iter().map(|s| display_name(&s.path)).collect();
        let title = if names.len() == 1 { names[0].clone() } else { format!("{} items", names.len()) };
        let snap = JobSnapshot {
            id: id.clone(),
            kind: req.kind,
            status: Status::Queued,
            title,
            src_fs: req.sources[0].fs.clone(),
            dest: req.dest.clone(),
            total_bytes: 0,
            done_bytes: 0,
            total_files: 0,
            done_files: 0,
            skipped: 0,
            current: None,
            speed: 0.0,
            eta_secs: None,
            error: None,
            created_at: now_ms(),
            finished_at: None,
            open_after: req.open_after,
            outputs: vec![],
        };
        let (pause, _) = watch::channel(false);
        let job =
            Arc::new(Job { req, snap: Mutex::new(snap), cancel: CancellationToken::new(), pause, speed: Mutex::new(SpeedMeter::new()) });
        {
            let mut jobs = self.jobs.lock();
            jobs.push_back(job.clone());
            // Trim old finished jobs.
            while jobs.len() > MAX_HISTORY {
                match jobs.iter().position(|j| j.snap.lock().status.is_final()) {
                    Some(i) => {
                        jobs.remove(i);
                    }
                    None => break,
                }
            }
        }
        (self.notify)();
        let me = self.clone();
        // Commands may call this from the main thread (no Tokio context): use Tauri's runtime.
        tauri::async_runtime::spawn(async move { me.run(job).await });
        Ok(id)
    }

    pub fn pause(&self, id: &str, paused: bool) -> AppResult<()> {
        let job = self.find(id)?;
        job.pause.send_replace(paused);
        {
            let mut s = job.snap.lock();
            if !s.status.is_final() {
                if paused && s.status == Status::Running {
                    s.status = Status::Paused;
                    s.speed = 0.0;
                    s.eta_secs = None;
                } else if !paused && s.status == Status::Paused {
                    s.status = Status::Running;
                }
            }
        }
        (self.notify)();
        Ok(())
    }

    pub fn cancel(&self, id: &str) -> AppResult<()> {
        self.find(id)?.cancel.cancel();
        Ok(())
    }

    pub fn retry(self: &Arc<Self>, id: &str) -> AppResult<String> {
        let job = self.find(id)?;
        if !job.snap.lock().status.is_final() {
            return Err(AppError::Invalid("job is still running".into()));
        }
        self.remove(id);
        self.enqueue(job.req.clone())
    }

    pub fn remove(&self, id: &str) {
        self.jobs.lock().retain(|j| {
            let s = j.snap.lock();
            s.id != id || !s.status.is_final()
        });
        (self.notify)();
    }

    pub fn clear_finished(&self) {
        self.jobs.lock().retain(|j| !j.snap.lock().status.is_final());
        (self.notify)();
    }

    fn set(&self, job: &Job, f: impl FnOnce(&mut JobSnapshot)) {
        f(&mut job.snap.lock());
        (self.notify)();
    }

    async fn run(self: Arc<Self>, job: Arc<Job>) {
        let permit = tokio::select! {
            p = self.slots.clone().acquire_owned() => p.ok(),
            _ = job.cancel.cancelled() => None,
        };
        let result = match permit {
            Some(_permit) => {
                let r = self.execute(&job).await;
                drop(_permit);
                r
            }
            None => Err(AppError::Cancelled),
        };
        let finished = {
            let mut s = job.snap.lock();
            s.finished_at = Some(now_ms());
            s.speed = 0.0;
            s.eta_secs = None;
            s.current = None;
            match result {
                Ok(()) => {
                    s.status = Status::Done;
                    s.done_bytes = s.total_bytes.max(s.done_bytes);
                }
                Err(AppError::Cancelled) => s.status = Status::Cancelled,
                Err(e) => {
                    s.status = Status::Failed;
                    s.error = Some(e.to_string());
                }
            }
            s.clone()
        };
        if finished.status == Status::Done {
            (self.on_done)(&finished);
        }
        (self.notify)();
    }

    async fn execute(&self, job: &Arc<Job>) -> AppResult<()> {
        let src = (self.resolve)(job.req.sources[0].fs.clone()).await?;
        self.set(job, |s| s.status = Status::Scanning);
        let plan = self.scan(job, &src).await?;
        self.set(job, |s| {
            s.status = if *job.pause.borrow() { Status::Paused } else { Status::Running };
            s.total_bytes = plan.bytes;
            s.total_files = plan.files;
        });
        if job.req.kind == JobKind::Delete {
            for item in &job.req.sources {
                check(job).await?;
                self.set(job, |s| s.current = Some(display_name(&item.path)));
                src.remove(&item.path).await?;
                self.set(job, |s| s.done_files += plan.counts.get(&item.path).copied().unwrap_or(1));
            }
            return Ok(());
        }

        let dest_loc = job.req.dest.clone().unwrap();
        let dst = (self.resolve)(dest_loc.fs.clone()).await?;
        let same_fs = job.req.sources[0].fs == dest_loc.fs;

        for item in &job.req.sources {
            check(job).await?;
            let src_entry = src.stat(&item.path).await?;
            let name = src.file_name(&item.path);
            dst.check_name(&name)?;
            let (item_bytes, item_files) =
                (plan.sizes.get(&item.path).copied().unwrap_or(0), plan.counts.get(&item.path).copied().unwrap_or(1));
            // Identity checks go through the provider (canonical paths, real file identity),
            // never plain string comparison: case, trailing separators and symlinks differ.
            if same_fs && src_entry.is_dir && src.contains(&item.path, &dest_loc.path).await {
                return Err(AppError::Invalid(format!("can't {} “{name}” into itself", verb(job.req.kind))));
            }
            let mut target = dst.join(&dest_loc.path, &name);
            let mut replace = false;
            if same_fs && src.same_entry(&target, &item.path).await {
                if job.req.kind == JobKind::Move {
                    // Moving onto itself is a no-op.
                    self.add_done(job, item_bytes, item_files);
                    continue;
                }
                // Duplicate in place ("copy" in the same folder): always keep both.
                target = vfs::unique_name(&*dst, &dest_loc.path, &name, src_entry.is_dir).await?;
            } else if dst.exists(&target).await? {
                match job.req.conflict {
                    Conflict::Skip => {
                        self.set(job, |s| s.skipped += item_files);
                        self.add_done(job, item_bytes, item_files);
                        continue;
                    }
                    Conflict::KeepBoth => {
                        target = vfs::unique_name(&*dst, &dest_loc.path, &name, src_entry.is_dir).await?;
                    }
                    Conflict::Overwrite => {
                        // Never replace something that contains what we're transferring.
                        if same_fs && dst.contains(&target, &item.path).await {
                            return Err(AppError::Invalid(format!(
                                "can't replace “{name}”: it contains the item being {}",
                                if job.req.kind == JobKind::Move { "moved" } else { "copied" }
                            )));
                        }
                        let existing = dst.stat(&target).await?;
                        match (existing.is_dir, src_entry.is_dir) {
                            (true, true) => {}                // merge folders
                            (false, false) => replace = true, // atomic replace after the copy
                            _ => dst.remove(&target).await?,  // file <-> folder: type changes
                        }
                    }
                }
            }

            // Fast path: rename within the same file system.
            if job.req.kind == JobKind::Move && same_fs && src.caps().rename && (!dst.exists(&target).await? || replace) {
                if replace {
                    // Source is complete and stays intact until the rename succeeds.
                    dst.remove(&target).await?;
                }
                if src.rename(&item.path, &target).await.is_ok() {
                    self.add_done(job, item_bytes, item_files);
                    job.snap.lock().outputs.push(target);
                    continue;
                }
                // Cross-device move or unsupported: fall back to copy + delete.
            }

            let skipped_before = job.snap.lock().skipped;
            self.copy_tree(job, &src, &dst, &item.path, &target, src_entry.is_dir, same_fs, replace).await?;
            job.snap.lock().outputs.push(target);
            if job.req.kind == JobKind::Move {
                check(job).await?;
                // Children skipped inside this tree (bad names, folder links, conflicts)
                // exist only at the source: keep the source in that case.
                if job.snap.lock().skipped == skipped_before {
                    src.remove(&item.path).await?;
                }
            }
        }
        Ok(())
    }

    fn add_done(&self, job: &Job, bytes: u64, files: u64) {
        self.set(job, |s| {
            s.done_bytes += bytes;
            s.done_files += files;
        });
    }

    async fn scan(&self, job: &Arc<Job>, src: &DynProvider) -> AppResult<Plan> {
        let mut plan = Plan::default();
        for item in &job.req.sources {
            let (mut bytes, mut files) = (0u64, 0u64);
            let mut stack = vec![src.stat(&item.path).await?];
            while let Some(e) = stack.pop() {
                check(job).await?;
                if e.is_dir && !e.is_symlink {
                    stack.extend(src.list(&e.path).await?);
                    if job.req.kind == JobKind::Delete {
                        files += 1;
                    }
                } else {
                    files += 1;
                    bytes += e.size;
                }
            }
            plan.bytes += bytes;
            plan.files += files;
            plan.sizes.insert(item.path.clone(), bytes);
            plan.counts.insert(item.path.clone(), files);
            self.set(job, |s| {
                s.total_bytes = plan.bytes;
                s.total_files = plan.files;
            });
        }
        Ok(plan)
    }

    #[allow(clippy::too_many_arguments)]
    async fn copy_tree(
        &self,
        job: &Arc<Job>,
        src: &DynProvider,
        dst: &DynProvider,
        from: &str,
        to: &str,
        is_dir: bool,
        same_fs: bool,
        replace: bool,
    ) -> AppResult<()> {
        if !is_dir {
            return self.copy_file(job, src, dst, from, to, same_fs, replace).await;
        }
        let mut stack = vec![(from.to_string(), to.to_string())];
        while let Some((sdir, ddir)) = stack.pop() {
            check(job).await?;
            // Existing destination folder = merge: every child may clash.
            let merging = dst.exists(&ddir).await?;
            dst.mkdir(&ddir).await?;
            let mut taken = std::collections::HashSet::new();
            for e in src.list(&sdir).await? {
                // Names come from a (possibly hostile) remote listing: they must be valid
                // single path components on the destination, or they could escape it.
                if dst.check_name(&e.name).is_err() || (e.is_dir && e.is_symlink) {
                    self.set(job, |s| s.skipped += 1);
                    self.add_done(job, e.size, 1);
                    continue;
                }
                let mut target = dst.join(&ddir, &e.name);
                // Two source names that fold to the same destination name ("README" and
                // "readme" onto macOS/Windows) must never overwrite each other.
                let key = if dst.case_insensitive() { e.name.to_lowercase() } else { e.name.clone() };
                let folded_clash = !taken.insert(key);
                let mut replace_file = false;
                if folded_clash {
                    target = vfs::unique_name(&**dst, &ddir, &e.name, e.is_dir).await?;
                } else if merging && dst.exists(&target).await? {
                    match job.req.conflict {
                        Conflict::Skip => {
                            self.set(job, |s| s.skipped += 1);
                            self.add_done(job, e.size, 1);
                            continue;
                        }
                        Conflict::KeepBoth => target = vfs::unique_name(&**dst, &ddir, &e.name, e.is_dir).await?,
                        Conflict::Overwrite => {
                            let existing = dst.stat(&target).await?;
                            match (existing.is_dir, e.is_dir) {
                                (true, true) => {}
                                (false, false) => replace_file = true,
                                _ => dst.remove(&target).await?,
                            }
                        }
                    }
                }
                if e.is_dir {
                    stack.push((e.path, target));
                } else {
                    self.copy_file(job, src, dst, &e.path, &target, same_fs, replace_file).await?;
                }
            }
        }
        Ok(())
    }

    /// Copies one file. With `replace`, the data goes to a temporary sibling first and
    /// only then takes the target's place, so a cancelled or failed copy never destroys
    /// the file being replaced.
    #[allow(clippy::too_many_arguments)]
    async fn copy_file(
        &self,
        job: &Arc<Job>,
        src: &DynProvider,
        dst: &DynProvider,
        from: &str,
        to: &str,
        same_fs: bool,
        replace: bool,
    ) -> AppResult<()> {
        check(job).await?;
        let size = src.stat(from).await.map(|e| e.size).unwrap_or(0);
        self.set(job, |s| s.current = Some(display_name(from)));
        // Object stores replace atomically on write; elsewhere stage next to the target.
        let staged = replace && dst.caps().rename;
        let write_to = if staged {
            let parent = dst.parent(to).unwrap_or_else(|| dst.root());
            let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
            dst.join(&parent, &format!(".{}.5files-part-{tag}", dst.file_name(to)))
        } else {
            to.to_string()
        };

        let result = self.copy_bytes(job, src, dst, from, &write_to, size, same_fs).await;
        if let Err(e) = result {
            // Never leave a truncated file behind (the original, if any, is untouched).
            let _ = dst.remove(&write_to).await;
            return Err(e);
        }
        if staged {
            dst.remove(to).await?;
            dst.rename(&write_to, to).await?;
        }
        self.set(job, |s| s.done_files += 1);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn copy_bytes(
        &self,
        job: &Arc<Job>,
        src: &DynProvider,
        dst: &DynProvider,
        from: &str,
        to: &str,
        size: u64,
        same_fs: bool,
    ) -> AppResult<()> {
        // Same-backend server-side / OS-level copy (APFS clone, S3 CopyObject…).
        let small_or_remote = size <= FAST_COPY_MAX || src.is_remote();
        if same_fs
            && small_or_remote
            && src.caps().server_copy
            && let Ok(true) = src.copy_file(from, to).await
        {
            self.progress(job, size);
            return Ok(());
        }
        let mut reader = src.reader(from).await?;
        let mut writer = dst.writer(to).await?;
        let mut buf = vec![0u8; BUF];
        loop {
            check(job).await?;
            let n = tokio::select! {
                r = reader.read(&mut buf) => r?,
                _ = job.cancel.cancelled() => return Err(AppError::Cancelled),
            };
            if n == 0 {
                break;
            }
            tokio::select! {
                r = writer.write_all(&buf[..n]) => r?,
                _ = job.cancel.cancelled() => return Err(AppError::Cancelled),
            }
            self.progress(job, n as u64);
        }
        // Finishing can be long (WebDAV uploads the spooled file here): keep it cancellable.
        tokio::select! {
            r = writer.shutdown() => r?,
            _ = job.cancel.cancelled() => return Err(AppError::Cancelled),
        }
        Ok(())
    }

    fn progress(&self, job: &Job, n: u64) {
        let mut s = job.snap.lock();
        s.done_bytes += n;
        let speed = job.speed.lock().update(s.done_bytes);
        s.speed = speed;
        s.eta_secs = if speed > 1.0 && s.total_bytes > s.done_bytes { Some((s.total_bytes - s.done_bytes) as f64 / speed) } else { None };
        drop(s);
        (self.notify)();
    }
}

#[derive(Default)]
struct Plan {
    bytes: u64,
    files: u64,
    sizes: std::collections::HashMap<String, u64>,
    counts: std::collections::HashMap<String, u64>,
}

/// Waits while paused; errors if cancelled.
async fn check(job: &Job) -> AppResult<()> {
    if job.cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let mut rx = job.pause.subscribe();
    while *rx.borrow_and_update() {
        tokio::select! {
            r = rx.changed() => { if r.is_err() { break; } }
            _ = job.cancel.cancelled() => return Err(AppError::Cancelled),
        }
    }
    Ok(())
}

fn verb(k: JobKind) -> &'static str {
    match k {
        JobKind::Copy => "copy",
        JobKind::Move => "move",
        JobKind::Delete => "delete",
    }
}

fn display_name(path: &str) -> String {
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::local::LocalFs;
    use std::time::Duration;

    fn manager() -> Arc<Manager> {
        let local: DynProvider = Arc::new(LocalFs);
        let mem: DynProvider =
            Arc::new(vfs::dal::DalFs::new(opendal::Operator::new(opendal::services::Memory::default()).unwrap().finish(), true));
        let resolve: Resolver = Arc::new(move |fs: String| {
            let p = if fs == "mem" { mem.clone() } else { local.clone() };
            Box::pin(async move { Ok(p) })
        });
        Manager::new(2, resolve, Arc::new(|| {}), Arc::new(|_| {}))
    }

    async fn wait(m: &Manager, id: &str) -> JobSnapshot {
        for _ in 0..500 {
            let s = m.snapshots().into_iter().find(|s| s.id == id).unwrap();
            if s.status.is_final() {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("job did not finish");
    }

    fn loc(fs: &str, p: &std::path::Path) -> Loc {
        Loc { fs: fs.into(), path: p.to_string_lossy().into_owned() }
    }

    fn tree(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::write(root.join("src/a.txt"), vec![1u8; 3000]).unwrap();
        std::fs::write(root.join("src/nested/b.bin"), vec![2u8; 2 * 1024 * 1024 + 17]).unwrap();
    }

    #[tokio::test]
    async fn copies_tree_with_byte_accurate_progress() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        std::fs::create_dir(dir.path().join("dst")).unwrap();
        let m = manager();
        let id = m
            .enqueue(JobRequest {
                kind: JobKind::Copy,
                sources: vec![loc("local", &dir.path().join("src"))],
                dest: Some(loc("local", &dir.path().join("dst"))),
                conflict: Conflict::KeepBoth,
                open_after: false,
            })
            .unwrap();
        let s = wait(&m, &id).await;
        assert_eq!(s.status, Status::Done, "{:?}", s.error);
        assert_eq!(s.total_files, 2);
        assert_eq!(s.done_files, 2);
        assert_eq!(s.total_bytes, 3000 + 2 * 1024 * 1024 + 17);
        assert_eq!(s.done_bytes, s.total_bytes);
        assert_eq!(std::fs::read(dir.path().join("dst/src/nested/b.bin")).unwrap().len(), 2 * 1024 * 1024 + 17);

        // Same copy again → keep both creates "src (2)".
        let id = m
            .enqueue(JobRequest {
                kind: JobKind::Copy,
                sources: vec![loc("local", &dir.path().join("src"))],
                dest: Some(loc("local", &dir.path().join("dst"))),
                conflict: Conflict::KeepBoth,
                open_after: false,
            })
            .unwrap();
        assert_eq!(wait(&m, &id).await.status, Status::Done);
        assert!(dir.path().join("dst/src (2)/a.txt").exists());
    }

    #[tokio::test]
    async fn skip_and_overwrite_policies() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), b"new").unwrap();
        std::fs::create_dir(dir.path().join("d")).unwrap();
        std::fs::write(dir.path().join("d/f.txt"), b"old").unwrap();
        let m = manager();
        let mk = |c| JobRequest {
            kind: JobKind::Copy,
            sources: vec![loc("local", &dir.path().join("f.txt"))],
            dest: Some(loc("local", &dir.path().join("d"))),
            conflict: c,
            open_after: false,
        };
        let s = wait(&m, &m.enqueue(mk(Conflict::Skip)).unwrap()).await;
        assert_eq!(s.skipped, 1);
        assert_eq!(std::fs::read(dir.path().join("d/f.txt")).unwrap(), b"old");
        wait(&m, &m.enqueue(mk(Conflict::Overwrite)).unwrap()).await;
        assert_eq!(std::fs::read(dir.path().join("d/f.txt")).unwrap(), b"new");
    }

    #[tokio::test]
    async fn move_across_providers_and_back() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let m = manager();
        // local -> memory (streams through reader/writer, then deletes source)
        let s = wait(
            &m,
            &m.enqueue(JobRequest {
                kind: JobKind::Move,
                sources: vec![loc("local", &dir.path().join("src"))],
                dest: Some(Loc { fs: "mem".into(), path: "/".into() }),
                conflict: Conflict::KeepBoth,
                open_after: false,
            })
            .unwrap(),
        )
        .await;
        assert_eq!(s.status, Status::Done, "{:?}", s.error);
        assert!(!dir.path().join("src").exists());
        // memory -> local
        let s = wait(
            &m,
            &m.enqueue(JobRequest {
                kind: JobKind::Copy,
                sources: vec![Loc { fs: "mem".into(), path: "/src".into() }],
                dest: Some(loc("local", dir.path())),
                conflict: Conflict::KeepBoth,
                open_after: false,
            })
            .unwrap(),
        )
        .await;
        assert_eq!(s.status, Status::Done, "{:?}", s.error);
        assert_eq!(std::fs::read(dir.path().join("src/a.txt")).unwrap(), vec![1u8; 3000]);
        assert_eq!(s.done_bytes, 3000 + 2 * 1024 * 1024 + 17);
    }

    #[tokio::test]
    async fn refuses_to_copy_into_itself() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let m = manager();
        let s = wait(
            &m,
            &m.enqueue(JobRequest {
                kind: JobKind::Copy,
                sources: vec![loc("local", &dir.path().join("src"))],
                dest: Some(loc("local", &dir.path().join("src/nested"))),
                conflict: Conflict::KeepBoth,
                open_after: false,
            })
            .unwrap(),
        )
        .await;
        assert_eq!(s.status, Status::Failed);
    }

    #[tokio::test]
    async fn pause_resume_and_cancel() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let m = manager();
        let id = m
            .enqueue(JobRequest {
                kind: JobKind::Copy,
                sources: vec![loc("local", &dir.path().join("src"))],
                dest: Some(Loc { fs: "mem".into(), path: "/".into() }),
                conflict: Conflict::KeepBoth,
                open_after: false,
            })
            .unwrap();
        m.pause(&id, true).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let s = m.snapshots().into_iter().find(|s| s.id == id).unwrap();
        assert!(!s.status.is_final());
        m.cancel(&id).unwrap();
        let s = wait(&m, &id).await;
        assert_eq!(s.status, Status::Cancelled);
        // retry runs it to completion
        let id2 = m.retry(&id).unwrap();
        assert_eq!(wait(&m, &id2).await.status, Status::Done);
    }

    #[tokio::test]
    async fn delete_job_removes_everything() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let m = manager();
        let s = wait(
            &m,
            &m.enqueue(JobRequest {
                kind: JobKind::Delete,
                sources: vec![loc("local", &dir.path().join("src"))],
                dest: None,
                conflict: Conflict::KeepBoth,
                open_after: false,
            })
            .unwrap(),
        )
        .await;
        assert_eq!(s.status, Status::Done);
        assert!(!dir.path().join("src").exists());
    }

    async fn mem_with(files: &[(&str, &[u8])]) -> (Arc<Manager>, DynProvider) {
        use tokio::io::AsyncWriteExt;
        let mem: DynProvider =
            Arc::new(vfs::dal::DalFs::new(opendal::Operator::new(opendal::services::Memory::default()).unwrap().finish(), true));
        for (path, data) in files {
            let mut w = mem.writer(path).await.unwrap();
            w.write_all(data).await.unwrap();
            w.shutdown().await.unwrap();
        }
        let local: DynProvider = Arc::new(LocalFs);
        let m2 = mem.clone();
        let resolve: Resolver = Arc::new(move |fs: String| {
            let p = if fs == "mem" { m2.clone() } else { local.clone() };
            Box::pin(async move { Ok(p) })
        });
        (Manager::new(2, resolve, Arc::new(|| {}), Arc::new(|_| {})), mem)
    }

    #[tokio::test]
    async fn hostile_remote_names_cannot_escape_the_destination() {
        let (m, _mem) = mem_with(&[("/pkg/ok.txt", b"ok"), ("/pkg/..\\..\\evil.txt", b"evil"), ("/pkg/a:b.txt", b"ads")]).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        std::fs::create_dir(&dest).unwrap();
        let s = wait(
            &m,
            &m.enqueue(JobRequest {
                kind: JobKind::Copy,
                sources: vec![Loc { fs: "mem".into(), path: "/pkg".into() }],
                dest: Some(loc("local", &dest)),
                conflict: Conflict::KeepBoth,
                open_after: false,
            })
            .unwrap(),
        )
        .await;
        assert_eq!(s.status, Status::Done, "{:?}", s.error);
        assert!(dest.join("pkg/ok.txt").exists());
        assert!(s.skipped >= 1);
        // Nothing was written outside dest/pkg.
        let outside: Vec<_> =
            walkdir::WalkDir::new(dir.path()).into_iter().flatten().filter(|e| e.path().to_string_lossy().contains("evil")).collect();
        assert!(outside.is_empty(), "{outside:?}");
        if cfg!(windows) {
            assert!(!dest.join("pkg").join("a").exists(), "':' must not create an alternate data stream");
        }
    }

    #[tokio::test]
    async fn names_that_fold_together_never_overwrite_each_other() {
        let (m, _mem) = mem_with(&[("/docs/README.md", b"upper"), ("/docs/readme.md", b"lower")]).await;
        let dir = tempfile::tempdir().unwrap();
        let s = wait(
            &m,
            &m.enqueue(JobRequest {
                kind: JobKind::Move,
                sources: vec![Loc { fs: "mem".into(), path: "/docs".into() }],
                dest: Some(loc("local", dir.path())),
                conflict: Conflict::Overwrite,
                open_after: false,
            })
            .unwrap(),
        )
        .await;
        assert_eq!(s.status, Status::Done, "{:?}", s.error);
        let mut contents: Vec<Vec<u8>> =
            std::fs::read_dir(dir.path().join("docs")).unwrap().flatten().map(|e| std::fs::read(e.path()).unwrap()).collect();
        contents.sort();
        assert_eq!(contents, vec![b"lower".to_vec(), b"upper".to_vec()], "both files must survive");
    }

    #[tokio::test]
    async fn replace_refuses_to_delete_a_folder_containing_the_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/report")).unwrap();
        std::fs::write(dir.path().join("a/report/report"), b"precious").unwrap();
        let m = manager();
        let s = wait(
            &m,
            &m.enqueue(JobRequest {
                kind: JobKind::Move,
                sources: vec![loc("local", &dir.path().join("a/report/report"))],
                dest: Some(loc("local", &dir.path().join("a"))),
                conflict: Conflict::Overwrite,
                open_after: false,
            })
            .unwrap(),
        )
        .await;
        assert_eq!(s.status, Status::Failed);
        assert_eq!(std::fs::read(dir.path().join("a/report/report")).unwrap(), b"precious");
    }

    #[tokio::test]
    async fn into_itself_is_caught_despite_trailing_separator() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let m = manager();
        let dest = format!("{}{}", dir.path().join("src").to_string_lossy(), std::path::MAIN_SEPARATOR);
        let s = wait(
            &m,
            &m.enqueue(JobRequest {
                kind: JobKind::Copy,
                sources: vec![loc("local", &dir.path().join("src"))],
                dest: Some(Loc { fs: "local".into(), path: dest }),
                conflict: Conflict::KeepBoth,
                open_after: false,
            })
            .unwrap(),
        )
        .await;
        assert_eq!(s.status, Status::Failed);
        assert!(!dir.path().join("src/src").exists());
    }

    #[tokio::test]
    async fn replace_is_atomic_and_leaves_no_staging_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.bin"), vec![9u8; 2 * 1024 * 1024]).unwrap();
        std::fs::create_dir(dir.path().join("d")).unwrap();
        std::fs::write(dir.path().join("d/f.bin"), b"old").unwrap();
        let m = manager();
        let s = wait(
            &m,
            &m.enqueue(JobRequest {
                kind: JobKind::Copy,
                sources: vec![loc("local", &dir.path().join("f.bin"))],
                dest: Some(loc("local", &dir.path().join("d"))),
                conflict: Conflict::Overwrite,
                open_after: false,
            })
            .unwrap(),
        )
        .await;
        assert_eq!(s.status, Status::Done, "{:?}", s.error);
        assert_eq!(std::fs::read(dir.path().join("d/f.bin")).unwrap().len(), 2 * 1024 * 1024);
        let names: Vec<String> =
            std::fs::read_dir(dir.path().join("d")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["f.bin".to_string()]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn folder_links_inside_a_tree_are_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        std::os::unix::fs::symlink(dir.path().join("src/nested"), dir.path().join("src/lib64")).unwrap();
        std::fs::create_dir(dir.path().join("dst")).unwrap();
        let m = manager();
        let s = wait(
            &m,
            &m.enqueue(JobRequest {
                kind: JobKind::Move,
                sources: vec![loc("local", &dir.path().join("src"))],
                dest: Some(Loc { fs: "mem".into(), path: "/".into() }),
                conflict: Conflict::KeepBoth,
                open_after: false,
            })
            .unwrap(),
        )
        .await;
        assert_eq!(s.status, Status::Done, "{:?}", s.error);
        assert_eq!(s.skipped, 1);
        // Something was skipped, so the move keeps the source.
        assert!(dir.path().join("src/a.txt").exists());
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name("/a/b/"), "b");
        assert_eq!(display_name("C:\\x\\y.txt"), "y.txt");
    }
}
