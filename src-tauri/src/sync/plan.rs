//! Sync planning: compares the two sides (and, for two-way syncs, the state recorded
//! after the last successful run) and decides what to copy, delete or flag as a
//! conflict. Pure: no I/O, so every rule is unit-tested.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Timestamps closer than this are "the same" (FAT/FTP/S3 store coarse mtimes).
pub const MTIME_TOLERANCE_MS: i64 = 2_000;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// The destination becomes an exact replica: deletions are replicated too.
    Mirror,
    /// Everything is copied, nothing is ever deleted.
    KeepDeleted,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Side {
    Source,
    Dest,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Source => Side::Dest,
            Side::Dest => Side::Source,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub is_dir: bool,
    pub size: u64,
    /// Unix ms (0 when unknown).
    pub mtime: i64,
}

impl Meta {
    pub fn file(size: u64, mtime: i64) -> Self {
        Meta { is_dir: false, size, mtime }
    }
    pub fn dir() -> Self {
        Meta { is_dir: true, size: 0, mtime: 0 }
    }
}

/// Relative path ("a/b/c.txt", always `/`) → metadata, for one side.
pub type Tree = BTreeMap<String, Meta>;

/// Both sides as they were right after the last successful run (two-way syncs).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Baseline {
    pub source: Tree,
    pub dest: Tree,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "op")]
pub enum Op {
    /// Create folder `rel` on `to`.
    MkDir { to: Side, rel: String },
    /// Copy file `rel` from the other side onto `to` (creating or replacing).
    Copy { to: Side, rel: String, size: u64 },
    /// Remove `rel` (file or whole folder) from `on`.
    Delete { on: Side, rel: String },
    /// Both sides changed `rel`: `winner` overwrites, the loser is kept as a
    /// "(conflicted copy)" next to it on both sides.
    Conflict { rel: String, winner: Side, size: u64 },
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub copies: usize,
    pub copy_bytes: u64,
    pub to_source: usize,
    pub deletes: usize,
    /// Files that the deletions remove (folders count their contents).
    pub deleted_files: usize,
    pub conflicts: usize,
    pub mkdirs: usize,
}

/// Same content as far as metadata can tell.
fn same(a: Option<&Meta>, b: Option<&Meta>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.is_dir == b.is_dir && (a.is_dir || (a.size == b.size && (a.mtime - b.mtime).abs() <= MTIME_TOLERANCE_MS)),
        _ => false,
    }
}

fn under(prefix: &str, rel: &str) -> bool {
    rel.len() > prefix.len() && rel.starts_with(prefix) && rel.as_bytes()[prefix.len()] == b'/'
}

fn tree(side: Side, src: &Tree, dst: &Tree) -> Tree {
    match side {
        Side::Source => src.clone(),
        Side::Dest => dst.clone(),
    }
}

/// One-way: the destination follows the source.
fn plan_one_way(mode: Mode, src: &Tree, dst: &Tree) -> Vec<Op> {
    let mut ops = Vec::new();
    for (rel, s) in src {
        let d = dst.get(rel);
        if s.is_dir {
            match d {
                Some(d) if d.is_dir => {}
                Some(_) => {
                    ops.push(Op::Delete { on: Side::Dest, rel: rel.clone() });
                    ops.push(Op::MkDir { to: Side::Dest, rel: rel.clone() });
                }
                None => ops.push(Op::MkDir { to: Side::Dest, rel: rel.clone() }),
            }
        } else {
            match d {
                None => ops.push(Op::Copy { to: Side::Dest, rel: rel.clone(), size: s.size }),
                Some(d) if d.is_dir => {
                    ops.push(Op::Delete { on: Side::Dest, rel: rel.clone() });
                    ops.push(Op::Copy { to: Side::Dest, rel: rel.clone(), size: s.size });
                }
                // Copy when the content differs: other size, or the source is newer.
                Some(d) if d.size != s.size || s.mtime > d.mtime + MTIME_TOLERANCE_MS => {
                    ops.push(Op::Copy { to: Side::Dest, rel: rel.clone(), size: s.size })
                }
                Some(_) => {}
            }
        }
    }
    if mode == Mode::Mirror {
        let mut gone: Vec<&String> = Vec::new();
        for rel in dst.keys() {
            if src.contains_key(rel) || gone.iter().any(|g| under(g, rel)) {
                continue;
            }
            gone.push(rel);
            ops.push(Op::Delete { on: Side::Dest, rel: rel.clone() });
        }
    }
    ops
}

