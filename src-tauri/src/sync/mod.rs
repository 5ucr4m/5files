//! Folder sync: pairs of folders (on any drive) kept in step automatically.
//! Mirror or keep-deleted, one-way or two-way. Local folders are watched; anything
//! involving a remote drive is also re-checked periodically.

pub mod plan;

use crate::error::{AppError, AppResult};
use crate::transfer::{Loc, Notify, Resolver};
use crate::vfs::DynProvider;
use parking_lot::Mutex;
use plan::{Baseline, Meta, Mode, Op, Side, Summary, Tree};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Wait this long after the last change before syncing (lets bursts of writes settle).
const DEBOUNCE: Duration = Duration::from_secs(2);
const POLL_LOCAL: Duration = Duration::from_secs(600);
const POLL_REMOTE: Duration = Duration::from_secs(300);
const PARALLEL_RUNS: usize = 2;
/// Never synced: OS clutter and our own in-flight temp files.
const IGNORED: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini", ".localized", "Icon\r"];

fn ignored(name: &str) -> bool {
    IGNORED.contains(&name) || name.starts_with("~$") || name.starts_with(".~lock.") || name.contains(".5files-part-")
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    pub at: i64,
    pub ok: bool,
    pub copied: usize,
    pub deleted: usize,
    pub conflicts: usize,
    pub bytes: u64,
    /// Operations that failed (the rest of the run still went through).
    pub failed: usize,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPair {
    pub id: String,
    pub name: String,
    pub source: Loc,
    pub dest: Loc,
    pub mode: Mode,
    pub two_way: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub last_run: Option<RunResult>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncInput {
    pub id: Option<String>,
    pub name: String,
    pub source: Loc,
    pub dest: Loc,
    pub mode: Mode,
    pub two_way: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum State {
    Idle,
    Scanning,
    Syncing {
        done: usize,
        total: usize,
        bytes: u64,
        total_bytes: u64,
        current: Option<String>,
    },
    /// Held: this run would delete a lot. Waits for the user to confirm.
    Review {
        summary: Summary,
    },
    Error {
        message: String,
    },
    Paused,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncView {
    #[serde(flatten)]
    pub pair: SyncPair,
    #[serde(flatten)]
    pub state: State,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub summary: Summary,
    pub needs_review: bool,
    /// First operations, for "what will happen" lists.
    pub sample: Vec<Op>,
}

pub struct Engine {
    file: PathBuf,
    baselines: PathBuf,
    pairs: Mutex<Vec<SyncPair>>,
    states: Mutex<HashMap<String, State>>,
    running: Mutex<HashSet<String>>,
    dirty: Mutex<HashMap<String, Instant>>,
    last_check: Mutex<HashMap<String, Instant>>,
    allow_deletes: Mutex<HashSet<String>>,
    watchers: Mutex<HashMap<String, Vec<notify::RecommendedWatcher>>>,
    resolve: Resolver,
    notify: Notify,
    slots: Arc<tokio::sync::Semaphore>,
    /// Local deletions go to the OS trash (off in tests, to keep the real Trash clean).
    use_trash: std::sync::atomic::AtomicBool,
}

// ---------------------------------------------------------------------------------
// Scanning & paths
// ---------------------------------------------------------------------------------

fn abs(p: &DynProvider, root: &str, rel: &str) -> String {
    rel.split('/').filter(|s| !s.is_empty()).fold(root.to_string(), |acc, seg| p.join(&acc, seg))
}

/// Recursive listing below `root` as relative `/`-separated paths.
pub async fn scan(p: &DynProvider, root: &str) -> AppResult<Tree> {
    let top = p.stat(root).await.map_err(|e| match e {
        AppError::NotFound(_) => AppError::NotFound(format!("folder is missing: {root}")),
        e => e,
    })?;
    if !top.is_dir {
        return Err(AppError::Invalid(format!("not a folder: {root}")));
    }
    let mut out = Tree::new();
    let mut stack = vec![(root.to_string(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        for e in p.list(&dir).await? {
            if ignored(&e.name) {
                continue;
            }
            let r = if rel.is_empty() { e.name.clone() } else { format!("{rel}/{}", e.name) };
            if e.is_dir {
                // Folder links are not followed (they could loop or point outside).
                if e.is_symlink {
                    continue;
                }
                out.insert(r.clone(), Meta::dir());
                stack.push((e.path, r));
            } else {
                out.insert(r, Meta::file(e.size, e.modified.unwrap_or(0)));
            }
        }
    }
    Ok(out)
}

/// Copies one file, replacing any existing one atomically where the drive supports
/// renames. Local copies keep the source's modification time.
async fn copy_one(from: &DynProvider, from_path: &str, to: &DynProvider, to_path: &str, mtime: i64) -> AppResult<u64> {
    let exists = to.exists(to_path).await?;
    let staged = exists && to.caps().rename;
    let write_to = if staged {
        let parent = to.parent(to_path).unwrap_or_else(|| to.root());
        to.join(&parent, &format!(".{}.5files-part-{}", to.file_name(to_path), &uuid::Uuid::new_v4().simple().to_string()[..8]))
    } else {
        to_path.to_string()
    };
    let result: AppResult<u64> = async {
        let mut r = from.reader(from_path).await?;
        let mut w = to.writer(&write_to).await?;
        let mut buf = vec![0u8; 1024 * 1024];
        let mut total = 0u64;
        loop {
            let n = r.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n]).await?;
            total += n as u64;
        }
        w.shutdown().await?;
        Ok(total)
    }
    .await;
    let bytes = match result {
        Ok(b) => b,
        Err(e) => {
            let _ = to.remove(&write_to).await;
            return Err(e);
        }
    };
    if staged {
        to.remove(to_path).await?;
        to.rename(&write_to, to_path).await?;
    }
    if !to.is_remote() && mtime > 0 {
        let path = to_path.to_string();
        let _ = tokio::task::spawn_blocking(move || {
            let t = std::time::UNIX_EPOCH + Duration::from_millis(mtime as u64);
            std::fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(t))
        })
        .await;
    }
    Ok(bytes)
}

/// Deletions on this computer go to the Trash / Recycle Bin, so a sync can't lose data
/// for good; remote drives have no trash.
async fn delete_one(p: &DynProvider, path: &str, use_trash: bool) -> AppResult<()> {
    if use_trash && !p.is_remote() {
        let owned = path.to_string();
        let trashed = tokio::task::spawn_blocking(move || trash::delete(&owned)).await;
        if matches!(trashed, Ok(Ok(()))) {
            return Ok(());
        }
    }
    p.remove(path).await
}

// ---------------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------------

impl Engine {
    pub fn new(dir: PathBuf, resolve: Resolver, notify: Notify) -> Arc<Self> {
        let file = dir.join("syncs.json");
        let pairs: Vec<SyncPair> = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Arc::new(Self {
            file,
            baselines: dir.join("sync"),
            pairs: Mutex::new(pairs),
            states: Default::default(),
            running: Default::default(),
            dirty: Default::default(),
            last_check: Default::default(),
            allow_deletes: Default::default(),
            watchers: Default::default(),
            resolve,
            notify,
            slots: Arc::new(tokio::sync::Semaphore::new(PARALLEL_RUNS)),
            use_trash: std::sync::atomic::AtomicBool::new(true),
        })
    }

    fn persist(&self) -> AppResult<()> {
        let data = serde_json::to_vec_pretty(&*self.pairs.lock())?;
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, &self.file)?;
        Ok(())
    }

    fn baseline_path(&self, id: &str) -> PathBuf {
        self.baselines.join(format!("{id}.json"))
    }

    fn load_baseline(&self, id: &str) -> Baseline {
        std::fs::read(self.baseline_path(id)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save_baseline(&self, id: &str, b: &Baseline) -> AppResult<()> {
        std::fs::create_dir_all(&self.baselines)?;
        std::fs::write(self.baseline_path(id), serde_json::to_vec(b)?)?;
        Ok(())
    }

    fn set_state(&self, id: &str, s: State) {
        self.states.lock().insert(id.to_string(), s);
        (self.notify)();
    }

    /// "Sync all now" (skips paused pairs).
    pub fn run_all(&self) {
        let ids: Vec<String> = self.pairs.lock().iter().filter(|p| !p.paused).map(|p| p.id.clone()).collect();
        for id in ids {
            self.states.lock().insert(id.clone(), State::Idle);
            self.trigger(&id);
        }
        (self.notify)();
    }

    pub fn all_paused(&self) -> bool {
        let pairs = self.pairs.lock();
        !pairs.is_empty() && pairs.iter().all(|p| p.paused)
    }

    /// Pauses or resumes every sync.
    pub fn pause_all(&self, paused: bool) {
        let ids: Vec<String> = {
            let mut pairs = self.pairs.lock();
            pairs.iter_mut().for_each(|p| p.paused = paused);
            pairs.iter().map(|p| p.id.clone()).collect()
        };
        let _ = self.persist();
        if !paused {
            for id in &ids {
                self.states.lock().insert(id.clone(), State::Idle);
                self.trigger(id);
            }
        }
        (self.notify)();
    }

    /// Whether a sync is running right now on a folder of `fs` matched by `on`.
    pub fn busy_on(&self, fs: &str, on: &dyn Fn(&str) -> bool) -> bool {
        let running = self.running.lock();
        self.pairs
            .lock()
            .iter()
            .any(|p| running.contains(&p.id) && ((p.source.fs == fs && on(&p.source.path)) || (p.dest.fs == fs && on(&p.dest.path))))
    }

    pub fn get(&self, id: &str) -> Option<SyncPair> {
        self.pairs.lock().iter().find(|p| p.id == id).cloned()
    }

    pub fn views(&self) -> Vec<SyncView> {
        let states = self.states.lock();
        self.pairs
            .lock()
            .iter()
            .map(|p| SyncView {
                state: if p.paused { State::Paused } else { states.get(&p.id).cloned().unwrap_or(State::Idle) },
                pair: p.clone(),
            })
            .collect()
    }

    /// Checks that both folders are usable and don't contain each other.
    async fn validate(&self, input: &SyncInput) -> AppResult<(DynProvider, DynProvider)> {
        if input.name.trim().is_empty() {
            return Err(AppError::Invalid("give this sync a name".into()));
        }
        let src = (self.resolve)(input.source.fs.clone()).await?;
        let dst = (self.resolve)(input.dest.fs.clone()).await?;
        if !src.stat(&input.source.path).await?.is_dir {
            return Err(AppError::Invalid("the source must be a folder".into()));
        }
        if !dst.exists(&input.dest.path).await? {
            dst.mkdir(&input.dest.path).await?;
        } else if !dst.stat(&input.dest.path).await?.is_dir {
            return Err(AppError::Invalid("the destination must be a folder".into()));
        }
        if input.source.fs == input.dest.fs
            && (src.contains(&input.source.path, &input.dest.path).await || src.contains(&input.dest.path, &input.source.path).await)
        {
            return Err(AppError::Invalid("the two folders can't be the same or inside each other".into()));
        }
        Ok((src, dst))
    }

    pub async fn preview(&self, input: &SyncInput) -> AppResult<Preview> {
        let (src_p, dst_p) = self.validate(input).await?;
        let (src, dst) = tokio::try_join!(scan(&src_p, &input.source.path), scan(&dst_p, &input.dest.path))?;
        let base = match &input.id {
            Some(id) if input.two_way => self.load_baseline(id),
            _ => Baseline::default(),
        };
        let ops = plan::plan(input.mode, input.two_way, &src, &dst, &base);
        let summary = plan::summarize(&ops, &src, &dst);
        Ok(Preview { needs_review: plan::needs_review(&summary, &src, &dst), sample: ops.into_iter().take(200).collect(), summary })
    }

    pub async fn save(self: &Arc<Self>, input: SyncInput) -> AppResult<SyncPair> {
        self.validate(&input).await?;
        let id = input.id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let prev = self.get(&id);
        // Changing what is synced invalidates the recorded state.
        if let Some(p) = &prev {
            if p.source.path != input.source.path
                || p.dest.path != input.dest.path
                || p.source.fs != input.source.fs
                || p.dest.fs != input.dest.fs
            {
                let _ = std::fs::remove_file(self.baseline_path(&id));
            }
        }
        let pair = SyncPair {
            id: id.clone(),
            name: input.name.trim().to_string(),
            source: input.source,
            dest: input.dest,
            mode: input.mode,
            two_way: input.two_way,
            paused: prev.as_ref().map(|p| p.paused).unwrap_or(false),
            created_at: prev.as_ref().map(|p| p.created_at).unwrap_or_else(now_ms),
            last_run: prev.and_then(|p| p.last_run),
        };
        {
            let mut pairs = self.pairs.lock();
            match pairs.iter_mut().find(|p| p.id == id) {
                Some(p) => *p = pair.clone(),
                None => pairs.push(pair.clone()),
            }
        }
        self.persist()?;
        self.rewatch(&pair).await;
        // The user just reviewed the preview: run now, deletions included.
        self.allow_deletes.lock().insert(id.clone());
        self.trigger(&id);
        (self.notify)();
        Ok(pair)
    }

    pub fn delete(&self, id: &str) -> AppResult<()> {
        self.pairs.lock().retain(|p| p.id != id);
        self.watchers.lock().remove(id);
        self.states.lock().remove(id);
        let _ = std::fs::remove_file(self.baseline_path(id));
        self.persist()?;
        (self.notify)();
        Ok(())
    }

    pub fn set_paused(self: &Arc<Self>, id: &str, paused: bool) -> AppResult<()> {
        if let Some(p) = self.pairs.lock().iter_mut().find(|p| p.id == id) {
            p.paused = paused;
        }
        self.persist()?;
        if !paused {
            self.states.lock().insert(id.into(), State::Idle);
            self.trigger(id);
        }
        (self.notify)();
        Ok(())
    }

    /// Runs as soon as possible. `allow_deletes` confirms a run held for review.
    pub fn run_now(self: &Arc<Self>, id: &str, allow_deletes: bool) {
        if allow_deletes {
            self.allow_deletes.lock().insert(id.to_string());
        }
        self.states.lock().insert(id.into(), State::Idle);
        self.trigger(id);
    }

    fn trigger(&self, id: &str) {
        // Backdate so the debounce has already elapsed.
        self.dirty.lock().insert(id.to_string(), Instant::now() - DEBOUNCE);
    }

    /// Watches the local side(s) of a pair for changes.
    async fn rewatch(self: &Arc<Self>, pair: &SyncPair) {
        use notify::{RecursiveMode, Watcher};
        let mut ws = Vec::new();
        let mut sides = vec![&pair.source];
        if pair.two_way {
            sides.push(&pair.dest);
        }
        for loc in sides {
            if loc.fs != "local" {
                continue;
            }
            let me = Arc::downgrade(self);
            let id = pair.id.clone();
            let w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let Ok(ev) = res else { return };
                if ev.paths.iter().all(|p| p.file_name().map(|n| ignored(&n.to_string_lossy())).unwrap_or(false)) {
                    return;
                }
                if let Some(me) = me.upgrade() {
                    me.dirty.lock().insert(id.clone(), Instant::now());
                }
            });
            if let Ok(mut w) = w {
                if w.watch(std::path::Path::new(&loc.path), RecursiveMode::Recursive).is_ok() {
                    ws.push(w);
                }
            }
        }
        self.watchers.lock().insert(pair.id.clone(), ws);
    }

    /// Starts watchers and the scheduler (call once at startup).
    pub fn start(self: &Arc<Self>) {
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            let pairs = me.pairs.lock().clone(); // don't hold the lock across awaits
            for p in pairs {
                me.rewatch(&p).await;
            }
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                me.schedule();
            }
        });
    }

    fn schedule(self: &Arc<Self>) {
        let pairs = self.pairs.lock().clone();
        for p in pairs {
            if p.paused || self.running.lock().contains(&p.id) {
                continue;
            }
            if matches!(self.states.lock().get(&p.id), Some(State::Review { .. })) && !self.allow_deletes.lock().contains(&p.id) {
                continue;
            }
            let changed = self.dirty.lock().get(&p.id).map(|t| t.elapsed() >= DEBOUNCE).unwrap_or(false);
            let every = if p.source.fs != "local" || p.dest.fs != "local" { POLL_REMOTE } else { POLL_LOCAL };
            let stale = self.last_check.lock().get(&p.id).map(|t| t.elapsed() >= every).unwrap_or(true);
            if changed || stale {
                self.dirty.lock().remove(&p.id);
                self.last_check.lock().insert(p.id.clone(), Instant::now());
                let me = self.clone();
                tauri::async_runtime::spawn(async move { me.run(p).await });
            }
        }
    }

    async fn run(self: Arc<Self>, pair: SyncPair) {
        if !self.running.lock().insert(pair.id.clone()) {
            return;
        }
        let _slot = self.slots.clone().acquire_owned().await;
        self.set_state(&pair.id, State::Scanning);
        let outcome = self.run_inner(&pair).await;
        self.running.lock().remove(&pair.id);
        let (state, result) = match outcome {
            Ok(Some(r)) => (if r.ok { State::Idle } else { State::Error { message: r.error.clone().unwrap_or_default() } }, Some(r)),
            Ok(None) => return, // held for review: state already set
            Err(e) => (
                State::Error { message: e.to_string() },
                Some(RunResult {
                    at: now_ms(),
                    ok: false,
                    copied: 0,
                    deleted: 0,
                    conflicts: 0,
                    bytes: 0,
                    failed: 0,
                    error: Some(e.to_string()),
                }),
            ),
        };
        if let Some(r) = result {
            if let Some(p) = self.pairs.lock().iter_mut().find(|p| p.id == pair.id) {
                p.last_run = Some(r);
            }
            let _ = self.persist();
        }
        self.set_state(&pair.id, state);
    }

    /// `Ok(None)` when the run was held for review.
    async fn run_inner(&self, pair: &SyncPair) -> AppResult<Option<RunResult>> {
        let src_p = (self.resolve)(pair.source.fs.clone()).await?;
        let dst_p = (self.resolve)(pair.dest.fs.clone()).await?;
        if !dst_p.exists(&pair.dest.path).await? {
            dst_p.mkdir(&pair.dest.path).await?;
        }
        // A missing source is an error, never "empty": that's how mirrors get wiped.
        let (src, dst) = tokio::try_join!(scan(&src_p, &pair.source.path), scan(&dst_p, &pair.dest.path))?;
        let base = if pair.two_way { self.load_baseline(&pair.id) } else { Baseline::default() };
        let ops = plan::plan(pair.mode, pair.two_way, &src, &dst, &base);
        let summary = plan::summarize(&ops, &src, &dst);
        let allowed = self.allow_deletes.lock().remove(&pair.id);
        if plan::needs_review(&summary, &src, &dst) && !allowed {
            self.set_state(&pair.id, State::Review { summary });
            return Ok(None);
        }

        let total = ops.len();
        let total_bytes = summary.copy_bytes;
        let (mut done, mut bytes, mut failed) = (0usize, 0u64, 0usize);
        let mut first_error: Option<String> = None;
        let (mut copied, mut deleted, mut conflicts) = (0usize, 0usize, 0usize);
        let side = |s: Side| -> (&DynProvider, &str, &Tree) {
            match s {
                Side::Source => (&src_p, pair.source.path.as_str(), &src),
                Side::Dest => (&dst_p, pair.dest.path.as_str(), &dst),
            }
        };
        let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M").to_string();
        for op in &ops {
            let current = match op {
                Op::MkDir { rel, .. } | Op::Copy { rel, .. } | Op::Delete { rel, .. } | Op::Conflict { rel, .. } => rel.clone(),
            };
            self.set_state(&pair.id, State::Syncing { done, total, bytes, total_bytes, current: Some(current) });
            let r: AppResult<u64> = async {
                match op {
                    Op::MkDir { to, rel } => {
                        let (p, root, _) = side(*to);
                        p.mkdir(&abs(p, root, rel)).await?;
                        Ok(0)
                    }
                    Op::Copy { to, rel, .. } => {
                        let (fp, froot, ftree) = side(to.other());
                        let (tp, troot, _) = side(*to);
                        if let Some(parent) = rel.rsplit_once('/').map(|(d, _)| d) {
                            tp.mkdir(&abs(tp, troot, parent)).await?;
                        }
                        let mtime = ftree.get(rel).map(|m| m.mtime).unwrap_or(0);
                        let n = copy_one(fp, &abs(fp, froot, rel), tp, &abs(tp, troot, rel), mtime).await?;
                        copied += 1;
                        Ok(n)
                    }
                    Op::Delete { on, rel } => {
                        let (p, root, _) = side(*on);
                        delete_one(p, &abs(p, root, rel), self.use_trash.load(std::sync::atomic::Ordering::Relaxed)).await?;
                        deleted += 1;
                        Ok(0)
                    }
                    Op::Conflict { rel, winner, .. } => {
                        // Keep the losing version as "(conflicted copy …)" on both sides,
                        // then put the winner in place.
                        let loser = winner.other();
                        let (wp, wroot, wtree) = side(*winner);
                        let (lp, lroot, ltree) = side(loser);
                        let cname = plan::conflict_name(rel, &stamp);
                        let lmtime = ltree.get(rel).map(|m| m.mtime).unwrap_or(0);
                        copy_one(lp, &abs(lp, lroot, rel), lp, &abs(lp, lroot, &cname), lmtime).await?;
                        copy_one(lp, &abs(lp, lroot, &cname), wp, &abs(wp, wroot, &cname), lmtime).await?;
                        let wmtime = wtree.get(rel).map(|m| m.mtime).unwrap_or(0);
                        let n = copy_one(wp, &abs(wp, wroot, rel), lp, &abs(lp, lroot, rel), wmtime).await?;
                        conflicts += 1;
                        Ok(n)
                    }
                }
            }
            .await;
            match r {
                Ok(n) => bytes += n,
                Err(e) => {
                    failed += 1;
                    if first_error.is_none() {
                        first_error = Some(format!(
                            "{}: {e}",
                            match op {
                                Op::MkDir { rel, .. } | Op::Copy { rel, .. } | Op::Delete { rel, .. } | Op::Conflict { rel, .. } => rel,
                            }
                        ));
                    }
                }
            }
            done += 1;
        }
        if pair.two_way {
            // Record what both sides look like now: the next run compares against it.
            let (s, d) = tokio::try_join!(scan(&src_p, &pair.source.path), scan(&dst_p, &pair.dest.path))?;
            self.save_baseline(&pair.id, &Baseline { source: s, dest: d })?;
        }
        Ok(Some(RunResult {
            at: now_ms(),
            ok: failed == 0,
            copied,
            deleted,
            conflicts,
            bytes,
            failed,
            error: first_error.map(|e| if failed > 1 { format!("{e} (and {} more)", failed - 1) } else { e }),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::local::LocalFs;

    fn engine(dir: &std::path::Path) -> Arc<Engine> {
        let local: DynProvider = Arc::new(LocalFs);
        let resolve: Resolver = Arc::new(move |_fs: String| {
            let l = local.clone();
            Box::pin(async move { Ok(l) })
        });
        let e = Engine::new(dir.to_path_buf(), resolve, Arc::new(|| {}));
        e.use_trash.store(false, std::sync::atomic::Ordering::Relaxed);
        e
    }

    fn loc(p: &std::path::Path) -> Loc {
        Loc { fs: "local".into(), path: p.to_string_lossy().into_owned() }
    }

    async fn run_once(e: &Arc<Engine>, id: &str) {
        let p = e.get(id).unwrap();
        e.clone().run(p).await;
    }

    #[tokio::test]
    async fn mirror_end_to_end_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let (src, dst) = (tmp.path().join("src"), tmp.path().join("dst"));
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), b"hello").unwrap();
        std::fs::write(src.join("sub/b.bin"), vec![7u8; 3000]).unwrap();
        std::fs::write(src.join(".DS_Store"), b"junk").unwrap();
        let e = engine(&tmp.path().join("cfg"));
        std::fs::create_dir_all(tmp.path().join("cfg")).unwrap();
        let pair = e
            .save(SyncInput { id: None, name: "t".into(), source: loc(&src), dest: loc(&dst), mode: Mode::Mirror, two_way: false })
            .await
            .unwrap();
        run_once(&e, &pair.id).await;
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"hello");
        assert_eq!(std::fs::read(dst.join("sub/b.bin")).unwrap().len(), 3000);
        assert!(!dst.join(".DS_Store").exists(), "OS clutter is not synced");
        // Same mtime as the source, so the next run is a no-op.
        let (sm, dm) = (
            std::fs::metadata(src.join("a.txt")).unwrap().modified().unwrap(),
            std::fs::metadata(dst.join("a.txt")).unwrap().modified().unwrap(),
        );
        assert!(sm.duration_since(dm).unwrap_or_default() < Duration::from_secs(1));

        // Deleting at the source deletes at the destination (mirror).
        std::fs::remove_file(src.join("a.txt")).unwrap();
        run_once(&e, &pair.id).await;
        assert!(!dst.join("a.txt").exists());
        let r = e.get(&pair.id).unwrap().last_run.unwrap();
        assert!(r.ok && r.deleted == 1, "{r:?}");
    }

    #[tokio::test]
    async fn two_way_keep_deleted_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let (src, dst) = (tmp.path().join("src"), tmp.path().join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.txt"), b"from source").unwrap();
        std::fs::write(dst.join("b.txt"), b"from dest").unwrap();
        std::fs::create_dir_all(tmp.path().join("cfg")).unwrap();
        let e = engine(&tmp.path().join("cfg"));
        let pair = e
            .save(SyncInput { id: None, name: "t".into(), source: loc(&src), dest: loc(&dst), mode: Mode::KeepDeleted, two_way: true })
            .await
            .unwrap();
        run_once(&e, &pair.id).await;
        assert!(dst.join("a.txt").exists() && src.join("b.txt").exists(), "first run is a union");

        // Edit at the destination flows back to the source.
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(dst.join("a.txt"), b"edited at destination").unwrap();
        run_once(&e, &pair.id).await;
        assert_eq!(std::fs::read(src.join("a.txt")).unwrap(), b"edited at destination");

        // Deleting at the source keeps the destination copy, and it isn't copied back.
        std::fs::remove_file(src.join("b.txt")).unwrap();
        run_once(&e, &pair.id).await;
        run_once(&e, &pair.id).await;
        assert!(dst.join("b.txt").exists());
        assert!(!src.join("b.txt").exists());
    }

    #[tokio::test]
    async fn refuses_overlapping_folders_and_holds_mass_deletes() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("inner")).unwrap();
        std::fs::create_dir_all(tmp.path().join("cfg")).unwrap();
        let e = engine(&tmp.path().join("cfg"));
        let bad = e
            .save(SyncInput {
                id: None,
                name: "t".into(),
                source: loc(&src),
                dest: loc(&src.join("inner")),
                mode: Mode::Mirror,
                two_way: false,
            })
            .await;
        assert!(matches!(bad, Err(AppError::Invalid(_))));

        // A destination full of files and an (accidentally) empty source: held for review.
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(&dst).unwrap();
        for i in 0..40 {
            std::fs::write(dst.join(format!("keep{i}.txt")), b"x").unwrap();
        }
        let pair = SyncPair {
            id: "p".into(),
            name: "t".into(),
            source: loc(&src.join("inner")),
            dest: loc(&dst),
            mode: Mode::Mirror,
            two_way: false,
            paused: false,
            created_at: 0,
            last_run: None,
        };
        e.pairs.lock().push(pair.clone());
        e.clone().run(pair).await;
        assert!(matches!(e.views()[0].state, State::Review { .. }));
        assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 40, "nothing deleted while held");
    }
}
