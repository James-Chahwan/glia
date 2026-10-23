//! Co-change over the candidate file space (EA.3, item 2's data half):
//! rank_eval's `cnt` / `co` and its C0 signal, read at query time from each
//! repo's `.glia/history-snapshot/` (`glia history sync`'s window, newest
//! 2000 commits by default; ruling EA-1 keeps that window for answers).
//!
//! A commit's files map through [`FileIndex::index_of`] by `HistoryFile.p`
//! (the path at that commit) with NO rename fold: `from` is never read, so a
//! commit made under a file's old name is no commit of the file, exactly as
//! rank_eval's `git log --no-renames` mapped through `pidx`. A commit trains
//! iff it maps 1..=[`MAX_TRAIN_FILES`] candidate files (`train_ok`): each
//! file's count gains one, each ordered pair a != b one. Counts are
//! unfloored, unlike LF.5b's CO_CHANGES edges.
//!
//! [`CochangeHistory`] is the counting rule's read side, shared with EC's
//! leak-free replay; [`c0`] is `sum over seeds s of co(s, f) / (count(s) + 1)`.
//!
//! Fired_on, stderr (printed by `RankCache::cochange`): `[rank-cochange]
//! holder=<h> source=<snapshot|given> repos=<r> snapshots=<s> commits=<c>
//! train_commits=<t> files=<f> pairs=<p> ms=<m>`, or `[rank-cochange]
//! holder=<h> none reason=<no_roots|no_snapshot>`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use glia_code_domain::snapshots::read_history;
use serde::Serialize;

use super::files::FileIndex;

/// rank_eval's `MAX_FILES`: a commit of more candidate files trains nothing.
/// Equal to engine/src/external/history.rs `MAX_COMMIT_FILES`.
pub(crate) const MAX_TRAIN_FILES: usize = 30;

/// Per-file commit counts and per-pair co-change counts over a file space.
pub trait CochangeHistory {
    /// Training commits touching file `f`.
    fn count(&self, f: u32) -> u32;
    /// `(other file, commits touching both)` for every file that co-changed
    /// with `f`, ascending by other file.
    fn cochanged(&self, f: u32) -> &[(u32, u32)];
}

/// What a read saw.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CochangeStats {
    /// Repos with a root to read.
    pub repos: usize,
    /// Complete snapshots read.
    pub snapshots: usize,
    /// Repos with no complete snapshot.
    pub missing: usize,
    /// Commits in the snapshots read.
    pub commits: usize,
    /// Commits that trained (1..=30 candidate files).
    pub train_commits: usize,
    /// Files with a non-zero count.
    pub files: usize,
    /// Ordered pairs with a non-zero count.
    pub pairs: usize,
}

/// The co-change counts of one file space.
#[non_exhaustive]
#[derive(Clone, Debug, Default)]
pub struct CochangeIndex {
    count: Vec<u32>,
    pairs: Vec<Vec<(u32, u32)>>,
    stats: CochangeStats,
}

impl CochangeIndex {
    /// Read every repo's history snapshot (`roots`: repo id -> root, in key
    /// order) and count it over `files`.
    pub fn read(files: &FileIndex, repo_roots: &BTreeMap<u64, String>) -> CochangeIndex {
        let n = files.len();
        let mut count = vec![0u32; n];
        let mut pair: BTreeMap<(u32, u32), u32> = BTreeMap::new();
        let mut stats = CochangeStats { repos: repo_roots.len(), ..CochangeStats::default() };
        for (&repo, root) in repo_roots {
            let Some(snap) = read_history(Path::new(root)) else {
                stats.missing += 1;
                continue;
            };
            stats.snapshots += 1;
            stats.commits += snap.commits.len();
            for c in &snap.commits {
                let mapped: BTreeSet<u32> = c.files.iter().filter_map(|hf| files.index_of(repo, &hf.p)).collect();
                if mapped.is_empty() || mapped.len() > MAX_TRAIN_FILES {
                    continue;
                }
                stats.train_commits += 1;
                for &a in &mapped {
                    count[a as usize] += 1;
                    for &b in &mapped {
                        if a != b {
                            *pair.entry((a, b)).or_default() += 1;
                        }
                    }
                }
            }
        }
        let mut pairs: Vec<Vec<(u32, u32)>> = vec![Vec::new(); n];
        for ((a, b), x) in pair {
            pairs[a as usize].push((b, x));
        }
        let mut idx = CochangeIndex { count, pairs, stats };
        idx.recount();
        idx
    }

    /// The same index from counts a caller kept (EA.9's harness prefix, EC's
    /// replay): `count[f]` and per file its `(other, n)` pairs, sorted here.
    pub fn from_counts(count: Vec<u32>, mut pairs: Vec<Vec<(u32, u32)>>) -> CochangeIndex {
        if pairs.len() < count.len() {
            pairs.resize(count.len(), Vec::new());
        }
        for p in &mut pairs {
            p.sort_unstable();
        }
        let mut idx = CochangeIndex { count, pairs, stats: CochangeStats::default() };
        idx.recount();
        idx
    }

    fn recount(&mut self) {
        self.stats.files = self.count.iter().filter(|&&c| c > 0).count();
        self.stats.pairs = self.pairs.iter().map(Vec::len).sum();
    }

    pub fn len(&self) -> usize {
        self.count.len()
    }

    pub fn is_empty(&self) -> bool {
        self.count.is_empty()
    }

    pub fn stats(&self) -> CochangeStats {
        self.stats
    }
}

impl CochangeHistory for CochangeIndex {
    fn count(&self, f: u32) -> u32 {
        self.count.get(f as usize).copied().unwrap_or(0)
    }

    fn cochanged(&self, f: u32) -> &[(u32, u32)] {
        self.pairs.get(f as usize).map_or(&[], Vec::as_slice)
    }
}

/// C0 over `n` files: for each seed `s` in the given order, `co(s, f) /
/// (count(s) + 1)` added to `f`'s slot (rank_eval's `r3_tqueries` sum; a
/// caller wanting parity passes the harness's seed order). The seeds' own
/// slots keep what other seeds gave them.
pub fn c0(h: &dyn CochangeHistory, seeds: &[u32], n: usize) -> Vec<f64> {
    let mut acc = vec![0.0f64; n];
    for &s in seeds {
        let d = f64::from(h.count(s)) + 1.0;
        for &(f, x) in h.cochanged(s) {
            if let Some(slot) = acc.get_mut(f as usize) {
                *slot += f64::from(x) / d;
            }
        }
    }
    acc
}