fn conflict_winner(s: &Meta, d: &Meta) -> Side {
    if d.mtime > s.mtime { Side::Dest } else { Side::Source }
}

/// Two-way: whichever side changed since the baseline is propagated to the other.
fn plan_two_way(mode: Mode, src: &Tree, dst: &Tree, base: &Baseline) -> Vec<Op> {
    let mut ops = Vec::new();
    let all: BTreeSet<&String> = src.keys().chain(dst.keys()).collect();
    // Folders deleted as a whole on one side: skip everything below them.
    let mut deleted: Vec<(Side, String)> = Vec::new();
    for rel in all {
        if deleted.iter().any(|(_, p)| under(p, rel)) {
            continue;
        }
        let (s, d) = (src.get(rel), dst.get(rel));
        let (bs, bd) = (base.source.get(rel), base.dest.get(rel));
        if same(s, d) && s.is_some() {
            continue; // already identical
        }
        let s_changed = !same(s, bs);
        let d_changed = !same(d, bd);
        let push_to = |ops: &mut Vec<Op>, to: Side, m: &Meta| {
            ops.push(if m.is_dir { Op::MkDir { to, rel: rel.clone() } } else { Op::Copy { to, rel: rel.clone(), size: m.size } });
        };
        match (s_changed, d_changed) {
            (false, false) => {} // e.g. kept-deleted files: stay as they are
            (true, false) | (false, true) => {
                let from = if s_changed { Side::Source } else { Side::Dest };
                let (now, there) = if s_changed { (s, d) } else { (d, s) };
                match now {
                    Some(m) => {
                        if let Some(t) = there {
                            if t.is_dir != m.is_dir {
                                ops.push(Op::Delete { on: from.other(), rel: rel.clone() });
                                deleted.push((from.other(), rel.clone()));
                            }
                        }
                        push_to(&mut ops, from.other(), m);
                    }
                    // Deleted on `from`: replicate only in mirror mode.
                    None if mode == Mode::Mirror && there.is_some() => {
                        ops.push(Op::Delete { on: from.other(), rel: rel.clone() });
                        deleted.push((from.other(), rel.clone()));
                    }
                    None => {}
                }
            }
            (true, true) => match (s, d) {
                (None, None) => {}
                // Changed on one side, deleted on the other: the change wins.
                (Some(m), None) => push_to(&mut ops, Side::Dest, m),
                (None, Some(m)) => push_to(&mut ops, Side::Source, m),
                (Some(a), Some(b)) if a.is_dir && b.is_dir => {}
                (Some(a), Some(b)) => {
                    let winner = conflict_winner(a, b);
                    let size = if winner == Side::Source { a.size } else { b.size };
                    ops.push(Op::Conflict { rel: rel.clone(), winner, size });
                }
            },
        }
    }
    ops
}

pub fn plan(mode: Mode, two_way: bool, src: &Tree, dst: &Tree, base: &Baseline) -> Vec<Op> {
    let mut ops = if two_way { plan_two_way(mode, src, dst, base) } else { plan_one_way(mode, src, dst) };
    // Execution order: folders first (parents before children), then files, then
    // conflicts, then deletions (nothing is removed before new data is in place).
    let rank = |o: &Op| match o {
        Op::MkDir { .. } => 0,
        Op::Copy { .. } => 1,
        Op::Conflict { .. } => 2,
        Op::Delete { .. } => 3,
    };
    // Type swaps (file <-> folder) must delete before recreating: keep those in place.
    let swaps: BTreeSet<String> = ops
        .windows(2)
        .filter_map(|w| match (&w[0], &w[1]) {
            (Op::Delete { rel: a, .. }, Op::MkDir { rel: b, .. } | Op::Copy { rel: b, .. }) if a == b => Some(a.clone()),
            _ => None,
        })
        .collect();
    ops.sort_by_key(|o| match o {
        Op::Delete { rel, .. } if swaps.contains(rel) => -1,
        o => rank(o),
    });
    ops
}

