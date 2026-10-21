//! Recursive search that streams results to the UI in batches and can be cancelled.

use crate::vfs::{DynProvider, Entry, local};
use serde::Serialize;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub const MAX_RESULTS: usize = 5000;
const BATCH: usize = 64;
const FLUSH_EVERY: Duration = Duration::from_millis(120);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum SearchEvent {
    Results { entries: Vec<Entry> },
    Progress { scanned: u64 },
    Done { total: usize, truncated: bool, cancelled: bool },
}

/// Case-insensitive matcher: `*`/`?` globs, otherwise every whitespace-separated
/// term must appear in the name.
pub struct Matcher {
    glob: Option<Vec<char>>,
    terms: Vec<String>,
}

impl Matcher {
    pub fn new(query: &str) -> Self {
        let q = query.trim().to_lowercase();
        if q.contains('*') || q.contains('?') {
            Matcher { glob: Some(q.chars().collect()), terms: vec![] }
        } else {
            Matcher { glob: None, terms: q.split_whitespace().map(String::from).collect() }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.glob.is_none() && self.terms.is_empty()
    }

    pub fn matches(&self, name: &str) -> bool {
        let n = name.to_lowercase();
        match &self.glob {
            Some(p) => glob_match(p, &n.chars().collect::<Vec<_>>()),
            None => self.terms.iter().all(|t| n.contains(t.as_str())),
        }
    }
}

fn glob_match(p: &[char], s: &[char]) -> bool {
    // Iterative wildcard matching with backtracking on the last '*'.
    let (mut pi, mut si, mut star, mut mark) = (0, 0, None, 0);
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(st) = star {
            pi = st + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

pub async fn run(
    provider: DynProvider,
    root: String,
    query: String,
    include_hidden: bool,
    cancel: CancellationToken,
    emit: impl Fn(SearchEvent) + Send + Sync + 'static,
) {
    let matcher = Matcher::new(&query);
    if matcher.is_empty() {
        emit(SearchEvent::Done { total: 0, truncated: false, cancelled: false });
        return;
    }
    if !provider.is_remote() {
        let emit = std::sync::Arc::new(emit);
        let e2 = emit.clone();
        let c2 = cancel.clone();
        let res = tokio::task::spawn_blocking(move || search_local(&root, &matcher, include_hidden, &c2, &*e2)).await;
        if res.is_err() {
            emit(SearchEvent::Done { total: 0, truncated: false, cancelled: cancel.is_cancelled() });
        }
        return;
    }

    // Remote: breadth-first over the provider API.
    let mut queue = std::collections::VecDeque::from([root]);
    let (mut found, mut batch, mut scanned, mut last) = (0usize, Vec::new(), 0u64, Instant::now());
    while let Some(dir) = queue.pop_front() {
        if cancel.is_cancelled() {
            break;
        }
        let Ok(entries) = provider.list(&dir).await else { continue };
        for e in entries {
            scanned += 1;
            if e.hidden && !include_hidden {
                continue;
            }
            if e.is_dir && !e.is_symlink {
                queue.push_back(e.path.clone());
            }
            if matcher.matches(&e.name) {
                found += 1;
                batch.push(e);
            }
        }
        if batch.len() >= BATCH || last.elapsed() >= FLUSH_EVERY {
            emit(SearchEvent::Results { entries: std::mem::take(&mut batch) });
            emit(SearchEvent::Progress { scanned });
            last = Instant::now();
        }
        if found >= MAX_RESULTS {
            break;
        }
    }
    if !batch.is_empty() {
        emit(SearchEvent::Results { entries: batch });
    }
    emit(SearchEvent::Done { total: found.min(MAX_RESULTS), truncated: found >= MAX_RESULTS, cancelled: cancel.is_cancelled() });
}

fn search_local(root: &str, m: &Matcher, include_hidden: bool, cancel: &CancellationToken, emit: &dyn Fn(SearchEvent)) {
    let (mut found, mut batch, mut scanned, mut last) = (0usize, Vec::new(), 0u64, Instant::now());
    let walker = walkdir::WalkDir::new(root)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| include_hidden || !e.file_name().to_string_lossy().starts_with('.'));
    for de in walker.flatten() {
        if cancel.is_cancelled() {
            break;
        }
        scanned += 1;
        if m.matches(&de.file_name().to_string_lossy())
            && let Ok(e) = local::entry_from_path(de.path())
            && (include_hidden || !e.hidden)
        {
            found += 1;
            batch.push(e);
        }
        if batch.len() >= BATCH || (last.elapsed() >= FLUSH_EVERY && (!batch.is_empty() || scanned % 512 == 0)) {
            if !batch.is_empty() {
                emit(SearchEvent::Results { entries: std::mem::take(&mut batch) });
            }
            emit(SearchEvent::Progress { scanned });
            last = Instant::now();
        }
        if found >= MAX_RESULTS {
            break;
        }
    }
    if !batch.is_empty() {
        emit(SearchEvent::Results { entries: batch });
    }
    emit(SearchEvent::Done { total: found.min(MAX_RESULTS), truncated: found >= MAX_RESULTS, cancelled: cancel.is_cancelled() });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn matcher_terms_and_globs() {
        assert!(Matcher::new("rep 2024").matches("Report_2024_final.pdf"));
        assert!(!Matcher::new("rep 2023").matches("Report_2024_final.pdf"));
        assert!(Matcher::new("*.RS").matches("main.rs"));
        assert!(!Matcher::new("*.rs").matches("main.rsx"));
        assert!(Matcher::new("img_??.jpg").matches("IMG_01.jpg"));
        assert!(Matcher::new("a*b*c").matches("axxbyyc"));
        assert!(!Matcher::new("a*b*c").matches("axxbyy"));
        assert!(Matcher::new("  ").is_empty());
    }

    #[tokio::test]
    async fn local_search_streams_results() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/b/needle.txt"), b"").unwrap();
        std::fs::write(dir.path().join("a/hay.txt"), b"").unwrap();
        std::fs::write(dir.path().join(".hidden-needle"), b"").unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        run(
            Arc::new(local::LocalFs),
            dir.path().to_string_lossy().into_owned(),
            "needle".into(),
            false,
            CancellationToken::new(),
            move |e| ev.lock().unwrap().push(e),
        )
        .await;
        let events = events.lock().unwrap();
        let names: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                SearchEvent::Results { entries } => Some(entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(names, vec!["needle.txt"]);
        assert!(matches!(events.last(), Some(SearchEvent::Done { total: 1, .. })));
    }
}