pub fn summarize(ops: &[Op], src: &Tree, dst: &Tree) -> Summary {
    let mut s = Summary::default();
    for o in ops {
        match o {
            Op::MkDir { .. } => s.mkdirs += 1,
            Op::Copy { to, size, .. } => {
                s.copies += 1;
                s.copy_bytes += size;
                if *to == Side::Source {
                    s.to_source += 1;
                }
            }
            Op::Conflict { size, .. } => {
                s.conflicts += 1;
                s.copy_bytes += size;
            }
            Op::Delete { on, rel } => {
                s.deletes += 1;
                let t = tree(*on, src, dst);
                let files = t.iter().filter(|(r, m)| !m.is_dir && (*r == rel || under(rel, r))).count();
                s.deleted_files += files.max(1);
            }
        }
    }
    s
}

/// A run that would remove a large share of what's there is held for review: the
/// classic cause is a source that is temporarily empty (unmounted drive, expired login).
pub fn needs_review(summary: &Summary, src: &Tree, dst: &Tree) -> bool {
    let existing = src.values().chain(dst.values()).filter(|m| !m.is_dir).count();
    summary.deleted_files > 20 && summary.deleted_files * 10 > existing * 3
}

/// "report.pdf" -> "report (conflicted copy 2026-10-21 14-03).pdf"
pub fn conflict_name(rel: &str, stamp: &str) -> String {
    let (dir, name) = match rel.rsplit_once('/') {
        Some((d, n)) => (format!("{d}/"), n),
        None => (String::new(), rel),
    };
    let tag = format!(" (conflicted copy {stamp})");
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{dir}{stem}{tag}.{ext}"),
        _ => format!("{dir}{name}{tag}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(entries: &[(&str, Option<(u64, i64)>)]) -> Tree {
        entries
            .iter()
            .map(|(p, m)| {
                (
                    p.to_string(),
                    match m {
                        Some((s, t)) => Meta::file(*s, *t),
                        None => Meta::dir(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn mirror_copies_new_and_changed_and_deletes_extras() {
        let src = t(&[("a.txt", Some((1, 1000))), ("docs", None), ("docs/b.txt", Some((5, 1000))), ("same.txt", Some((3, 50)))]);
        let dst = t(&[("a.txt", Some((2, 900))), ("old", None), ("old/x.txt", Some((1, 1))), ("same.txt", Some((3, 50)))]);
        let ops = plan(Mode::Mirror, false, &src, &dst, &Baseline::default());
        assert_eq!(
            ops,
            vec![
                Op::MkDir { to: Side::Dest, rel: "docs".into() },
                Op::Copy { to: Side::Dest, rel: "a.txt".into(), size: 1 },
                Op::Copy { to: Side::Dest, rel: "docs/b.txt".into(), size: 5 },
                // The folder goes as one deletion, its contents are not listed again.
                Op::Delete { on: Side::Dest, rel: "old".into() },
            ]
        );
        let s = summarize(&ops, &src, &dst);
        assert_eq!((s.copies, s.deletes, s.deleted_files), (2, 1, 1));
    }

    #[test]
    fn keep_deleted_never_deletes() {
        let src = t(&[("a.txt", Some((1, 1)))]);
        let dst = t(&[("a.txt", Some((1, 1))), ("gone-from-source.txt", Some((9, 9)))]);
        assert!(plan(Mode::KeepDeleted, false, &src, &dst, &Baseline::default()).is_empty());
    }

    #[test]
    fn coarse_remote_timestamps_do_not_cause_recopies() {
        let src = t(&[("a", Some((10, 1_000_000)))]);
        let dst = t(&[("a", Some((10, 1_000_900)))]); // within tolerance
        assert!(plan(Mode::Mirror, false, &src, &dst, &Baseline::default()).is_empty());
        // Destination written later (an upload) is not considered out of date either.
        let dst = t(&[("a", Some((10, 9_000_000)))]);
        assert!(plan(Mode::Mirror, false, &src, &dst, &Baseline::default()).is_empty());
    }

    #[test]
    fn file_replaced_by_folder_deletes_first() {
        let src = t(&[("x", None), ("x/in.txt", Some((1, 1)))]);
        let dst = t(&[("x", Some((4, 4)))]);
        let ops = plan(Mode::KeepDeleted, false, &src, &dst, &Baseline::default());
        assert_eq!(ops[0], Op::Delete { on: Side::Dest, rel: "x".into() });
        assert_eq!(ops[1], Op::MkDir { to: Side::Dest, rel: "x".into() });
    }

    fn synced(tree: &Tree) -> Baseline {
        Baseline { source: tree.clone(), dest: tree.clone() }
    }

    #[test]
    fn two_way_propagates_each_side_and_deletions_in_mirror() {
        let base = synced(&t(&[("a", Some((1, 1))), ("b", Some((1, 1))), ("c", Some((1, 1)))]));
        // Source edited a, destination edited b, destination deleted c, source added n.
        let src = t(&[("a", Some((2, 5000))), ("b", Some((1, 1))), ("c", Some((1, 1))), ("n", Some((7, 7)))]);
        let dst = t(&[("a", Some((1, 1))), ("b", Some((3, 6000)))]);
        let ops = plan(Mode::Mirror, true, &src, &dst, &base);
        assert!(ops.contains(&Op::Copy { to: Side::Dest, rel: "a".into(), size: 2 }));
        assert!(ops.contains(&Op::Copy { to: Side::Source, rel: "b".into(), size: 3 }));
        assert!(ops.contains(&Op::Delete { on: Side::Source, rel: "c".into() }));
        assert!(ops.contains(&Op::Copy { to: Side::Dest, rel: "n".into(), size: 7 }));
    }

    #[test]
    fn two_way_keep_deleted_never_resurrects_or_deletes() {
        let base = synced(&t(&[("a", Some((1, 1)))]));
        // Deleted at the source: the destination keeps it…
        let src = Tree::new();
        let dst = t(&[("a", Some((1, 1)))]);
        assert!(plan(Mode::KeepDeleted, true, &src, &dst, &base).is_empty());
        // …and with the baseline recording that, later runs don't copy it back.
        let after = Baseline { source: Tree::new(), dest: dst.clone() };
        assert!(plan(Mode::KeepDeleted, true, &src, &dst, &after).is_empty());
    }

    #[test]
    fn two_way_conflicts_pick_the_newest_and_edits_beat_deletes() {
        let base = synced(&t(&[("doc", Some((1, 1))), ("x", Some((1, 1)))]));
        let src = t(&[("doc", Some((2, 9000)))]); // x deleted here
        let dst = t(&[("doc", Some((3, 7000))), ("x", Some((5, 8000)))]); // x edited there
        let ops = plan(Mode::Mirror, true, &src, &dst, &base);
        assert!(ops.contains(&Op::Conflict { rel: "doc".into(), winner: Side::Source, size: 2 }));
        assert!(ops.contains(&Op::Copy { to: Side::Source, rel: "x".into(), size: 5 }));
    }

    #[test]
    fn first_two_way_run_is_a_union() {
        let src = t(&[("only-src", Some((1, 1))), ("both", Some((2, 2)))]);
        let dst = t(&[("only-dst", Some((3, 3))), ("both", Some((2, 2)))]);
        let ops = plan(Mode::Mirror, true, &src, &dst, &Baseline::default());
        assert_eq!(ops.len(), 2);
        assert!(ops.contains(&Op::Copy { to: Side::Dest, rel: "only-src".into(), size: 1 }));
        assert!(ops.contains(&Op::Copy { to: Side::Source, rel: "only-dst".into(), size: 3 }));
    }

    #[test]
    fn mass_deletion_is_held_for_review() {
        let dst: Tree = (0..100).map(|i| (format!("f{i}"), Meta::file(1, 1))).collect();
        let ops = plan(Mode::Mirror, false, &Tree::new(), &dst, &Baseline::default());
        let s = summarize(&ops, &Tree::new(), &dst);
        assert!(needs_review(&s, &Tree::new(), &dst));
        // A handful of deletions in a big tree is just normal work.
        let src: Tree = (0..95).map(|i| (format!("f{i}"), Meta::file(1, 1))).collect();
        let ops = plan(Mode::Mirror, false, &src, &dst, &Baseline::default());
        assert!(!needs_review(&summarize(&ops, &src, &dst), &src, &dst));
    }

    #[test]
    fn conflict_names() {
        assert_eq!(conflict_name("a/report.pdf", "2026-10-21 14-03"), "a/report (conflicted copy 2026-10-21 14-03).pdf");
        assert_eq!(conflict_name("Makefile", "d"), "Makefile (conflicted copy d)");
    }
}
